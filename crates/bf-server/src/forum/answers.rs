//! Solved answers, tag groups and bookmark reminders.

use crate::forum::topics::readable_topic;
use crate::http::render::DbResult;
use crate::http::viewer::{CsrfForm, Viewer};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::post;
use axum::{Form, Router};
use bf_core::AppError;
use bf_core::perms::Perms;
use bf_db::forum::answers;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new().route("/post/{id}/solve", post(solve))
}

/// The topic's author or a moderator marks a reply as the answer; doing it again clears it.
async fn solve(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    state.require_feature("solved_enabled")?;
    let pool = state.db()?;
    let p = bf_db::forum::posts::post_ref(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    let (t, _, perms) = readable_topic(&state, &v, p.item_id).await?;
    crate::forum::topics::ensure_post_visible(&p, perms, &v)?;
    if !perms.has(Perms::MODERATE) && (t.author_id != Some(user.id.get()) || v.actor.silenced) {
        return Err(AppError::Forbidden);
    }
    crate::moderation::gate::account_ready(&state, user).map_err(AppError::Validation)?;
    // A closed topic keeps its answer, as it keeps its votes; only a moderator changes it.
    if (t.locked || t.archived) && !perms.has(Perms::MODERATE) {
        return Err(AppError::Validation("This topic is closed.".into()));
    }
    if p.post_number <= 1 {
        return Err(AppError::Validation("The first post cannot be its own answer.".into()));
    }
    let already = t.solved_post_number == Some(p.post_number);
    answers::set_solved(pool, t.id, (!already).then_some(p.post_number)).await.db()?;
    let url = crate::forum::topics::post_url(&state, t.id, &t.slug, p.post_number);
    if let (false, Some(author)) = (already, p.author_id.filter(|a| *a != user.id)) {
        if !crate::community::notifications::can_read(&state, pool, t.node_id, vec![author]).await.is_empty() {
            let data = json!({ "by": user.username, "title": t.title, "url": url });
            crate::community::notifications::send(&state, user.id.get(), &[author], "solved", &data).await;
        }
    }
    state.microcache.clear();
    Ok(Redirect::to(&url).into_response())
}
