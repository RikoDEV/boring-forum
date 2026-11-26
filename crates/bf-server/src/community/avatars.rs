//! Generated letter avatars.

use crate::state::AppState;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use sha2::{Digest, Sha256};

pub fn router() -> Router<AppState> {
    Router::new().route("/avatar/{name}", get(letter_avatar))
}

/// Fresh for five minutes, as a member may change their picture; after that a returning visitor
/// sees the cached one at once while the browser asks again.
const AVATAR_CACHE: &str = "public, max-age=300, stale-while-revalidate=604800";

/// Muted, readable colours (white text passes WCAG AA on each).
const PALETTE: [&str; 8] = ["#2f5d8a", "#6b4c9a", "#a23b5f", "#9a5b13", "#2d7a4f", "#3d6b7a", "#7a4a3d", "#55606e"];

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// `GET /avatar/{name}.svg`: a deterministic letter avatar ("generated letter avatars").
async fn letter_avatar(State(state): State<AppState>, Path(file): Path<String>) -> Response {
    let Some(name) = file.strip_suffix(".svg").filter(|n| n.len() <= 40) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    // Anything that lost its author still gets a picture.
    let name = if name.is_empty() { "deleted" } else { name };
    if let Some(url) = crate::forum::uploads::uploaded_avatar(&state, name).await {
        let mut res = Redirect::to(&url).into_response();
        res.headers_mut().insert(header::CACHE_CONTROL, axum::http::HeaderValue::from_static(AVATAR_CACHE));
        return res;
    }
    if state.settings().bool("gravatar_enabled")
        && let Some((bytes, kind)) = gravatar(&state, name).await
    {
        return ([(header::CONTENT_TYPE, kind), (header::CACHE_CONTROL, "public, max-age=3600")], bytes)
            .into_response();
    }
    let lower = name.to_lowercase();
    let hash = lower.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
    let colour = PALETTE[(hash as usize) % PALETTE.len()];
    let letter: String =
        lower.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect()).unwrap_or_default();
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" role="img"><rect width="64" height="64" rx="32" fill="{colour}"/><text x="32" y="32" dy=".35em" text-anchor="middle" font-family="system-ui,sans-serif" font-size="30" fill="#fff">{}</text></svg>"##,
        escape(&letter)
    );
    (
        // Short cache: the member may upload a picture later.
        [(header::CONTENT_TYPE, "image/svg+xml"), (header::CACHE_CONTROL, AVATAR_CACHE)],
        svg,
    )
        .into_response()
}

/// A member's Gravatar, fetched by this server so visitors' browsers never contact Gravatar
/// (no IP or email hash leaves the site, and the CSP needs no outside image host). Cached for
/// hours either way; a slow answer falls back to the letter and is asked again next time.
async fn gravatar(state: &AppState, name: &str) -> Option<(axum::body::Bytes, &'static str)> {
    let key = name.to_lowercase();
    if let Some(hit) = state.gravatars.get(&key) {
        return hit;
    }
    let pool = state.pool.as_ref()?;
    let Ok(email) = bf_db::forum::uploads::gravatar_email(pool, name).await else { return None };
    let Some(email) = email else {
        state.gravatars.insert(key, None);
        return None;
    };
    let digest = Sha256::digest(email.trim().to_lowercase().as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    // d=404: no picture means an error, not Gravatar's own placeholder.
    let url = format!("https://gravatar.com/avatar/{hex}?s=128&d=404");
    let fetched = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        crate::platform::outbound::fetch_public(&url, 256 * 1024),
    )
    .await;
    let found = match fetched {
        Err(_) => return None,
        Ok(Err(_)) => None,
        Ok(Ok(bytes)) => image_kind(&bytes).map(|kind| (axum::body::Bytes::from(bytes), kind)),
    };
    state.gravatars.insert(key, found.clone());
    found
}

/// Only real images are passed on, whatever the remote end claims.
fn image_kind(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if b.starts_with(b"GIF8") {
        Some("image/gif")
    } else if b.len() > 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}
