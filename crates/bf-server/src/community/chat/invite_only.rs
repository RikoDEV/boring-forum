//! Invite-only channels that members start and manage.

use crate::http::render::DbResult;
use crate::http::viewer::{Viewer, is_staff};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use bf_core::AppError;
use bf_db::community::chat;
use serde::Deserialize;
use serde_json::json;

use super::{enabled, viewable, writable};

#[derive(Deserialize)]
pub(super) struct MemberChannel {
    pub(super) name: String,
    #[serde(default)]
    pub(super) description: String,
}

/// Members who have earned it can start an invite-only channel and choose who is in it.
pub(super) async fn member_channel(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Json(b): Json<MemberChannel>,
) -> Result<Response, AppError> {
    enabled(&state)?;
    v.check_csrf_headers(&headers)?;
    let user = v.require_user()?;
    if v.actor.silenced
        || (!is_staff(&v) && i64::from(user.trust_level) < state.settings().int("chat_create_min_trust_level"))
    {
        return Err(AppError::Forbidden);
    }
    let n = b.name.trim().chars().count();
    if n == 0 || n > 60 {
        return Err(AppError::Validation("A channel name is 1 to 60 characters.".into()));
    }
    crate::moderation::gate::account_ready(&state, user).map_err(AppError::Validation)?;
    let name = crate::moderation::gate::words(&state, &v, b.name.trim()).await?;
    let description = crate::moderation::gate::words(&state, &v, b.description.trim()).await?;
    if name.held || description.held {
        let payload = json!({ "name": name.text, "description": description.text });
        crate::moderation::held::hold(&state, user.id.get(), "chat_channel", None, payload).await?;
        return Ok(crate::moderation::held::held_json());
    }
    let id = chat::create_channel(state.db()?, &name.text, &description.text, "invite", &[], user.id.get())
        .await
        .db()?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))).into_response())
}

/// An invite-only channel, for its creator or site-wide staff who can already see it.
pub(super) async fn managed(state: &AppState, v: &Viewer, id: i64) -> Result<chat::Channel, AppError> {
    let c = viewable(state, v, id).await?;
    if c.kind != "public" || c.access != "invite" {
        return Err(AppError::Validation("Only invite-only channels have a member list to manage.".into()));
    }
    if c.created_by != Some((v.require_user()?.id).get()) && !is_staff(v) {
        return Err(AppError::Forbidden);
    }
    Ok(c)
}

pub(super) async fn list_members(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    managed(&state, &v, id).await?;
    let rows = chat::members(state.db()?, id).await.db()?;
    Ok(Json(rows.into_iter().map(|(id, username)| json!({ "id": id, "username": username })).collect::<Vec<_>>())
        .into_response())
}

#[derive(Deserialize)]
pub(super) struct MemberBody {
    pub(super) username: String,
}

pub(super) async fn add_member(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(b): Json<MemberBody>,
) -> Result<Response, AppError> {
    v.check_csrf_headers(&headers)?;
    let c = managed(&state, &v, id).await?;
    writable(&v, &c)?;
    let user = v.require_user()?;
    let pool = state.db()?;
    let person = crate::community::clubs::member_named(pool, &b.username).await?;
    // The "who can message me" setting keeps a member out of channels others start, too.
    if chat::ignoring(pool, &[person], user.id.get()).await.db()?
        || !bf_db::community::messages::restricting(pool, user.id.get(), &[person]).await.db()?.is_empty()
    {
        return Err(AppError::Validation("You cannot add that member.".into()));
    }
    chat::join(pool, id, person).await.db()?;
    let data = json!({ "by": user.username, "title": c.name, "url": format!("/chat/{id}") });
    crate::community::notifications::send(&state, user.id.get(), &[person], "chat_added", &data).await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub(super) async fn remove_member(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path((id, person)): Path<(i64, i64)>,
) -> Result<Response, AppError> {
    v.check_csrf_headers(&headers)?;
    let c = managed(&state, &v, id).await?;
    if c.created_by == Some(person) {
        return Err(AppError::Validation("The creator stays in the channel.".into()));
    }
    chat::leave(state.db()?, id, person).await.db()?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
