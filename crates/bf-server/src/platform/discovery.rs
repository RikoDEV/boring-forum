//! Machine-readable discovery: `/.well-known/security.txt` (RFC 9116),
//! `/.well-known/change-password` (W3C "well-known URL for changing passwords"),
//! `/.well-known/passkey-endpoints` (W3C passkey endpoints), `/llms.txt`, and an oEmbed
//! provider for topics.

use crate::forum::tree::guest;
use crate::http::render::DbResult;
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Json, Router};
use bf_core::AppError;
use bf_core::html::esc;
use bf_core::slug::{forum_path, topic_path};
use serde::Deserialize;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/.well-known/security.txt", get(security_txt))
        .route("/security.txt", get(|| async { Redirect::permanent("/.well-known/security.txt") }))
        .route("/.well-known/change-password", get(|| async { Redirect::to("/settings#s-password") }))
        .route("/.well-known/passkey-endpoints", get(passkey_endpoints))
        .route("/ads.txt", get(ads_txt))
        .route("/llms.txt", get(llms_txt))
        .route("/oembed", get(oembed))
}

fn text(body: String, max_age: u32) -> Response {
    let mut res = ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], body).into_response();
    if let Ok(v) = HeaderValue::from_str(&format!("public, max-age={max_age}")) {
        res.headers_mut().insert(header::CACHE_CONTROL, v);
    }
    res
}

/// IAB ads.txt: the administrator's own lines, served as written. Nothing configured means no
/// file (404), so a site without ads publishes nothing and an ad network sees no authorisation.
async fn ads_txt(State(state): State<AppState>) -> Result<Response, AppError> {
    let settings = state.settings();
    let lines: Vec<&str> = settings.str("ads_txt").lines().map(str::trim_end).collect();
    let body = lines.join("\n");
    if body.trim().is_empty() {
        // The site's own 404, like any other address it does not serve: a bare status with no body
        // and no content type is what strict checkers call an invalid response.
        return Err(AppError::NotFound);
    }
    Ok(text(format!("{}\n", body.trim()), 3_600))
}

/// RFC 9116. `Contact` is required, so without one configured there is no file (404) rather
/// than a file that sends reports nowhere. `Expires` rolls forward: always 180 days ahead,
/// because the settings are the source of truth and are current whenever the file is served.
async fn security_txt(State(state): State<AppState>) -> Result<Response, AppError> {
    let s = state.settings();
    let contact = s.str("security_contact").trim();
    let valid = |c: &str| {
        c.strip_prefix("mailto:").is_some_and(|a| a.contains('@'))
            || c.starts_with("https://")
            || c.strip_prefix("tel:").is_some_and(|n| !n.is_empty())
    };
    if !valid(contact) {
        return Err(AppError::NotFound);
    }
    let base = state.config.base_url.trim_end_matches('/');
    let expires = (chrono::Utc::now() + chrono::Duration::days(180)).format("%Y-%m-%dT%H:%M:%SZ");
    let mut body = format!("Contact: {contact}\nExpires: {expires}\n");
    let policy = s.str("security_policy_url").trim();
    if policy.starts_with("https://") {
        body.push_str(&format!("Policy: {policy}\n"));
    }
    body.push_str(&format!("Preferred-Languages: en, pl\nCanonical: {base}/.well-known/security.txt\n"));
    Ok(text(body, 86_400))
}

/// Where a password manager sends someone to add or manage a passkey.
async fn passkey_endpoints() -> Response {
    let mut res = Json(json!({ "enroll": "/settings#s-password", "manage": "/settings#s-password" })).into_response();
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=86400"));
    res
}

/// llmstxt.org: a short Markdown map of the site for language-model tools. Only what guests can
/// read is listed. Published whatever `ai_crawlers` says: it is a description of public pages,
/// and robots.txt is what grants or refuses crawling.
async fn llms_txt(State(state): State<AppState>) -> Result<Response, AppError> {
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    let readable = tree.readable(&guest());
    let s = state.settings();
    let base = state.config.base_url.trim_end_matches('/');
    let mut out = format!("# {}\n\n> {}\n\n## Forums\n\n", s.str("site_name"), s.str("site_description"));
    for id in &readable {
        if let Some(n) = tree.node(*id).filter(|n| n.kind == "forum") {
            out.push_str(&format!("- [{}]({base}{}): {} topics\n", n.title, forum_path(n.id, &n.slug), n.topic_count));
        }
    }
    out.push_str(&format!(
        "\n## Feeds\n\n- [Latest topics, JSON Feed]({base}/latest.json)\n- [Latest topics, Atom]({base}/latest.atom)\n- [Sitemap]({base}/sitemap.xml)\n"
    ));
    let legal = bf_web::pages::LegalDocs::current();
    let policies: Vec<String> = [
        (legal.terms, "Terms", "terms"),
        (legal.privacy, "Privacy", "privacy"),
        (legal.guidelines, "Guidelines", "guidelines"),
    ]
    .into_iter()
    .filter(|(on, ..)| *on)
    .map(|(_, label, kind)| format!("- [{label}]({base}/legal/{kind})\n"))
    .collect();
    if !policies.is_empty() {
        out.push_str(&format!("\n## Policies\n\n{}", policies.concat()));
    }
    Ok(text(out, 3600))
}

#[derive(Deserialize)]
struct OembedQuery {
    url: String,
    #[serde(default)]
    format: String,
    maxwidth: Option<u32>,
}

/// oEmbed 1.0 provider for topics, `rich` type. The HTML is a plain quote card with a link:
/// no script and no frame, so a consumer can show it without trusting this site.
async fn oembed(State(state): State<AppState>, Query(q): Query<OembedQuery>) -> Result<Response, AppError> {
    if !q.format.is_empty() && q.format != "json" {
        return Ok(StatusCode::NOT_IMPLEMENTED.into_response());
    }
    let base = state.config.base_url.trim_end_matches('/');
    let id = crate::forum::onebox::internal_topic(&q.url, base).ok_or(AppError::NotFound)?;
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    // Only what a guest may read, whoever asks: an embed is shown to strangers.
    let rows = bf_db::forum::feeds::feed_posts(pool, id, &tree.readable(&guest())).await.db()?;
    let first = rows.first().ok_or(AppError::NotFound)?;
    let s = state.settings();
    let link = esc(&format!("{base}{}", topic_path(first.id, &first.slug)));
    let excerpt = bf_seo::excerpt(&first.cooked, 200);
    let width = q.maxwidth.unwrap_or(550).clamp(200, 800);
    let html = format!(
        r#"<blockquote class="boring-forum-embed" cite="{link}"><p><a href="{link}"><strong>{}</strong></a></p><p>{}</p><footer>{}</footer></blockquote>"#,
        esc(&first.title),
        esc(&excerpt),
        esc(s.str("site_name")),
    );
    let mut res = Json(json!({
        "version": "1.0",
        "type": "rich",
        "title": first.title,
        "provider_name": s.str("site_name"),
        "provider_url": base,
        "cache_age": 3600,
        "width": width,
        "height": null,
        "html": html,
    }))
    .into_response();
    // No CORS header: oEmbed consumers fetch this from their servers, and CORS stays off by default
    // (the ZAP baseline fails on a cross-domain misconfiguration).
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=3600"));
    Ok(res)
}
