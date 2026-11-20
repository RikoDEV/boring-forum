//! Votes: answers in Q&A forums, topics in ideas forums.

use crate::forum::topics::{ensure_post_visible, readable_topic};
use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::post;
use axum::{Form, Json, Router};
use bf_core::AppError;
use bf_core::slug::topic_path;
use bf_db::accounts::users::User;
use serde::Deserialize;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new().route("/post/{id}/vote", post(vote_answer)).route("/t/{id}/vote-topic", post(vote_topic))
}

/// Members writing are members voting: the same account rules.
fn may_vote(state: &AppState, v: &Viewer) -> Result<(), AppError> {
    let user = v.require_user()?;
    if v.actor.silenced {
        return Err(AppError::Forbidden);
    }
    crate::moderation::gate::account_ready(state, user).map_err(AppError::Validation)
}

#[derive(Deserialize)]
struct AnswerVote {
    csrf: String,
    /// 1, -1, or 0 to take the vote back.
    value: i16,
}

async fn vote_answer(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<AnswerVote>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    may_vote(&state, &v)?;
    let user = v.require_user()?;
    if !(-1..=1).contains(&f.value) {
        return Err(AppError::BadRequest("a vote is 1, -1 or 0".into()));
    }
    let pool = state.db()?;
    let p = bf_db::forum::posts::post_ref(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    let (t, tree, perms) = readable_topic(&state, &v, p.item_id).await?;
    ensure_post_visible(&p, perms, &v)?;
    if tree.node(t.node_id).is_none_or(|n| n.mode != "qa") {
        return Err(AppError::NotFound);
    }
    if p.post_number == 1 || p.author_id == Some(user.id.get()) {
        return Err(AppError::Validation("You can vote on other members' answers.".into()));
    }
    if t.locked {
        return Err(AppError::Validation("This question is closed.".into()));
    }
    bf_db::forum::posts::vote_post(pool, id, user.id.get(), f.value).await.db()?;
    crate::platform::webhooks::fire(
        &state,
        "vote.cast",
        json!({ "user_id": user.id.get(), "target_type": "post", "target_id": id, "topic_id": t.id, "value": f.value }),
    );
    state.microcache.clear();
    Ok(Redirect::to(&format!("{}#post-{}", topic_path(t.id, &t.slug), p.post_number)).into_response())
}

/// How many ideas a member can back at once, by trust level; 0 is no limit.
pub(crate) fn vote_limit(state: &AppState, user: &User) -> i64 {
    let s = state.settings();
    match user.trust_level {
        ..=1 => s.int("topic_votes_per_member"),
        2 => s.int("topic_votes_tl2"),
        3 => s.int("topic_votes_tl3"),
        _ => s.int("topic_votes_tl4"),
    }
}

/// (left, limit) for a member, or None when their votes are unlimited.
pub(crate) async fn votes_left(state: &AppState, user: &User) -> Result<Option<(i64, i64)>, AppError> {
    let limit = vote_limit(state, user);
    if limit == 0 {
        return Ok(None);
    }
    let used = bf_db::forum::topics::votes_used(state.db()?, user.id.get()).await.db()?;
    Ok(Some(((limit - used).max(0), limit)))
}

#[derive(Deserialize)]
struct TopicVote {
    csrf: String,
    /// "1" to vote, "0" to take it back.
    on: String,
    /// The page to return to, when voting from a list without scripts.
    #[serde(default)]
    next: String,
}

async fn vote_topic(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(f): Form<TopicVote>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    may_vote(&state, &v)?;
    let user = v.require_user()?;
    let (t, tree, _) = readable_topic(&state, &v, id).await?;
    if tree.node(t.node_id).is_none_or(|n| n.mode != "ideas") {
        return Err(AppError::NotFound);
    }
    let on = f.on == "1";
    if on && t.locked {
        return Err(AppError::Validation("This topic is closed, so it takes no more votes.".into()));
    }
    let limit = vote_limit(&state, user);
    let Some(votes) = bf_db::forum::topics::vote_topic(state.db()?, id, user.id.get(), on, limit).await.db()? else {
        return Err(AppError::invalid(
            "You have used all {limit} of your votes. Take one back from another idea, or wait until one closes.",
            &[("limit", &limit)],
        ));
    };
    crate::platform::webhooks::fire(
        &state,
        "vote.cast",
        json!({ "user_id": user.id.get(), "target_type": "topic", "target_id": id, "topic_id": id, "value": i32::from(on) }),
    );
    state.forum.invalidate();
    state.microcache.clear();
    // The islands vote in place: the new count and what the member has left.
    if headers.get(header::ACCEPT).and_then(|a| a.to_str().ok()).is_some_and(|a| a.contains("application/json")) {
        let left = votes_left(&state, user).await?;
        return Ok(Json(json!({
            "votes": votes,
            "voted": on,
            "left": left.map(|l| l.0),
            "limit": left.map(|l| l.1),
        }))
        .into_response());
    }
    let next = if crate::platform::plugins::local_path(&f.next) { f.next.clone() } else { topic_path(t.id, &t.slug) };
    Ok(Redirect::to(&next).into_response())
}
