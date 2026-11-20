//! Joining, leaving, muting and marking a channel read.

use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use bf_core::AppError;
use bf_db::community::chat;
use serde::Deserialize;

use super::viewable;

pub(super) async fn membership(state: &AppState, v: &Viewer, headers: &HeaderMap, id: i64) -> Result<i64, AppError> {
    v.check_csrf_headers(headers)?;
    viewable(state, v, id).await?;
    Ok((v.require_user()?.id).get())
}

pub(super) async fn join(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    // An invite-only channel is only visible to members, so this cannot let anyone in.
    let user = membership(&state, &v, &headers, id).await?;
    chat::join(state.db()?, id, user).await.db()?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub(super) async fn leave(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let user = membership(&state, &v, &headers, id).await?;
    chat::leave(state.db()?, id, user).await.db()?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize)]
pub(super) struct MuteBody {
    pub(super) muted: bool,
}

pub(super) async fn mute(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(b): Json<MuteBody>,
) -> Result<Response, AppError> {
    let user = membership(&state, &v, &headers, id).await?;
    chat::set_muted(state.db()?, id, user, b.muted).await.db()?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize)]
pub(super) struct ReadBody {
    pub(super) id: i64,
}

pub(super) async fn read(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(b): Json<ReadBody>,
) -> Result<Response, AppError> {
    let user = membership(&state, &v, &headers, id).await?;
    chat::mark_read(state.db()?, id, user, b.id).await.db()?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
