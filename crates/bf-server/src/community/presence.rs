//! Which members shown on a page are online, asked for separately so pages stay cacheable.

use crate::http::render::DbResult;
use crate::state::AppState;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use bf_core::AppError;
use bf_db::community::directory;
use serde::Deserialize;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/online", get(online))
}

#[derive(Deserialize)]
struct OnlineQuery {
    #[serde(default)]
    names: String,
}

/// Who of the avatars on a page is online. Asked separately so pages stay cacheable.
async fn online(
    State(state): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<OnlineQuery>,
) -> Result<Response, AppError> {
    let names: Vec<String> = q.names.split(',').filter(|n| !n.is_empty()).take(100).map(str::to_owned).collect();
    let rows = directory::online_among(state.db()?, &names, 15).await.db()?;
    let mut res = Json(rows).into_response();
    res.headers_mut()
        .insert(axum::http::header::CACHE_CONTROL, axum::http::HeaderValue::from_static("public, max-age=30"));
    Ok(res)
}
