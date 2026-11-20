//! Likes and reactions.

use crate::http::render::DbResult;
use crate::http::viewer::{CsrfForm, Viewer};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post, put};
use axum::{Form, Json, Router};
use bf_core::AppError;
use bf_db::forum::reactions;
use serde::Deserialize;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/post/{id}/like", post(like))
        .route("/post/{id}/react", post(react))
        .route("/api/v1/posts/{id}/reactions", get(who_reacted))
        .route("/api/v1/admin/reactions", get(admin_reactions).post(create_reaction))
        .route("/api/v1/admin/reactions/order", put(reorder_reactions))
        .route("/api/v1/admin/reactions/{id}", put(update_reaction).delete(delete_reaction))
}

/// The limits every reaction write obeys: a change only inside the window, a new one only while
/// the day's allowance lasts. `adding` is true when this write creates a reaction.
async fn check_reaction_limits(
    state: &AppState,
    pool: &sqlx::PgPool,
    post_id: i64,
    user_id: i64,
    adding: bool,
) -> Result<(), AppError> {
    let s = state.settings();
    match reactions::reacted_at(pool, post_id, user_id).await.db()? {
        Some(at) => {
            let window = s.int("reaction_change_minutes");
            if window > 0 && (chrono::Utc::now() - at).num_minutes() >= window {
                return Err(AppError::Validation("It is too late to change this reaction.".into()));
            }
        }
        None if adding && reactions::reactions_today(pool, user_id).await.db()? >= s.int("reactions_per_day") => {
            return Err(AppError::Validation("You have used all your reactions for today.".into()));
        }
        None => {}
    }
    Ok(())
}

/// Toggles a like. Returns JSON for scripts and a redirect for plain forms.
async fn like(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    if v.actor.silenced {
        return Err(AppError::Forbidden);
    }
    crate::http::ratelimit::REACTIONS_BY_USER.enforce(&format!("react:{}", user.id))?;
    // Verified email, 2FA where required and current terms, as for every other write.
    crate::moderation::gate::account_ready(&state, user).map_err(AppError::Validation)?;
    let pool = state.db()?;
    let p = bf_db::forum::posts::post_ref(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    let (t, _, perms) = crate::forum::topics::readable_topic(&state, &v, p.item_id).await?;
    crate::forum::topics::ensure_post_visible(&p, perms, &v)?;
    if p.author_id == Some(user.id.get()) {
        return Err(AppError::Validation("You cannot like your own post.".into()));
    }
    // A like is a reaction: the same daily limit and change window apply.
    let adding = reactions::reacted_at(pool, id, user.id.get()).await.db()?.is_none();
    check_reaction_limits(&state, pool, id, user.id.get(), adding).await?;
    let (liked, count) =
        reactions::toggle_like_capped(pool, id, user.id.get(), Some(state.settings().int("reactions_per_day")))
            .await
            .db()?
            .ok_or_else(|| AppError::Validation("You have used all your reactions for today.".into()))?;
    crate::federation::reaction_changed(&state, id, user.id.get(), liked).await;
    crate::platform::webhooks::fire(
        &state,
        "reaction.changed",
        json!({ "user_id": user.id.get(), "post_id": id, "reaction": "like", "on": liked }),
    );
    if let (true, Some(author)) = (liked, p.author_id) {
        let url = crate::forum::topics::post_url(&state, t.id, &t.slug, p.post_number);
        if bf_db::community::notifications::like(pool, author, &user.username, user.id.get(), id, &t.title, &url)
            .await
            .db()?
        {
            state.hub.publish(&format!("user:{author}"), "notification", json!({ "kind": "like" }));
        }
    }
    state.microcache.clear();
    let wants_json =
        headers.get("accept").and_then(|h| h.to_str().ok()).is_some_and(|a| a.contains("application/json"));
    if wants_json {
        let mut body = standing(pool, id, user.id.get()).await?;
        body["liked"] = json!(liked);
        body["count"] = json!(count);
        return Ok(Json(body).into_response());
    }
    Ok(Redirect::to(&crate::forum::topics::post_url(&state, t.id, &t.slug, p.post_number)).into_response())
}

/// How the post's reactions stand after a change, for the page to redraw them without a reload:
/// `{"pills": [{"icon", "count", "mine"}], "mine": <the member's reaction id, or null>}`.
async fn standing(pool: &sqlx::PgPool, post: i64, user: i64) -> Result<serde_json::Value, AppError> {
    let (counts, mine) = reactions::summary(pool, post, user).await.db()?;
    let pills: Vec<serde_json::Value> =
        counts.into_iter().map(|(id, icon, n)| json!({ "icon": icon, "count": n, "mine": Some(id) == mine })).collect();
    Ok(json!({ "pills": pills, "mine": mine }))
}

#[derive(Deserialize)]
pub(crate) struct ReactForm {
    csrf: String,
    #[serde(default)]
    reaction: i64,
}

/// One reaction per member per post; 0 removes it. It can be changed only within a window.
pub(crate) async fn react(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(f): Form<ReactForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    if v.actor.silenced {
        return Err(AppError::Forbidden);
    }
    // The same per-minute cap as a like (the JSON API calls this route).
    crate::http::ratelimit::REACTIONS_BY_USER.enforce(&format!("react:{}", user.id))?;
    // Verified email, 2FA where required and current terms, as for every other write.
    crate::moderation::gate::account_ready(&state, user).map_err(AppError::Validation)?;
    let pool = state.db()?;
    let p = bf_db::forum::posts::post_ref(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    let (t, _, perms) = crate::forum::topics::readable_topic(&state, &v, p.item_id).await?;
    crate::forum::topics::ensure_post_visible(&p, perms, &v)?;
    if p.author_id == Some(user.id.get()) {
        return Err(AppError::Validation("You cannot react to your own post.".into()));
    }
    let choice = (f.reaction != 0).then_some(f.reaction);
    if let Some(r) = choice
        && !bf_web::theme::reactions().iter().any(|x| x.0 == r)
    {
        return Err(AppError::Validation("That reaction is not available.".into()));
    }
    let earlier = reactions::reacted_at(pool, id, user.id.get()).await.db()?;
    check_reaction_limits(&state, pool, id, user.id.get(), earlier.is_none() && choice.is_some()).await?;
    reactions::set_reaction_capped(pool, id, user.id.get(), choice, Some(state.settings().int("reactions_per_day")))
        .await
        .db()?
        .ok_or_else(|| AppError::Validation("You have used all your reactions for today.".into()))?;
    crate::federation::reaction_changed(&state, id, user.id.get(), choice.is_some()).await;
    crate::platform::webhooks::fire(
        &state,
        "reaction.changed",
        json!({ "user_id": user.id.get(), "post_id": id, "reaction": choice, "on": choice.is_some() }),
    );
    let url = crate::forum::topics::post_url(&state, t.id, &t.slug, p.post_number);
    // Only a reaction that counts for the author (a positive score) is a "like" notification.
    let positive = match choice {
        Some(r) => reactions::reactions(pool).await.db()?.iter().any(|x| x.id == r && x.score > 0),
        None => false,
    };
    if let (true, true, Some(author)) = (earlier.is_none(), positive, p.author_id)
        && bf_db::community::notifications::like(pool, author, &user.username, user.id.get(), id, &t.title, &url)
            .await
            .db()?
    {
        state.hub.publish(&format!("user:{author}"), "notification", json!({ "kind": "like" }));
    }
    state.microcache.clear();
    let wants_json =
        headers.get("accept").and_then(|h| h.to_str().ok()).is_some_and(|a| a.contains("application/json"));
    if wants_json {
        return Ok(Json(standing(pool, id, user.id.get()).await?).into_response());
    }
    Ok(Redirect::to(&url).into_response())
}

/// `GET /api/v1/posts/{id}/reactions`: who reacted to a post, for the dialog under it.
/// `{"groups": [{"id", "icon", "name", "users": [{"name", "at"}]}]}`, the reactions in the order the
/// site lists them, the newest member first in each.
async fn who_reacted(State(state): State<AppState>, v: Viewer, Path(id): Path<i64>) -> Result<Response, AppError> {
    let pool = state.db()?;
    let p = bf_db::forum::posts::post_ref(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    let (_, _, perms) = crate::forum::topics::readable_topic(&state, &v, p.item_id).await?;
    crate::forum::topics::ensure_post_visible(&p, perms, &v)?;
    let mut groups: Vec<serde_json::Value> = Vec::new();
    let mut last = None;
    for r in reactions::who_reacted(pool, id).await.db()? {
        if last != Some(r.reaction_id) {
            groups.push(json!({ "id": r.reaction_id, "icon": r.icon, "name": r.name, "users": [] }));
            last = Some(r.reaction_id);
        }
        if let Some(users) = groups.last_mut().and_then(|g| g["users"].as_array_mut()) {
            users.push(json!({ "name": r.username, "at": r.at.to_rfc3339() }));
        }
    }
    Ok(Json(json!({ "groups": groups })).into_response())
}

/// Keeps the enabled reactions in memory for topic pages.
pub(crate) async fn load_reactions(state: &AppState) -> Result<(), AppError> {
    let rows = reactions::reactions(state.db()?).await.db()?;
    bf_web::theme::set_reactions(rows.into_iter().filter(|r| r.enabled).map(|r| (r.id, r.name, r.icon)).collect());
    state.microcache.clear();
    Ok(())
}

fn check_reaction(r: &reactions::Reaction) -> Result<(), AppError> {
    let good = !r.name.trim().is_empty()
        && r.name.chars().count() <= 40
        && !r.icon.trim().is_empty()
        && r.icon.chars().count() <= 8
        && (-1..=1).contains(&r.score);
    if good {
        Ok(())
    } else {
        Err(AppError::Validation("A reaction needs a name, a short icon and a score of -1, 0 or 1.".into()))
    }
}

async fn admin_reactions(State(state): State<AppState>, v: Viewer, headers: HeaderMap) -> Result<Response, AppError> {
    crate::admin::api::admin(&state, &v, &headers, false)?;
    Ok(Json(reactions::reactions(state.db()?).await.db()?).into_response())
}

async fn create_reaction(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Json(b): Json<reactions::Reaction>,
) -> Result<Response, AppError> {
    crate::admin::api::admin(&state, &v, &headers, true)?;
    check_reaction(&b)?;
    let id = reactions::create_reaction(state.db()?, &b).await.db()?;
    load_reactions(&state).await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))).into_response())
}

#[derive(serde::Deserialize)]
struct Order {
    ids: Vec<i64>,
}

/// Body: `{"ids": [..]}`, the reactions in the order wanted.
async fn reorder_reactions(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Json(b): Json<Order>,
) -> Result<Response, AppError> {
    crate::admin::api::admin(&state, &v, &headers, true)?;
    reactions::reorder_reactions(state.db()?, &b.ids).await.db()?;
    load_reactions(&state).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn delete_reaction(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    use reactions::ReactionDelete;
    crate::admin::api::admin(&state, &v, &headers, true)?;
    match reactions::delete_reaction(state.db()?, id).await.db()? {
        ReactionDelete::Deleted => {}
        ReactionDelete::NotFound => return Err(AppError::NotFound),
        ReactionDelete::IsLike => {
            return Err(AppError::Validation(
                "The like button is built on this reaction. Switch it off instead.".into(),
            ));
        }
        ReactionDelete::FromPlugin => {
            return Err(AppError::Validation(
                "A plugin's reactions belong to the plugin: remove the plugin to remove them.".into(),
            ));
        }
    }
    load_reactions(&state).await?;
    state.microcache.clear();
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn update_reaction(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(b): Json<reactions::Reaction>,
) -> Result<Response, AppError> {
    crate::admin::api::admin(&state, &v, &headers, true)?;
    check_reaction(&b)?;
    if !reactions::update_reaction(state.db()?, id, &b).await.db()? {
        return Err(AppError::NotFound);
    }
    load_reactions(&state).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
