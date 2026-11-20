//! Automatic promotion rules.

use crate::admin::api::admin;
use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use bf_core::AppError;
use bf_db::community::promotion;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/admin/promotion-rules", get(list_rules).post(create_rule))
        .route("/api/v1/admin/promotion-rules/{id}", put(update_rule).delete(delete_rule))
}

fn check_rule(r: &promotion::Rule) -> Result<(), AppError> {
    let conditions = r.min_posts >= 0 && r.min_days >= 0 && r.min_reputation >= 0 && r.min_topics_read >= 0;
    let some_condition = r.min_posts > 0 || r.min_days > 0 || r.min_reputation > 0 || r.min_topics_read > 0;
    // Levels have their own requirements, in Settings under Trust and warnings.
    let level = r.set_trust_level.is_none();
    // Staff groups are never handed out by a rule.
    let group = r.add_group.is_none_or(|g| g != 3 && g != 4 && g > 0);
    let action = r.add_group.is_some();
    if r.name.trim().is_empty() || !conditions || !some_condition || !level || !group || !action {
        return Err(AppError::Validation(
            "A group rule needs a name, at least one condition and a group (not a staff group). Trust levels are set in Settings.".into(),
        ));
    }
    Ok(())
}

async fn list_rules(State(state): State<AppState>, v: Viewer, headers: HeaderMap) -> Result<Response, AppError> {
    admin(&state, &v, &headers, false)?;
    Ok(Json(promotion::rules(state.db()?).await.db()?).into_response())
}

async fn create_rule(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Json(r): Json<promotion::Rule>,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, true)?;
    check_rule(&r)?;
    let id = promotion::save_rule(state.db()?, 0, &r).await.db()?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))).into_response())
}

async fn update_rule(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(r): Json<promotion::Rule>,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, true)?;
    check_rule(&r)?;
    if id <= 0 || promotion::save_rule(state.db()?, id, &r).await.db()?.is_none() {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn delete_rule(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, true)?;
    if !promotion::delete_rule(state.db()?, id).await.db()? {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}
