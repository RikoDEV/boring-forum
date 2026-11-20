//! Creating forums.

use crate::forum::tree::GUESTS;
use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use bf_core::AppError;
use bf_core::perms::Perms;
use bf_core::slug::slugify;
use bf_db::forum::nodes;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/admin/nodes", post(create_node))
}

#[derive(Deserialize)]
struct NewNode {
    parent_id: Option<i64>,
    kind: String,
    title: String,
    #[serde(default)]
    description: String,
    /// Group id → permission bits. Left out: a forum under a parent uses the parent's permissions,
    /// a top-level one gets guests read, members post.
    grants: Option<HashMap<i64, u32>>,
    /// Target of a link forum.
    url: Option<String>,
}

/// Admin: create a category or forum (`POST /api/v1/admin/nodes`). Like every admin write: an
/// administrator's session with the CSRF token in `X-CSRF-Token` and a recent password
/// confirmation, or an admin key.
async fn create_node(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Json(n): Json<NewNode>,
) -> Result<Response, AppError> {
    let who = crate::admin::api::admin(&state, &v, &headers, true)?;
    if !matches!(n.kind.as_str(), "category" | "forum" | "link" | "archive") || n.title.trim().is_empty() {
        return Err(AppError::Validation(
            "kind must be category, forum, link or archive, and title is required".into(),
        ));
    }
    let url_ok =
        n.url.as_deref().is_some_and(|u| u.starts_with("https://") || u.starts_with("http://") || u.starts_with('/'));
    if (n.kind == "link") != url_ok {
        return Err(AppError::Validation("Link forums need an http(s) or /path url; other kinds take none.".into()));
    }
    let read = Perms::READER;
    let post = read.union(Perms::CREATE).union(Perms::REPLY).union(Perms::ATTACH);
    let grants: Vec<(i64, Perms)> = match n.grants {
        Some(g) => g.into_iter().map(|(k, v)| (k, Perms(v & Perms::ALL.0))).collect(),
        None if n.parent_id.is_some() => Vec::new(),
        None => vec![(GUESTS, read), (2, post), (3, post.union(Perms::MODERATE)), (4, Perms::ALL)],
    };
    let pool = state.db()?;
    let desc = bf_cook::cook(&n.description).html;
    let link = n.url.as_deref().map(str::trim).filter(|u| !u.is_empty());
    let id = nodes::create_node_with_link(
        pool,
        n.parent_id,
        &n.kind,
        n.title.trim(),
        &slugify(&n.title),
        &desc,
        link,
        &grants,
    )
    .await
    .db()?;
    crate::admin::api::audit_log(pool, who, "node.create", &format!("node:{id}"), json!({})).await?;
    state.forum.invalidate();
    crate::admin::text_overrides::refresh(&state).await;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))).into_response())
}
