//! Tags: list, create, rename, delete.

use crate::admin::api::{Admin, AdminWrite};
use crate::http::render::DbResult;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use bf_core::AppError;
use serde::Deserialize;

admin_get!(list_tags(state) bf_db::forum::tags::all_tags(state.db()?).await.db()?);

/// `POST /api/v1/admin/tags`: a tag that exists before any topic uses it.
pub(super) async fn create_tag(
    State(state): State<AppState>,
    Admin(..): AdminWrite,
    Json(p): Json<TagPatch>,
) -> Result<Response, AppError> {
    let (name, slug) = crate::forum::posting::clean_tag(&state, &p.name)
        .ok_or_else(|| AppError::Validation("Not a valid tag name.".into()))?;
    let id = bf_db::forum::tags::create_tag(state.db()?, &name, &slug)
        .await
        .db()?
        .ok_or_else(|| AppError::Validation("That tag already exists.".into()))?;
    Ok((StatusCode::CREATED, Json(serde_json::json!({ "id": id, "name": name }))).into_response())
}

#[derive(Deserialize)]
pub(super) struct TagPatch {
    pub(super) name: String,
}

pub(super) async fn rename_tag(
    State(state): State<AppState>,
    Admin(..): AdminWrite,
    Path(id): Path<i64>,
    Json(p): Json<TagPatch>,
) -> Result<Response, AppError> {
    let (name, slug) = crate::forum::posting::clean_tag(&state, &p.name)
        .ok_or_else(|| AppError::Validation("Not a valid tag name.".into()))?;
    bf_db::forum::tags::rename_tag(state.db()?, id, &name, &slug).await.found()?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub(super) async fn delete_tag(
    State(state): State<AppState>,
    Admin(..): AdminWrite,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    bf_db::forum::tags::delete_tag(state.db()?, id).await.found()?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
