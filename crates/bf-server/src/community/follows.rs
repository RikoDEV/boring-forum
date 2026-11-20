//! Following members, topics and forums, and ignoring members.

use crate::http::render::{ago, DbResult};
use crate::http::ui::simple_page;
use crate::http::viewer::{CsrfForm, Viewer};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use bf_core::AppError;
use bf_core::html::esc;
use bf_core::slug::topic_path;
use bf_db::community::follows;
use bf_db::community::follows::Level;
use serde::Deserialize;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/u/{id}/follow", post(follow_user))
        .route("/settings/hide-followers", post(toggle_hide_followers))
        .route("/following", get(following))
        .route("/t/{id}/follow", post(follow_topic))
        .route("/c/{id}/follow", post(follow_forum))
        .route("/u/{id}/ignore", post(ignore))
}

#[derive(Deserialize)]
struct FollowForm {
    csrf: String,
    level: String,
}

async fn follow(state: &AppState, v: &Viewer, target: &str, id: i64, f: FollowForm) -> Result<(), AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let level = Level::parse(&f.level).ok_or_else(|| AppError::BadRequest("unknown level".into()))?;
    follows::set_follow(state.db()?, user.id.get(), target, id, level).await.db()?;
    let kind = if target == "item" { "topic" } else { "forum" };
    crate::platform::webhooks::fire(
        state,
        "follow.changed",
        serde_json::json!({ "user_id": user.id.get(), "target_type": kind, "target_id": id, "level": f.level }),
    );
    Ok(())
}

async fn follow_topic(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<FollowForm>,
) -> Result<Response, AppError> {
    let (t, _, _) = crate::forum::topics::readable_topic(&state, &v, id).await?;
    follow(&state, &v, "item", id, f).await?;
    Ok(Redirect::to(&bf_core::slug::topic_path(id, &t.slug)).into_response())
}

async fn follow_forum(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<FollowForm>,
) -> Result<Response, AppError> {
    let tree = state.forum.get(state.db()?).await?;
    let node = tree
        .node(id)
        .filter(|_| tree.perms(&v.actor, id).has(bf_core::perms::Perms::READ))
        .ok_or(AppError::NotFound)?;
    let url = bf_core::slug::forum_path(id, &node.slug);
    follow(&state, &v, "node", id, f).await?;
    Ok(Redirect::to(&url).into_response())
}

#[derive(Deserialize)]
struct IgnoreForm {
    csrf: String,
    on: String,
}

async fn ignore(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<IgnoreForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    if user.id == id {
        return Err(AppError::Validation("You cannot ignore yourself.".into()));
    }
    let pool = state.db()?;
    let p = bf_db::accounts::users::profile(pool, id.into()).await.db()?.ok_or(AppError::NotFound)?;
    follows::set_ignored(pool, user.id.get(), id, f.on == "1").await.db()?;
    Ok(Redirect::to(&bf_core::slug::profile_path(id, &p.username)).into_response())
}

async fn follow_user(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    if id == user.id {
        return Err(AppError::Validation("You cannot follow yourself.".into()));
    }
    let pool = state.db()?;
    let target =
        bf_db::accounts::users::user_by_id(pool, id.into()).await.db()?.ok_or(AppError::NotFound)?;
    let on = follows::toggle_follow_user(pool, user.id.get(), id).await.db()?;
    crate::platform::webhooks::fire(
        &state,
        "follow.changed",
        serde_json::json!({ "user_id": user.id.get(), "target_type": "user", "target_id": id, "on": on }),
    );
    if on {
        // The group folds follow, unfollow, follow into one row.
        let data = serde_json::json!({ "by": user.username, "url": bf_core::slug::profile_path(user.id.get(), &user.username), "group": format!("follower:{}", user.id) });
        crate::community::notifications::send(&state, user.id.get(), &[id], "follower", &data).await;
    }
    Ok(Redirect::to(&bf_core::slug::profile_path(id, &target.username)).into_response())
}

async fn toggle_hide_followers(
    State(state): State<AppState>,
    v: Viewer,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    follows::toggle_hide_followers(state.db()?, user.id.get()).await.db()?;
    Ok(Redirect::to(&bf_core::slug::profile_path(user.id.get(), &user.username)).into_response())
}

async fn following(State(state): State<AppState>, v: Viewer) -> Result<Response, AppError> {
    let user = v.require_user()?;
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    let rows = follows::following_feed(pool, user.id.get(), &tree.readable(&v.actor)).await.db()?;
    let items: String = rows
        .iter()
        .map(|(id, slug, title, n, who, at)| {
            format!(
                "<li><a href=\"{}#post-{n}\">{}</a> <span class=\"muted small\">{} · {}</span></li>",
                esc(&topic_path(*id, slug)),
                esc(title),
                esc(who),
                ago(*at)
            )
        })
        .collect();
    let html = if items.is_empty() {
        "<p class=\"muted\">Nothing yet. Follow members from their profiles.</p>".to_owned()
    } else {
        format!("<ul class=\"activity\">{items}</ul>")
    };
    Ok(simple_page(&state, &v, &v.t().msg("Following"), &html, false))
}
