//! IndexNow: tells participating search engines when a public page appears, so they do
//! not wait for the next crawl. Off by default (`indexnow_enabled`): it sends page addresses to
//! a third party. The key file is served at `/{key}.txt` while it is on.

use crate::forum::tree::guest;
use crate::state::AppState;
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use bf_core::slug::topic_path;
use sha2::{Digest, Sha256};

const ENDPOINT: &str = "https://api.indexnow.org/IndexNow";

/// The site's key: the `indexnow_key` setting when it is a valid key (8 to 128 letters, digits
/// and dashes), otherwise one derived from the site's address. A key is public by design, so
/// deriving it needs no secret.
pub(crate) fn key(state: &AppState) -> String {
    let set = state.settings().str("indexnow_key").trim().to_owned();
    if (8..=128).contains(&set.len()) && set.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return set;
    }
    let digest = Sha256::digest(format!("indexnow:{}", state.config.base_url));
    digest.iter().take(16).map(|b| format!("{b:02x}")).collect()
}

/// The key file, when `path` is `/{key}.txt` and IndexNow is on.
pub(crate) fn key_file(state: &AppState, path: &str) -> Option<Response> {
    if !state.settings().bool("indexnow_enabled") {
        return None;
    }
    let key = key(state);
    (path.strip_prefix('/')? == format!("{key}.txt")).then(|| {
        let mut res = key.into_response();
        res.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
        res
    })
}

/// Pings the search engines about a new topic, when IndexNow is on and guests can read it.
/// Runs in the background; a failure is only logged.
pub(crate) fn topic_published(state: &AppState, node_id: i64, topic_id: i64, slug: &str) {
    if !state.settings().bool("indexnow_enabled") || !state.config.base_url.starts_with("https://") {
        return;
    }
    let state = state.clone();
    let path = topic_path(topic_id, slug);
    tokio::spawn(async move {
        let Some(pool) = &state.pool else { return };
        let Ok(tree) = state.forum.get(pool).await else { return };
        if !tree.readable(&guest()).contains(&node_id) {
            return;
        }
        let base = state.config.base_url.trim_end_matches('/').to_owned();
        let Some(host) = base.strip_prefix("https://") else { return };
        let key = key(&state);
        let body = serde_json::json!({
            "host": host,
            "key": key,
            "keyLocation": format!("{base}/{key}.txt"),
            "urlList": [format!("{base}{path}")],
        });
        let sent = crate::platform::outbound::post_public(
            ENDPOINT,
            &[("content-type", "application/json; charset=utf-8")],
            body.to_string().into_bytes(),
        )
        .await;
        match sent {
            Ok(status) if status < 300 => {}
            Ok(status) => tracing::warn!(status, "IndexNow ping refused"),
            Err(e) => tracing::warn!(error = %e, "IndexNow ping failed"),
        }
    });
}
