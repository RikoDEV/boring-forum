//! Administration of channels and of flagged messages.

use crate::admin::api::admin;
use crate::http::render::{DbResult, db_err};
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use bf_core::AppError;
use bf_db::community::chat;
use serde::Deserialize;
use serde_json::json;

use super::changed;

#[derive(Deserialize)]
pub(super) struct ChannelBody {
    pub(super) name: String,
    #[serde(default)]
    pub(super) description: String,
    #[serde(default = "open_access")]
    pub(super) access: String,
    #[serde(default)]
    pub(super) group_ids: Vec<i64>,
    #[serde(default)]
    pub(super) archived: bool,
}

pub(super) fn open_access() -> String {
    "open".into()
}

pub(super) fn check_channel(b: &ChannelBody) -> Result<(), AppError> {
    let n = b.name.trim().chars().count();
    if n == 0 || n > 60 {
        return Err(AppError::Validation("A channel name is 1 to 60 characters.".into()));
    }
    if !matches!(b.access.as_str(), "open" | "groups" | "invite") {
        return Err(AppError::Validation("Access must be open, groups or invite.".into()));
    }
    if b.access == "groups" && b.group_ids.is_empty() {
        return Err(AppError::Validation("Choose at least one group.".into()));
    }
    Ok(())
}

pub(super) async fn admin_channels(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, false)?;
    Ok(Json(chat::admin_channels(state.db()?).await.db()?).into_response())
}

pub(super) async fn add_channel(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Json(b): Json<ChannelBody>,
) -> Result<Response, AppError> {
    let who = admin(&state, &v, &headers, true)?;
    check_channel(&b)?;
    let id = chat::create_channel(
        state.db()?,
        b.name.trim(),
        b.description.trim(),
        &b.access,
        &b.group_ids,
        who.unwrap_or(0),
    )
    .await
    .map_err(bad_node)?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))).into_response())
}

pub(super) fn bad_node(e: sqlx::Error) -> AppError {
    match e {
        sqlx::Error::Database(d) if d.is_foreign_key_violation() => {
            AppError::Validation("Unknown forum or group.".into())
        }
        e => db_err(e),
    }
}

pub(super) async fn update_channel(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(b): Json<ChannelBody>,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, true)?;
    check_channel(&b)?;
    if !chat::update_channel(state.db()?, id, b.name.trim(), b.description.trim(), &b.access, &b.group_ids, b.archived)
        .await
        .map_err(bad_node)?
    {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub(super) async fn delete_channel(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, true)?;
    if !chat::delete_channel(state.db()?, id).await.db()? {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub(super) async fn admin_flags(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, false)?;
    Ok(Json(chat::flags(state.db()?).await.db()?).into_response())
}

/// Dismisses (`DELETE`) or upholds and removes (`POST`) a flag on a message.
pub(super) async fn decide_flag(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    method: axum::http::Method,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let who = admin(&state, &v, &headers, true)?;
    if !chat::decide_flags(state.db()?, id, method == axum::http::Method::DELETE, who.unwrap_or(0)).await.db()? {
        return Err(AppError::NotFound);
    }
    changed(&state, id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
