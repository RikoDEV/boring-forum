//! Acting on one message: edit, remove, react, pin and flag, and staff decisions on flagged ones.

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

use super::{check_length, screen, visible_message, writable};

/// Tells people watching the channel that a message changed.
pub(super) async fn changed(state: &AppState, id: i64) -> Result<Response, AppError> {
    let m = chat::message(state.db()?, id).await.db()?.ok_or(AppError::NotFound)?;
    state.hub.publish(&format!("chat:{}", m.channel_id), "changed", json!(m));
    Ok(Json(m).into_response())
}

#[derive(Deserialize)]
pub(super) struct EditBody {
    pub(super) raw: String,
}

pub(super) async fn edit(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(b): Json<EditBody>,
) -> Result<Response, AppError> {
    v.check_csrf_headers(&headers)?;
    let (m, c) = visible_message(&state, &v, id).await?;
    writable(&v, &c)?;
    if m.deleted || m.user_id != Some((v.require_user()?.id).get()) {
        return Err(AppError::Forbidden);
    }
    check_length(&state, &b.raw)?;
    let (raw, cooked, held) = screen(&state, &v, &c, b.raw.trim(), false).await?;
    if held {
        let user = v.require_user()?;
        crate::moderation::held::hold(&state, user.id.get(), "chat_edit", None, json!({ "message": id, "raw": raw }))
            .await?;
        return Ok(crate::moderation::held::held_json());
    }
    chat::edit(state.db()?, id, &raw, &cooked.html).await.db()?;
    changed(&state, id).await
}

pub(super) async fn remove(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    v.check_csrf_headers(&headers)?;
    let (m, _) = visible_message(&state, &v, id).await?;
    let user = v.require_user()?;
    if m.user_id != Some(user.id.get()) && !is_staff(&v) {
        return Err(AppError::Forbidden);
    }
    chat::delete(state.db()?, id, user.id.get()).await.db()?;
    changed(&state, id).await
}

#[derive(Deserialize)]
pub(super) struct ReactBody {
    pub(super) emoji: String,
}

pub(super) async fn react(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(b): Json<ReactBody>,
) -> Result<Response, AppError> {
    v.check_csrf_headers(&headers)?;
    let (m, c) = visible_message(&state, &v, id).await?;
    writable(&v, &c)?;
    crate::moderation::gate::account_ready(&state, v.require_user()?).map_err(AppError::Validation)?;
    // A cap of its own, apart from the reactions on posts.
    crate::http::ratelimit::REACTIONS_BY_USER.enforce(&format!("chat-react:{}", v.require_user()?.id))?;
    let emoji = b.emoji.trim();
    // One emoji: short, and no text a member could use to write messages in reactions.
    if m.deleted
        || emoji.is_empty()
        || emoji.chars().count() > 8
        || (emoji.chars().any(char::is_alphanumeric) && !emoji.contains('\u{20e3}'))
    {
        return Err(AppError::Validation("Choose an emoji.".into()));
    }
    chat::toggle_reaction(state.db()?, id, (v.require_user()?.id).get(), emoji).await.db()?;
    changed(&state, id).await
}

pub(super) async fn pin(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    v.check_csrf_headers(&headers)?;
    visible_message(&state, &v, id).await?;
    if !is_staff(&v) {
        return Err(AppError::Forbidden);
    }
    chat::toggle_pin(state.db()?, id).await.db()?;
    changed(&state, id).await
}

#[derive(Deserialize)]
pub(super) struct FlagBody {
    #[serde(default)]
    pub(super) reason: String,
}

pub(super) async fn flag(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(b): Json<FlagBody>,
) -> Result<Response, AppError> {
    v.check_csrf_headers(&headers)?;
    let (m, _) = visible_message(&state, &v, id).await?;
    let user = v.require_user()?;
    if m.deleted || m.user_id == Some(user.id.get()) {
        return Err(AppError::Validation("You cannot flag that message.".into()));
    }
    crate::http::ratelimit::REPORTS_BY_USER.enforce(&format!("report:{}", user.id))?;
    let reason: String = b.reason.trim().chars().take(500).collect();
    let pool = state.db()?;
    let count = chat::flag(pool, id, user.id.get(), &reason).await.db()?;
    let reviewers = bf_db::community::notifications::reviewers(pool, 0).await.db()?;
    let data = json!({ "url": "/review", "group": "chat-flags", "count": 1 });
    crate::community::notifications::send(&state, user.id.get(), &reviewers, "review_queue", &data).await;
    if count >= state.settings().int("flag_hide_score") {
        hide_flagged(&state, &m, &reviewers).await?;
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Enough members flagged it: out of view until staff decide. Staff's own messages stay, and a
/// member with several hidden in a day is silenced.
pub(super) async fn hide_flagged(state: &AppState, m: &chat::Message, staff: &[i64]) -> Result<(), AppError> {
    let pool = state.db()?;
    let Some(author) = m.user_id else { return Ok(()) };
    if staff.contains(&author) {
        return Ok(());
    }
    chat::auto_hide(pool, m.id).await.db()?;
    changed(state, m.id).await?;
    let title = "chat";
    let data = json!({ "title": title, "url": format!("/chat/{}", m.channel_id) });
    crate::community::notifications::send(state, 0, &[author], "post_hidden", &data).await;
    let limit = state.settings().int("chat_auto_silence_hidden");
    if limit > 0 && chat::hidden_today(pool, author).await.db()? >= limit {
        chat::auto_silence(pool, author, state.settings().int("chat_silence_hours") as i32).await.db()?;
        state.sessions.invalidate_all();
        crate::community::notifications::send(state, 0, &[author], "silenced", &json!({ "url": "/settings" })).await;
    }
    Ok(())
}

#[derive(Deserialize)]
pub(super) struct DecideForm {
    pub(super) csrf: String,
    pub(super) decision: String,
}

/// From the review queue, for site-wide staff.
pub(super) async fn staff_decide(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    axum::Form(f): axum::Form<DecideForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    if !is_staff(&v) {
        return Err(AppError::Forbidden);
    }
    if !chat::decide_flags(state.db()?, id, f.decision == "keep", (v.require_user()?.id).get()).await.db()? {
        return Err(AppError::NotFound);
    }
    changed(&state, id).await?;
    let done = if f.decision == "keep" { "chat_keep" } else { "chat_remove" };
    Ok(axum::response::Redirect::to(&format!("/review?done={done}")).into_response())
}
