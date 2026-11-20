//! Forum topics as comments on other sites:
//! `<iframe src="https://forum/embed/comments?url=https%3A%2F%2Fblog%2Fpost"></iframe>`.
//! Only pages on hosts in `embed_hosts` get a topic, created at first view in `embed_forum`, and
//! only those hosts may frame the result. The frame shows public posts and links out to reply:
//! it never acts for the visitor, so it has no session or CSRF surface.

use crate::http::i18n::trh;
use crate::http::render::DbResult;
use crate::state::AppState;
use axum::Router;
use axum::extract::{Query, State};
use axum::http::{HeaderValue, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use bf_core::AppError;
use bf_core::html::esc;
use bf_core::perms::{Actor, Perms};
use bf_core::slug::{slugify, topic_path};
use bf_db::forum::embeds;
use serde::Deserialize;

pub fn router() -> Router<AppState> {
    Router::new().route("/embed/comments", get(comments))
}

const MAX_NEW_PER_HOUR: i64 = 50;

#[derive(Deserialize)]
struct EmbedQuery {
    url: String,
}

/// The page's address without its fragment, if its host is on the list.
fn allowed_page(raw: &str, hosts: &str) -> Option<String> {
    let mut u = url::Url::parse(raw).ok()?;
    if !matches!(u.scheme(), "https" | "http") || !u.username().is_empty() {
        return None;
    }
    let host = u.host_str()?.to_ascii_lowercase();
    hosts.split(',').map(str::trim).any(|h| !h.is_empty() && h.eq_ignore_ascii_case(&host)).then_some(())?;
    // One thread per page: a query string could otherwise make endless threads for one page.
    u.set_fragment(None);
    u.set_query(None);
    Some(u.to_string())
}

fn page_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let open = start + lower[start..].find('>')? + 1;
    let end = open + lower[open..].find("</title")?;
    // One line: the title becomes a topic title and part of a Markdown link.
    let title = html.get(open..end)?.split_whitespace().collect::<Vec<_>>().join(" ");
    (!title.is_empty()).then(|| title.chars().take(150).collect())
}

/// The first post of a comments topic: a link to the page, whatever the page's title or the
/// visitor-supplied address contains.
fn comments_body(title: &str, url: &str) -> String {
    let mut label = String::new();
    for c in title.chars() {
        match c {
            '\\' | '[' | ']' | '`' | '*' | '_' | '<' | '>' | '!' | '#' | '|' | '~' | '&' => {
                label.push('\\');
                label.push(c);
            }
            c if c.is_whitespace() => label.push(' '),
            c => label.push(c),
        }
    }
    let target = url.replace('(', "%28").replace(')', "%29").replace(' ', "%20");
    format!("{} [{label}]({target})", bf_core::i18n::en("Comments on"))
}

async fn create_topic(state: &AppState, url: &str, node: i64) -> Result<i64, AppError> {
    let pool = state.db()?;
    if embeds::embeds_last_hour(pool).await.db()? >= MAX_NEW_PER_HOUR {
        return Err(AppError::NotFound);
    }
    // Through the guarded fetcher: an allowed name could still resolve somewhere internal.
    // A page that does not exist gets no thread, so made-up addresses cannot fill the forum.
    let fetched = crate::platform::outbound::fetch_public(url, 512 * 1024).await.map_err(|_| AppError::NotFound)?;
    let title = page_title(&String::from_utf8_lossy(&fetched)).unwrap_or_else(|| "Discussion".to_owned());
    let title = crate::platform::plugins::filter_text(state, "item.before_create", "title", &title)
        .await
        .map_err(|_| AppError::NotFound)?;
    let slug = slugify(&title);
    let author = bf_db::forum::anonymous::anonymous_account(pool).await.db()?;
    let body = comments_body(&title, url);
    let body = crate::platform::plugins::before_post_system(state, author, &body).await.ok_or(AppError::NotFound)?;
    let cooked = bf_cook::cook(&body);
    let post = bf_db::forum::posts::NewPost {
        author_id: author,
        raw: &body,
        cooked: &cooked.html,
        cooked_version: bf_cook::COOK_VERSION,
        ip: None,
        pending: false,
    };
    let item = bf_db::forum::posts::create_topic(pool, node, &title, &slug, &post).await.db()?;
    embeds::add_embed(pool, url, item).await.db()?;
    crate::forum::posting::after_system_post(state, author, bf_core::i18n::en("Anonymous"), &title, &slug, item, node, 1).await;
    state.forum.invalidate();
    // Two first views at once: both made a topic, and the earlier one is the thread.
    Ok(embeds::embed_item(pool, url).await.db()?.unwrap_or(item))
}

async fn comments(State(state): State<AppState>, Query(q): Query<EmbedQuery>) -> Result<Response, AppError> {
    let s = state.settings();
    let hosts = s.str("embed_hosts").to_owned();
    let node = s.int("embed_forum");
    let url = allowed_page(&q.url, &hosts).ok_or(AppError::NotFound)?;
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    let guest = Actor { groups: vec![1.into()], ..Default::default() };
    // Everything here is shown publicly, so the forum must be public.
    if node <= 0 || !tree.perms(&guest, node).has(Perms::READ) {
        return Err(AppError::NotFound);
    }
    let item = match embeds::embed_item(pool, &url).await.db()? {
        Some(i) => i,
        None => create_topic(&state, &url, node).await?,
    };
    let t = bf_db::forum::topics::topic_for(pool, item, None).await.db()?.ok_or(AppError::NotFound)?;
    if t.state != "visible" || !tree.perms(&guest, t.node_id).has(Perms::READ) {
        return Err(AppError::NotFound);
    }
    let posts = bf_db::forum::posts::topic_posts(pool, item, 2, 100, false, None).await.db()?;
    let base = &state.config.base_url;
    let mut html = String::from(
        "<!doctype html><html><head><meta charset=\"utf-8\"><base target=\"_blank\"><style>body{font:15px/1.5 system-ui,sans-serif;margin:0}article{border-block-end:1px solid #ddd;padding:.5rem 0}img{max-width:100%}</style></head><body>",
    );
    for p in posts.iter().filter(|p| p.action.is_none()) {
        html.push_str(&format!(
            "<article><strong>{}</strong><div>{}</div></article>",
            match p.author.as_deref() {
                Some(n) => format!("<a href=\"{}{}\">{}</a>", esc(base), esc(&bf_core::slug::profile_path(0, n)), esc(n)),
                None => trh("deleted member"),
            },
            p.cooked
        ));
    }
    html.push_str(&format!(
        "<p><a href=\"{}{}\">{tr1}</a></p></body></html>",
        esc(base),
        esc(&topic_path(item, &t.slug)),
        tr1 = trh("Join the discussion")
    ));
    let ancestors: Vec<String> =
        hosts.split(',').map(str::trim).filter(|h| !h.is_empty()).map(|h| format!("https://{h}")).collect();
    let csp = format!(
        "default-src 'none'; style-src 'unsafe-inline'; img-src https:; base-uri 'none'; form-action 'none'; frame-ancestors {}",
        ancestors.join(" ")
    );
    let mut res = Html(html).into_response();
    if let Ok(v) = HeaderValue::from_str(&csp) {
        res.headers_mut().insert(header::CONTENT_SECURITY_POLICY, v);
    }
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=30"));
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_listed_hosts() {
        let hosts = "blog.example, other.example";
        assert_eq!(allowed_page("https://blog.example/a#c", hosts).as_deref(), Some("https://blog.example/a"));
        assert!(allowed_page("https://blog.example.evil.test/", hosts).is_none());
        assert!(allowed_page("https://blog.example@evil.test/", hosts).is_none());
        assert!(allowed_page("https://user@blog.example/", hosts).is_none());
        assert!(allowed_page("file:///etc/passwd", hosts).is_none());
        assert!(allowed_page("https://blog.example/", "").is_none());
    }

    #[test]
    fn the_comments_link_cannot_be_hijacked() {
        // A ")" in the address would end the link target and start a second link.
        let url = allowed_page("https://blog.example/ok)[click](https://evil.test", "blog.example").unwrap();
        let body = comments_body("Hi", &url);
        assert!(body.starts_with("Comments on [Hi](https://blog.example/ok%29"), "{body}");
        let html = bf_cook::cook(&body).html;
        assert_eq!(html.matches("<a ").count(), 1, "{html}");
        assert!(!html.contains("href=\"https://evil.test"));
        // A title with brackets and a line break stays inside one label.
        let title = page_title("<title>a\n](https://evil.test)[b</title>").unwrap();
        let body = comments_body(&title, "https://blog.example/");
        assert!(!body.contains('\n'));
        let html = bf_cook::cook(&body).html;
        assert_eq!(html.matches("<a ").count(), 1, "{html}");
        assert!(!html.contains("href=\"https://evil.test"));
    }

    #[test]
    fn titles_are_read_from_pages() {
        assert_eq!(page_title("<html><TITLE lang=en> Hi </TITLE>").as_deref(), Some("Hi"));
        assert_eq!(page_title("<html>"), None);
    }
}
