//! Bulk group membership.

use crate::admin::api::admin;
use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use bf_core::AppError;
use bf_db::community::groups;
use serde::Deserialize;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/admin/bulk-group", post(bulk_group))
}

#[derive(Deserialize)]
struct BulkGroup {
    #[serde(default)]
    min_posts: i32,
    max_posts: Option<i32>,
    group: i64,
}

async fn bulk_group(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Json(b): Json<BulkGroup>,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, true)?;
    if b.group == 3 || b.group == 4 || b.group <= 0 {
        return Err(AppError::Validation("Staff groups cannot be granted in bulk.".into()));
    }
    let n = groups::bulk_add_group(state.db()?, b.min_posts, b.max_posts, b.group).await.db()?;
    Ok(Json(json!({ "added": n })).into_response())
}
