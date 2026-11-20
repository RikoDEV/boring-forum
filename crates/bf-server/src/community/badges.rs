//! First-party features: badges and points, polls, assignment, calendar events, and two
//! compiled-in plugins that use the same hooks as WASM ones: chat webhooks and Akismet.

use crate::admin::api::admin;
use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use bf_core::AppError;
use bf_db::community::badges;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/u/{id}/badges", get(member_badges))
        .route("/api/v1/admin/badges", get(list_badges).post(create_badge))
        .route("/api/v1/admin/badges/{id}", put(update_badge).delete(delete_badge))
}

/// Now a tab of the profile; old links keep working.
async fn member_badges(Path(id): Path<i64>) -> Response {
    Redirect::permanent(&format!("/profile/{id}-x/?tab=badges")).into_response()
}

fn check_badge(b: &badges::Badge) -> Result<(), AppError> {
    // A badge that comes with the forum has its condition in the code.
    let any = b.slug.is_some() || b.min_posts > 0 || b.min_reputation > 0 || b.min_solved > 0;
    if b.name.trim().is_empty() || b.name.chars().count() > 60 || !any || b.points < 0 {
        return Err(AppError::Validation("A badge needs a name and at least one condition.".into()));
    }
    let plain_icon = !b.icon.is_empty()
        && b.icon.len() <= 40
        && b.icon.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-');
    if !matches!(b.tier.as_str(), "bronze" | "silver" | "gold")
        || !plain_icon
        || !(0..=9).contains(&b.color)
        || b.level < 0
    {
        return Err(AppError::Validation("Choose a medal shape, colour and icon from the lists.".into()));
    }
    if b.category.trim().is_empty() || b.category.chars().count() > 40 {
        return Err(AppError::Validation("Give the badge a group of up to 40 characters.".into()));
    }
    Ok(())
}

async fn list_badges(State(state): State<AppState>, v: Viewer, headers: HeaderMap) -> Result<Response, AppError> {
    admin(&state, &v, &headers, false)?;
    Ok(Json(badges::badges(state.db()?).await.db()?).into_response())
}

async fn create_badge(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Json(b): Json<badges::Badge>,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, true)?;
    // Only the forum's own badges have their condition in the code.
    let b = badges::Badge { slug: None, ..b };
    check_badge(&b)?;
    let id = badges::save_badge(state.db()?, 0, &b).await.db()?;
    crate::admin::text_overrides::refresh(&state).await;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))).into_response())
}

async fn update_badge(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(b): Json<badges::Badge>,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, true)?;
    check_badge(&b)?;
    if id <= 0 || badges::save_badge(state.db()?, id, &b).await.db()?.is_none() {
        return Err(AppError::NotFound);
    }
    crate::admin::text_overrides::refresh(&state).await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn delete_badge(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, true)?;
    if !badges::delete_badge(state.db()?, id).await.db()? {
        return Err(AppError::Validation(
            "A plugin's badges belong to the plugin: remove the plugin to remove them.".into(),
        ));
    }
    crate::admin::text_overrides::refresh(&state).await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Runs from the maintenance tick.
pub(crate) async fn run_awards(state: &AppState) {
    let Some(pool) = &state.pool else { return };
    let system = badges::award_system_badges(pool).await;
    let custom = badges::award_badges(pool).await;
    for result in [system, custom] {
        match result {
            Ok(awards) => {
                for (user, name) in awards {
                    let data = json!({ "name": name, "url": format!("/u/{user}/badges") });
                    crate::community::notifications::send(state, 0, &[user], "badge", &data).await;
                    crate::platform::webhooks::fire(state, "badge.awarded", json!({ "user_id": user, "badge": name }));
                }
            }
            Err(e) => tracing::warn!(error = %e, "badge awards failed"),
        }
    }
}
