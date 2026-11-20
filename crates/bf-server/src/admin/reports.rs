//! Report series for the dashboard.

use crate::admin::api::{Admin, AdminRead};
use crate::http::render::DbResult;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use bf_core::AppError;
use bf_db::admin::reports;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/admin/reports/{name}", get(report))
}

async fn report(
    State(state): State<AppState>,
    Admin(..): AdminRead,
    Path(name): Path<String>,
) -> Result<Response, AppError> {
    let rows = reports::report(state.db()?, &name).await.db()?.ok_or(AppError::NotFound)?;
    let list: Vec<_> = rows.iter().map(|(day, n)| json!({ "day": day, "count": n })).collect();
    Ok(Json(list).into_response())
}
