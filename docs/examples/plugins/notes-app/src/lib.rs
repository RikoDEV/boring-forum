//! Notes: a small blog as a content app (Host API 1.3). Build with `sh build.sh`.
//!
//! - `page.request` answers four routes under `/app/notes-app/`: the list (`home`, paged with
//!   `offset`), an entry (`entry`, with title, description, canonical address, Open Graph and
//!   JSON-LD), the form to write one (`new`, GET and POST, behind the `write` permission) and a
//!   feed (`feed`, three formats from one answer).
//! - `sitemap.urls` lists the published entries with their dates.
//! - `content.review` publishes or drops an entry once a moderator decided.
//!
//! The host does the safe parts: it checks the permission, the CSRF token and the rate of posting
//! before this code runs, adds the token to the form, and sanitizes the HTML written here. This
//! code escapes the text it writes anyway.

use bf_plugin_api::hooks::{ContentReview, PageRequest};
use bf_plugin_api::{Reply, Request, Value, esc, export, host, json, t};

const APP: &str = "/app/notes-app";
const PER_PAGE: i64 = 10;

fn on_hook(req: Request) -> Reply {
    match req.hook.as_str() {
        "page.request" => match req.payload::<PageRequest>() {
            Ok(page) => route(&req, &page),
            Err(_) => Reply::none(),
        },
        "sitemap.urls" => sitemap(),
        "content.review" => match req.payload::<ContentReview>() {
            Ok(decision) => reviewed(&decision),
            Err(_) => Reply::none(),
        },
        _ => Reply::none(),
    }
}

fn route(req: &Request, page: &PageRequest) -> Reply {
    match page.route.as_str() {
        "home" => home(page),
        "entry" => entry(page),
        "new" => write(req, page),
        "feed" => feed(page),
        _ => Reply::page("Not found", "").with("status", json!(404)),
    }
}

/// Lower-case ASCII words joined by dashes: the address of an entry.
fn slugify(title: &str) -> String {
    let mut slug = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug: String = slug.trim_end_matches('-').chars().take(60).collect();
    if slug.is_empty() { "note".to_owned() } else { slug }
}

/// Plain text as paragraphs: blank lines separate them, every character is escaped.
fn paragraphs(text: &str) -> String {
    text.replace("\r\n", "\n")
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| format!("<p>{}</p>", esc(p).replace('\n', "<br>")))
        .collect()
}

fn excerpt(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max { flat } else { format!("{}…", flat.chars().take(max).collect::<String>().trim_end()) }
}

fn text<'a>(row: &'a Value, key: &str) -> &'a str {
    row.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn entry_url(row: &Value) -> String {
    format!("{APP}/entries/{}", text(row, "slug"))
}

fn author_link(row: &Value) -> String {
    let name = text(row, "author_name");
    format!("<a href=\"/u/{}\">{}</a>", esc(&name.to_lowercase()), esc(name))
}

fn when(row: &Value) -> String {
    let iso = text(row, "created");
    format!("<time datetime=\"{}\">{}</time>", esc(iso), esc(iso.get(..10).unwrap_or(iso)))
}

fn published() -> Value {
    json!({ "status": "published" })
}

fn home(page: &PageRequest) -> Reply {
    let n: i64 = page.query_param("page").parse().unwrap_or(1).clamp(1, 1000);
    let total = host::db_count("entries", &published()).unwrap_or(0);
    let rows = host::db_query(
        "entries",
        &json!({ "where": published(), "order": ["-created"], "limit": PER_PAGE, "offset": (n - 1) * PER_PAGE }),
    )
    .unwrap_or_default();
    let mut html = String::new();
    if page.can("write") {
        html.push_str(&format!("<p><a class=\"btn\" href=\"{APP}/new\">{}</a></p>", esc(t(&page.strings, "home.write"))));
    }
    if rows.is_empty() {
        html.push_str(&format!("<p>{}</p>", esc(t(&page.strings, "home.empty"))));
    }
    html.push_str("<div class=\"notes-list\">");
    for row in &rows {
        html.push_str(&format!(
            "<article class=\"notes-card\"><h2><a href=\"{}\">{}</a></h2><p class=\"notes-meta\">{} {} · {}</p><p>{}</p></article>",
            esc(&entry_url(row)),
            esc(text(row, "title")),
            esc(t(&page.strings, "entry.by")),
            author_link(row),
            when(row),
            esc(&excerpt(text(row, "body"), 200)),
        ));
    }
    html.push_str("</div><nav class=\"notes-pager\">");
    if n > 1 {
        html.push_str(&format!("<a rel=\"prev\" href=\"{APP}?page={}\">{}</a>", n - 1, esc(t(&page.strings, "home.newer"))));
    }
    if n * PER_PAGE < total {
        html.push_str(&format!("<a rel=\"next\" href=\"{APP}?page={}\">{}</a>", n + 1, esc(t(&page.strings, "home.older"))));
    }
    html.push_str("</nav>");
    // Page two of a list is the same list: one canonical address, no index entry of its own.
    Reply::page(t(&page.strings, "home.title"), html)
        .with("description", json!(t(&page.strings, "home.description")))
        .with("canonical", json!(APP))
        .with("noindex", json!(n > 1))
        .with("feed", json!(format!("{APP}/feed.rss")))
        .with(
            "jsonld",
            json!({ "@context": "https://schema.org", "@type": "Blog", "name": t(&page.strings, "home.title"),
                    "url": format!("{}{APP}", page.base_url) }),
        )
}

fn missing(page: &PageRequest) -> Reply {
    Reply::page(t(&page.strings, "entry.missing"), format!("<p><a href=\"{APP}\">{}</a></p>", esc(t(&page.strings, "entry.back"))))
        .with("status", json!(404))
}

fn entry(page: &PageRequest) -> Reply {
    let slug = page.param("slug");
    let Some(row) = host::db_query("entries", &json!({ "where": { "slug": slug }, "limit": 1 })).ok().and_then(|r| r.into_iter().next())
    else {
        return missing(page);
    };
    let live = text(&row, "status") == "published";
    // A waiting note is for its author and the moderators only; anyone else gets the same 404.
    let mine = page.viewer.as_ref().is_some_and(|v| v.staff || Some(v.id) == row.get("author").and_then(Value::as_i64));
    if !live && !mine {
        return missing(page);
    }
    let title = text(&row, "title");
    let mut html = String::new();
    if !live {
        html.push_str(&format!("<p class=\"notes-meta\">{}</p>", esc(t(&page.strings, "entry.pending"))));
    }
    html.push_str(&format!(
        "<article class=\"notes-body\"><p class=\"notes-meta\">{} {} · {}</p>{}</article><p><a href=\"{APP}\">{}</a></p>",
        esc(t(&page.strings, "entry.by")),
        author_link(&row),
        when(&row),
        paragraphs(text(&row, "body")),
        esc(t(&page.strings, "entry.back")),
    ));
    let url = format!("{}{}", page.base_url, entry_url(&row));
    let description = excerpt(text(&row, "body"), 160);
    Reply::page(title, html)
        .with("description", json!(description))
        .with("canonical", json!(entry_url(&row)))
        .with("noindex", json!(!live))
        .with("og", json!({ "type": "article" }))
        .with("breadcrumbs", json!([[t(&page.strings, "home.title"), APP]]))
        .with(
            "jsonld",
            json!({
                "@context": "https://schema.org", "@type": "BlogPosting", "headline": title, "description": description,
                "datePublished": text(&row, "created"), "url": url, "mainEntityOfPage": url,
                "author": { "@type": "Person", "name": text(&row, "author_name") },
            }),
        )
}

fn form(page: &PageRequest, title: &str, body: &str, error: Option<&str>) -> Reply {
    let error = error.map(|e| format!("<p class=\"notes-error\" role=\"alert\">{}</p>", esc(e))).unwrap_or_default();
    let html = format!(
        "{error}<form method=\"post\" action=\"{APP}/new\" class=\"notes-form\">\
         <label>{}<input name=\"title\" required minlength=\"3\" maxlength=\"120\" value=\"{}\"></label>\
         <label>{}<textarea name=\"body\" rows=\"10\" required minlength=\"10\" maxlength=\"20000\">{}</textarea></label>\
         <button class=\"btn\" type=\"submit\">{}</button></form>",
        esc(t(&page.strings, "form.label_title")),
        esc(title),
        esc(t(&page.strings, "form.label_body")),
        esc(body),
        esc(t(&page.strings, "form.submit")),
    );
    Reply::page(t(&page.strings, "form.title"), html).with("breadcrumbs", json!([[t(&page.strings, "home.title"), APP]]))
}

fn write(req: &Request, page: &PageRequest) -> Reply {
    if page.method != "POST" {
        return form(page, "", "", None);
    }
    let (title, body) = (page.field("title").trim(), page.field("body").trim());
    let invalid = |message: &str| form(page, title, body, Some(message)).with("status", json!(422));
    if !(3..=120).contains(&title.chars().count()) {
        return invalid(t(&page.strings, "form.error_title"));
    }
    if !(10..=20_000).contains(&body.chars().count()) {
        return invalid(t(&page.strings, "form.error_body"));
    }
    let Some(me) = page.viewer.as_ref() else { return invalid(t(&page.strings, "form.error_saving")) };
    // Moderators publish at once; everyone else waits when the setting is on.
    let hold = req.setting_bool("moderate", true) && !me.staff;
    let created = host::time_now().ok().and_then(|v| v.get("iso").and_then(Value::as_str).map(str::to_owned)).unwrap_or_default();
    let base = slugify(title);
    // The slug column is unique: when the address is taken the host says so, and a short random
    // suffix makes a new one.
    let mut slug = base.clone();
    let mut saved = false;
    for attempt in 0..4 {
        let row = json!({
            "slug": slug, "title": title, "body": body, "author": me.id, "author_name": me.username,
            "status": if hold { "pending" } else { "published" }, "created": created,
        });
        if host::db_insert_id("entries", &row).is_ok() {
            saved = true;
            break;
        }
        let suffix = host::random_hex(3).unwrap_or_else(|_| attempt.to_string());
        slug = format!("{}-{suffix}", base.chars().take(50).collect::<String>().trim_end_matches('-'));
    }
    if !saved {
        return invalid(t(&page.strings, "form.error_saving"));
    }
    let url = format!("{APP}/entries/{slug}");
    if hold {
        let _ = host::review_hold("entry", &slug, title, &excerpt(body, 300), &url, Some(me.id));
    } else {
        let _ = host::search_index("entry", &slug, title, body, &url, Some(&me.username));
    }
    Reply::redirect(url)
}

fn feed(page: &PageRequest) -> Reply {
    let rows = host::db_query("entries", &json!({ "where": published(), "order": ["-created"], "limit": 30 })).unwrap_or_default();
    let items = rows
        .iter()
        .map(|r| {
            json!({ "title": text(r, "title"), "link": entry_url(r), "html": paragraphs(text(r, "body")), "published": text(r, "created") })
        })
        .collect();
    Reply::feed(t(&page.strings, "home.title"), APP, t(&page.strings, "feed.description"), items)
}

fn sitemap() -> Reply {
    let rows = host::db_query("entries", &json!({ "where": published(), "order": ["-created"], "limit": 500 })).unwrap_or_default();
    let urls: Vec<Value> = rows.iter().map(|r| json!({ "path": entry_url(r), "lastmod": text(r, "created") })).collect();
    let mut all = vec![json!(APP)];
    all.extend(urls);
    Reply::raw(json!({ "urls": all }))
}

/// A moderator decided: an approved entry is published and offered to search; a rejected one is dropped.
fn reviewed(d: &ContentReview) -> Reply {
    let found = host::db_query("entries", &json!({ "where": { "slug": d.reference }, "limit": 1 })).unwrap_or_default();
    let Some(row) = found.first() else { return Reply::none() };
    if d.decision == "approve" {
        let _ = host::db_update("entries", &json!({ "slug": d.reference }), &json!({ "status": "published" }));
        let _ = host::search_index(
            "entry",
            &d.reference,
            text(row, "title"),
            text(row, "body"),
            &entry_url(row),
            Some(text(row, "author_name")),
        );
    } else {
        let _ = host::db_update("entries", &json!({ "slug": d.reference }), &json!({ "status": "rejected" }));
    }
    Reply::none()
}

export!(on_hook);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_plain_words() {
        assert_eq!(slugify("Hello, World! It's 2026"), "hello-world-it-s-2026");
        assert_eq!(slugify("***"), "note");
        assert_eq!(slugify("Zażółć gęślą"), "za-g-l");
        assert!(slugify(&"a".repeat(200)).len() <= 60);
    }

    #[test]
    fn text_is_escaped_into_paragraphs() {
        assert_eq!(paragraphs("a <b>\nc\n\nd"), "<p>a &lt;b&gt;<br>c</p><p>d</p>");
        assert_eq!(paragraphs("  \n\n "), "");
    }

    #[test]
    fn excerpts_are_cut_to_length_and_short_text_is_kept() {
        assert_eq!(excerpt("short   text", 50), "short text");
        assert_eq!(excerpt("one two three four", 9), "one two t…");
    }

    #[test]
    fn unknown_routes_are_a_404() {
        let page: PageRequest = serde_json::from_value(json!({ "route": "nope", "method": "GET" })).unwrap();
        let req: Request = serde_json::from_value(json!({ "hook": "page.request" })).unwrap();
        assert_eq!(route(&req, &page).0["status"], 404);
    }
}
