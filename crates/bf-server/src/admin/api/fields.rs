//! Custom profile fields.

use crate::admin::api::{Admin, AdminRead, AdminWrite};
use crate::http::render::DbResult;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use bf_core::AppError;
use serde_json::json;

/// The fields with how many members answered each, for the panel's list.
pub(super) async fn list_fields(State(state): State<AppState>, Admin(_, v): AdminRead) -> Result<Response, AppError> {
    let pool = state.db()?;
    let fields = bf_db::accounts::fields::fields(pool).await.db()?;
    let counts = bf_db::accounts::fields::answer_counts(pool).await.db()?;
    let rows: Vec<serde_json::Value> = fields
        .into_iter()
        .map(|f| {
            let answers = counts.iter().find(|(id, _)| *id == f.id).map_or(0, |(_, n)| *n);
            let hint = f.hint(&v.t());
            let mut row = serde_json::to_value(f).unwrap_or_default();
            row["answers"] = json!(answers);
            row["hint"] = json!(hint);
            row
        })
        .collect();
    Ok(Json(rows).into_response())
}

pub(super) async fn create_field(
    State(state): State<AppState>,
    Admin(..): AdminWrite,
    Json(mut f): Json<bf_db::accounts::fields::Field>,
) -> Result<Response, AppError> {
    f.normalise().map_err(AppError::Validation)?;
    let id = bf_db::accounts::fields::create_field(state.db()?, &f).await.db()?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))).into_response())
}

/// Changes a field. Its type cannot change, since members' answers were checked against it.
pub(super) async fn update_field(
    State(state): State<AppState>,
    Admin(..): AdminWrite,
    Path(id): Path<i64>,
    Json(mut f): Json<bf_db::accounts::fields::Field>,
) -> Result<Response, AppError> {
    let pool = state.db()?;
    let old =
        bf_db::accounts::fields::fields(pool).await.db()?.into_iter().find(|x| x.id == id).ok_or(AppError::NotFound)?;
    f.kind = old.kind;
    f.normalise().map_err(AppError::Validation)?;
    bf_db::accounts::fields::update_field(pool, id, &f).await.found()?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Body: the field ids in their new order.
pub(super) async fn reorder_fields(
    State(state): State<AppState>,
    Admin(..): AdminWrite,
    Json(ids): Json<Vec<i64>>,
) -> Result<Response, AppError> {
    if ids.len() > 500 {
        return Err(AppError::Validation("Too many fields.".into()));
    }
    bf_db::accounts::fields::reorder_fields(state.db()?, &ids).await.db()?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub(super) async fn delete_field(
    State(state): State<AppState>,
    Admin(..): AdminWrite,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    bf_db::accounts::fields::delete_field(state.db()?, id).await.found()?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
