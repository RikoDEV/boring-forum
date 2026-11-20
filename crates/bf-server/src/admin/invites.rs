//! Invitations.

use crate::admin::api::{Admin, AdminRead, AdminWrite};
use crate::http::render::DbResult;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Json, Router};
use bf_core::AppError;
use serde::Deserialize;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/admin/invites", get(list_invites).post(create_invite))
        .route("/api/v1/admin/invites/{id}", delete(delete_invite))
}

#[derive(Deserialize)]
struct NewInvite {
    note: Option<String>,
    days: Option<i32>,
}

/// Creates an invite; the code is shown once and only its digest is stored.
async fn create_invite(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Json(b): Json<NewInvite>,
) -> Result<Response, AppError> {
    let code = bf_core::auth::random_token();
    let days = b.days.unwrap_or(14).clamp(1, 365);
    let id = bf_db::accounts::invites::create_invite(
        state.db()?,
        &bf_core::auth::token_digest(&code),
        who,
        b.note.as_deref(),
        days,
    )
    .await
    .db()?;
    // The code itself is a credential and stays out of the log.
    let details = json!({ "note": b.note, "days": days });
    crate::admin::api::audit_log(state.db()?, who, "invite.create", &format!("invite:{id}"), details).await?;
    let url = format!("{}/register?invite={code}", state.config.base_url);
    Ok((StatusCode::CREATED, Json(json!({ "id": id, "code": code, "url": url }))).into_response())
}

async fn list_invites(State(state): State<AppState>, Admin(..): AdminRead) -> Result<Response, AppError> {
    let rows = bf_db::accounts::invites::list_invites(state.db()?).await.db()?;
    let out: Vec<_> = rows
        .into_iter()
        .map(|(id, note, used_by, exp)| json!({ "id": id, "note": note, "used_by": used_by, "expires_at": exp }))
        .collect();
    Ok(Json(out).into_response())
}

async fn delete_invite(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    bf_db::accounts::invites::delete_invite(state.db()?, id).await.found()?;
    crate::admin::api::audit_log(state.db()?, who, "invite.delete", &format!("invite:{id}"), json!({})).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
