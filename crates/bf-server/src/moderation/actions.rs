//! Contact form, bulk mail, member status, signatures and saved moderator actions.

use crate::admin::api::admin;
use crate::forum::topics::readable_topic;
use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post, put};
use axum::{Form, Json, Router};
use bf_core::AppError;
use bf_core::perms::Perms;
use bf_core::slug::topic_path;
use bf_db::moderation::tools;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/assets/flags/{file}", get(flag))
        .route("/t/{id}/action/{action}", post(apply_action))
        .route("/api/v1/admin/mod-actions", get(list_actions).post(create_action))
        .route("/api/v1/admin/mod-actions/{id}", put(update_action).delete(delete_action))
}

/// A country flag, from the set compiled into the binary.
async fn flag(Path(file): Path<String>) -> Result<Response, AppError> {
    let svg = file.strip_suffix(".svg").and_then(bf_web::assets::flag).ok_or(AppError::NotFound)?;
    Ok((
        [
            (axum::http::header::CONTENT_TYPE, "image/svg+xml"),
            (axum::http::header::CACHE_CONTROL, "public, max-age=604800"),
        ],
        svg,
    )
        .into_response())
}

pub(crate) async fn load_mod_actions(state: &AppState) -> Result<(), AppError> {
    let rows = tools::mod_actions(state.db()?).await.db()?;
    bf_web::theme::set_mod_actions(rows.into_iter().map(|a| (a.id, a.name)).collect());
    Ok(())
}

fn check_steps(a: &tools::ModAction) -> Result<(), AppError> {
    let good = a.steps.iter().all(|s| match s.kind.as_str() {
        "lock" | "unlock" | "pin" | "unpin" => true,
        "reply" => s.value.as_deref().is_some_and(|t| !t.trim().is_empty() && t.chars().count() <= 10_000),
        "move" => s.value.as_deref().is_some_and(|t| t.parse::<i64>().is_ok()),
        _ => false,
    });
    if a.name.trim().is_empty() || a.steps.is_empty() || a.steps.len() > 10 || !good {
        return Err(AppError::Validation("Name the action and give 1 to 10 valid steps.".into()));
    }
    Ok(())
}

async fn list_actions(State(state): State<AppState>, v: Viewer, headers: HeaderMap) -> Result<Response, AppError> {
    admin(&state, &v, &headers, false)?;
    Ok(Json(tools::mod_actions(state.db()?).await.db()?).into_response())
}

async fn create_action(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Json(a): Json<tools::ModAction>,
) -> Result<Response, AppError> {
    let who = admin(&state, &v, &headers, true)?;
    check_steps(&a)?;
    let id = tools::save_mod_action(state.db()?, 0, &a).await.db()?;
    crate::moderation::audit(
        &state,
        who,
        "mod_action.create",
        format!("mod_action:{}", id.unwrap_or(0)),
        json!({ "before": null, "after": { "name": a.name, "steps": a.steps } }),
    )
    .await;
    load_mod_actions(&state).await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))).into_response())
}

async fn update_action(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(a): Json<tools::ModAction>,
) -> Result<Response, AppError> {
    let who = admin(&state, &v, &headers, true)?;
    check_steps(&a)?;
    let before = if id > 0 { tools::mod_action(state.db()?, id).await.db()? } else { None };
    if id <= 0 || tools::save_mod_action(state.db()?, id, &a).await.db()?.is_none() {
        return Err(AppError::NotFound);
    }
    crate::moderation::audit(
        &state,
        who,
        "mod_action.update",
        format!("mod_action:{id}"),
        json!({
            "before": before.map(|b| json!({ "name": b.name, "steps": b.steps })),
            "after": { "name": a.name, "steps": a.steps },
        }),
    )
    .await;
    load_mod_actions(&state).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn delete_action(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let who = admin(&state, &v, &headers, true)?;
    let before = tools::mod_action(state.db()?, id).await.db()?;
    if !tools::delete_mod_action(state.db()?, id).await.db()? {
        return Err(AppError::NotFound);
    }
    crate::moderation::audit(
        &state,
        who,
        "mod_action.delete",
        format!("mod_action:{id}"),
        json!({ "before": before.map(|b| json!({ "name": b.name, "steps": b.steps })), "after": null }),
    )
    .await;
    load_mod_actions(&state).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Runs a saved action on one topic. A move also needs moderator rights in the target forum.
async fn apply_action(
    State(state): State<AppState>,
    v: Viewer,
    Path((id, action)): Path<(i64, i64)>,
    Form(f): Form<crate::http::viewer::CsrfForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let (t, tree, perms) = readable_topic(&state, &v, id).await?;
    if !perms.has(Perms::MODERATE) {
        return Err(AppError::Forbidden);
    }
    let pool = state.db()?;
    let a = tools::mod_action(pool, action).await.db()?.ok_or(AppError::NotFound)?;
    for s in &a.steps {
        let value = s.value.as_deref().unwrap_or_default();
        match s.kind.as_str() {
            "lock" => tools::set_topic_flag(pool, id, "locked", true).await.db()?,
            "unlock" => tools::set_topic_flag(pool, id, "locked", false).await.db()?,
            "pin" => tools::set_topic_flag(pool, id, "pinned", true).await.db()?,
            "unpin" => tools::set_topic_flag(pool, id, "pinned", false).await.db()?,
            "move" => {
                let to: i64 = value.parse().map_err(|_| AppError::Validation("Bad forum in that action.".into()))?;
                let allowed =
                    tree.node(to).is_some_and(|n| n.kind == "forum") && tree.perms(&v.actor, to).has(Perms::MODERATE);
                if !allowed {
                    return Err(AppError::Forbidden);
                }
                bf_db::moderation::topics::move_topic(pool, id, to, user.id.get()).await.db()?;
            }
            "reply" => {
                // A canned reply is still text a plugin may filter; a refusal skips the step.
                let Some(value) = crate::platform::plugins::before_post_system(&state, user.id.get(), value).await
                else {
                    continue;
                };
                let value = value.as_str();
                let cooked = crate::platform::plugins::cook(&state, value).await;
                let post = bf_db::forum::posts::NewPost {
                    author_id: user.id.get(),
                    raw: value,
                    cooked: &cooked.html,
                    cooked_version: bf_cook::COOK_VERSION,
                    ip: None,
                    pending: false,
                };
                bf_db::forum::posts::create_reply(pool, id, t.node_id, &post).await.db()?;
            }
            _ => {}
        }
    }
    tools::log_action(pool, user.id.get(), id, &a.name).await.db()?;
    crate::moderation::topics::refresh(&state);
    Ok(Redirect::to(&topic_path(t.id, &t.slug)).into_response())
}
