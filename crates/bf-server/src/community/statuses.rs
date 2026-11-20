//! Status updates on member profiles. A member posts to their own profile; other members
//! reply. The owner and staff can delete or lock; a reply's author can delete it too.

use crate::http::render::DbResult;
use crate::http::viewer::{CsrfForm, Viewer, is_staff};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::post;
use axum::{Form, Router};
use bf_core::AppError;
use bf_core::slug::profile_path;
use bf_db::community::statuses;
use serde::Deserialize;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/u/{id}/status", post(create))
        .route("/status/{id}/reply", post(reply))
        .route("/status/{id}/delete", post(delete))
        .route("/status/{id}/lock", post(lock))
        .route("/status-reply/{id}/delete", post(delete_reply))
}

const MAX_LEN: usize = 500;

#[derive(Deserialize)]
struct BodyForm {
    csrf: String,
    #[serde(default)]
    body: String,
}

fn clean(body: &str) -> Result<&str, AppError> {
    let body = body.trim();
    if body.is_empty() || body.chars().count() > MAX_LEN {
        return Err(AppError::invalid(
            "A status or reply must be 1 to {MAX_LEN} characters.",
            &[("MAX_LEN", &MAX_LEN)],
        ));
    }
    Ok(body)
}

/// The feed of the member with this id, at one status when given.
async fn back(state: &AppState, owner: i64, status: Option<i64>) -> Result<Response, AppError> {
    let p =
        bf_db::accounts::users::profile(state.db()?, owner.into()).await.db()?.ok_or(AppError::NotFound)?;
    state.microcache.clear();
    let at = status.map_or(String::new(), |s| format!("#status-{s}"));
    Ok(Redirect::to(&format!("{}?tab=status{at}", profile_path(owner, &p.username))).into_response())
}

/// A status can be managed by its owner and by staff.
async fn manageable(state: &AppState, v: &Viewer, id: i64) -> Result<statuses::Status, AppError> {
    let user = v.require_user()?;
    let s = statuses::get(state.db()?, id).await.db()?.ok_or(AppError::NotFound)?;
    if s.user_id != user.id && !is_staff(v) {
        return Err(AppError::Forbidden);
    }
    Ok(s)
}

async fn create(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<BodyForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    if user.id != id || v.actor.silenced {
        return Err(AppError::Forbidden);
    }
    let (body, _) = crate::moderation::gate::check_post(&state, &v, clean(&f.body)?).await?;
    if body.held {
        crate::moderation::held::hold(&state, id, "status", None, serde_json::json!({ "body": body.text })).await?;
        return Ok(crate::moderation::held::held_page(&state, &v));
    }
    let new = statuses::create(state.db()?, id, clean(&body.text)?).await.db()?;
    back(&state, id, Some(new)).await
}

async fn reply(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<BodyForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    if v.actor.silenced {
        return Err(AppError::Forbidden);
    }
    crate::moderation::gate::account_ready(&state, user).map_err(AppError::Validation)?;
    let pool = state.db()?;
    let s = statuses::get(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    check_reply(&state, &s, user.id.get(), is_staff(&v)).await?;
    if !is_staff(&v) {
        // Replies wait like posts do.
        let flood = state.settings().int("flood_wait_seconds");
        if flood > 0
            && let Some(since) = statuses::seconds_since_last_reply(pool, user.id.get()).await.db()?
            && since < flood
        {
            return Err(AppError::invalid("Please wait {n} more seconds.", &[("n", &(flood - since))]));
        }
    }
    let (body, _) = crate::moderation::gate::check_post(&state, &v, clean(&f.body)?).await?;
    if body.held {
        let payload = serde_json::json!({ "status": id, "body": body.text });
        crate::moderation::held::hold(&state, user.id.get(), "status_reply", None, payload).await?;
        return Ok(crate::moderation::held::held_page(&state, &v));
    }
    deliver_reply(&state, id, (user.id.get(), &user.username), clean(&body.text)?).await?;
    back(&state, s.user_id, Some(id)).await
}

/// Whether `replier` may write under this status now: the profile is visible to them, the status
/// is not locked and its owner does not ignore them. Staff are exempt, as on the profile page.
async fn check_reply(state: &AppState, s: &statuses::Status, replier: i64, staff: bool) -> Result<(), AppError> {
    let pool = state.db()?;
    let owner =
        bf_db::accounts::users::profile(pool, s.user_id.into()).await.db()?.ok_or(AppError::NotFound)?;
    // The same rule as the profile page, which is the only place a status shows.
    let visible = match owner.profile_visibility.as_str() {
        "nobody" => replier == s.user_id || staff,
        _ => true,
    };
    if !visible {
        return Err(AppError::Forbidden);
    }
    if s.locked && replier != s.user_id && !staff {
        return Err(AppError::Validation("This status is locked.".into()));
    }
    // Someone the owner ignores does not write on their profile.
    if !staff && bf_db::community::follows::ignored_ids(pool, s.user_id).await.db()?.contains(&replier) {
        return Err(AppError::Validation("You cannot reply to this status.".into()));
    }
    Ok(())
}

/// Adds a reply to a status. Also used when a held reply is approved, so the rules that applied
/// when it was written are checked again here.
pub(crate) async fn deliver_reply(state: &AppState, id: i64, from: (i64, &str), body: &str) -> Result<(), AppError> {
    let pool = state.db()?;
    let s = statuses::get(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    let replier = bf_db::accounts::users::user_by_id(pool, from.0.into()).await.db()?.ok_or(AppError::Forbidden)?;
    let staff = replier.is_admin || replier.groups.contains(&bf_core::ids::GroupId::MODERATORS);
    check_reply(state, &s, from.0, staff).await?;
    let owner =
        bf_db::accounts::users::profile(pool, s.user_id.into()).await.db()?.ok_or(AppError::NotFound)?;
    let url = format!("{}?tab=status#status-{id}", profile_path(s.user_id, &owner.username));
    statuses::reply(pool, &s, from.0, body).await.db()?;
    // Through the sender every other notification uses: the owner's preference for this kind, their
    // ignore list, suspension and email settings all apply (it also skips telling someone about
    // their own reply).
    crate::community::notifications::send(
        state,
        from.0,
        &[s.user_id],
        "status_reply",
        &serde_json::json!({ "by": from.1, "url": url }),
    )
    .await;
    Ok(())
}

async fn delete(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let s = manageable(&state, &v, id).await?;
    statuses::delete(state.db()?, id).await.db()?;
    back(&state, s.user_id, None).await
}

async fn lock(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let s = manageable(&state, &v, id).await?;
    statuses::set_locked(state.db()?, id, !s.locked).await.db()?;
    back(&state, s.user_id, Some(id)).await
}

async fn delete_reply(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let pool = state.db()?;
    let (author, owner, status) = statuses::reply_owner(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    if user.id != author && user.id != owner && !is_staff(&v) {
        return Err(AppError::Forbidden);
    }
    statuses::delete_reply(pool, id).await.db()?;
    back(&state, owner, Some(status)).await
}
