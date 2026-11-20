//! Link previews (§ oneboxes): a bare link on its own line becomes a card. Posts are not
//! changed; the page asks here when such a link comes into view. Only addresses that appear in
//! a post are fetched, so this cannot be used as an open proxy, and results are cached.

use crate::forum::topics::readable_topic;
use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use bf_core::AppError;
use bf_db::forum::links;
use serde::Deserialize;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/onebox", get(preview))
}

const MAX_PAGE: usize = 512 * 1024;

const MAX_HOPS: usize = 3;

#[derive(Deserialize)]
struct PreviewQuery {
    url: String,
}

fn answer(state: &AppState, p: &links::Preview) -> Response {
    // The card's picture comes from the site's imgproxy when there is one, so a reader's browser
    // does not call the other site for it.
    let image = super::imgproxy::Config::from_settings(&state.settings(), &state.config.base_url)
        .and_then(|c| c.url(&p.image, 640))
        .unwrap_or_else(|| p.image.clone());
    let mut res = Json(json!({ "title": p.title, "description": p.description, "image": image, "site": p.site }))
        .into_response();
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=3600"));
    res
}

async fn preview(
    State(state): State<AppState>,
    v: Viewer,
    Query(q): Query<PreviewQuery>,
) -> Result<Response, AppError> {
    let pool = state.db()?;
    // Our own topics come from the database, with the viewer's permissions.
    if let Some(id) = internal_topic(&q.url, &state.config.base_url) {
        let (t, _, _) = readable_topic(&state, &v, id).await?;
        let site = state.settings().str("site_name").to_owned();
        let mut res = answer(&state, &links::Preview {
            title: t.title,
            description: String::new(),
            image: String::new(),
            site,
            ok: true,
        });
        // Permission-checked: no shared cache may keep this.
        res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
        return Ok(res);
    }
    // Only addresses in public, visible posts: the server fetches for anyone who asks.
    let tree = state.forum.get(pool).await?;
    let readable = tree.readable(&crate::forum::tree::guest());
    if !links::is_posted_link(pool, &q.url, &readable).await.db()? {
        return Err(AppError::NotFound);
    }
    let p = if let Some(p) = links::cached(pool, &q.url).await.db()? {
        p
    } else {
        let p = match crate::platform::plugins::onebox_card(&state, &q.url).await {
            Some(card) => card,
            None => fetch(&q.url).await.unwrap_or_default(),
        };
        links::store(pool, &q.url, &p).await.db()?;
        p
    };
    if !p.ok {
        return Err(AppError::NotFound);
    }
    Ok(answer(&state, &p))
}

pub(crate) fn internal_topic(raw: &str, base: &str) -> Option<i64> {
    let path = raw.strip_prefix(base.trim_end_matches('/'))?;
    if let Some(rest) = path.strip_prefix("/t/") {
        let mut parts = rest.split(['/', '?', '#']);
        let first = parts.next()?;
        return parts.next().filter(|p| !p.is_empty()).unwrap_or(first).parse().ok();
    }
    // Old address, as older posts still link them.
    path.strip_prefix("/topic/")?.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

async fn fetch(start: &str) -> Option<links::Preview> {
    let mut url = start.to_owned();
    for _ in 0..=MAX_HOPS {
        match crate::platform::outbound::fetch_page(&url, MAX_PAGE).await.ok()? {
            crate::platform::outbound::Page::Redirect(to) => {
                url = url::Url::parse(&url).ok()?.join(&to).ok()?.to_string();
            }
            crate::platform::outbound::Page::Html(bytes) => return parse(&String::from_utf8_lossy(&bytes), &url),
        }
    }
    None
}

fn decode(s: &str) -> String {
    let out = s
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn cut(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// The value of one attribute inside a tag's text, compared in lower case.
fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let at = from + i;
        from = at + name.len();
        let before_ok = at == 0 || lower.as_bytes()[at - 1].is_ascii_whitespace();
        let after = lower[from..].trim_start();
        if !before_ok || !after.starts_with('=') {
            continue;
        }
        let start = tag.len() - after.len() + 1;
        let value = tag[start..].trim_start();
        let quote = value.chars().next()?;
        return if quote == '"' || quote == '\'' {
            value[1..].find(quote).map(|e| value[1..=e].to_owned())
        } else {
            Some(value.split_whitespace().next()?.trim_end_matches('/').to_owned())
        };
    }
    None
}

/// Open Graph values, falling back on the page's title and description.
fn parse(html: &str, url: &str) -> Option<links::Preview> {
    let head = html.get(..html.to_ascii_lowercase().find("</head>").unwrap_or(html.len())).unwrap_or(html);
    let lower = head.to_ascii_lowercase();
    let (mut title, mut og_title, mut description, mut og_description, mut image, mut site) =
        (String::new(), String::new(), String::new(), String::new(), String::new(), String::new());
    let mut from = 0;
    while let Some(i) = lower[from..].find("<meta") {
        let start = from + i;
        let Some(end) = lower[start..].find('>') else { break };
        let tag = &head[start + 5..start + end];
        from = start + end;
        let key = attr(tag, "property").or_else(|| attr(tag, "name")).unwrap_or_default().to_ascii_lowercase();
        let Some(value) = attr(tag, "content") else { continue };
        match key.as_str() {
            "og:title" => og_title = value,
            "og:description" => og_description = value,
            "description" => description = value,
            "og:image" => image = value,
            "og:site_name" => site = value,
            _ => {}
        }
    }
    if let (Some(a), Some(b)) = (lower.find("<title"), lower.find("</title>"))
        && let Some(open) = lower[a..].find('>')
        && a + open < b
    {
        head[a + open + 1..b].clone_into(&mut title);
    }
    let page = url::Url::parse(url).ok()?;
    let title = decode(if og_title.is_empty() { &title } else { &og_title });
    if title.is_empty() {
        return None;
    }
    // Only pictures that the page's security policy lets the browser load.
    let image = page
        .join(&image)
        .ok()
        .filter(|u| !image.is_empty() && u.scheme() == "https")
        .map(String::from)
        .unwrap_or_default();
    let host = page.host_str().unwrap_or_default().trim_start_matches("www.").to_owned();
    Some(links::Preview {
        ok: true,
        title: cut(&title, 200),
        description: cut(&decode(if og_description.is_empty() { &description } else { &og_description }), 300),
        image,
        site: cut(&if site.is_empty() { host } else { decode(&site) }, 80),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_open_graph_first() {
        let html = r#"<html><head><title>Plain</title><META content="OG &amp; title" property="og:title">
            <meta name='description' content='Desc'><meta property="og:image" content="/pic.png"></head><body><meta property="og:title" content="ignored"></body>"#;
        let p = parse(html, "https://www.example.com/a/b").unwrap();
        assert_eq!(p.title, "OG & title");
        assert_eq!(p.description, "Desc");
        assert_eq!(p.image, "https://www.example.com/pic.png");
        assert_eq!(p.site, "example.com");
    }

    #[test]
    fn falls_back_and_refuses_empty_or_unsafe() {
        let p = parse(
            "<title>Just a\n  title</title><meta property=og:image content=http://x.test/a.png>",
            "https://x.test/",
        )
        .unwrap();
        assert_eq!(p.title, "Just a title");
        assert_eq!(p.image, "");
        assert!(parse("<p>nothing</p>", "https://x.test/").is_none());
    }

    #[test]
    fn knows_its_own_topics() {
        assert_eq!(internal_topic("https://f.test/topic/12-hello/", "https://f.test/"), Some(12));
        assert_eq!(internal_topic("https://f.test/t/hello/12", "https://f.test/"), Some(12));
        assert_eq!(internal_topic("https://f.test/t/hello/12/3#post-3", "https://f.test/"), Some(12));
        assert_eq!(internal_topic("https://f.test/t/hello", "https://f.test/"), None);
        assert_eq!(internal_topic("https://f.test/forum/12/", "https://f.test"), None);
        assert_eq!(internal_topic("https://other.test/topic/12/", "https://f.test"), None);
    }
}
