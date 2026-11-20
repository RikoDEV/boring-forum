//! Report reasons staff can edit, kept in memory so post pages and reports need no query.

use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use bf_core::AppError;
use bf_db::moderation::report_reasons::{self as db, Reason};
use std::sync::{Arc, LazyLock, RwLock};

static REASONS: LazyLock<RwLock<Arc<Vec<Reason>>>> = LazyLock::new(Default::default);

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/admin/report-reasons", get(list).post(save))
        .route("/api/v1/admin/report-reasons/{key}", put(update).delete(remove))
}

/// Every reason, enabled or not, so an old report still finds its label and weight.
pub(crate) fn all() -> Arc<Vec<Reason>> {
    REASONS.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

/// The reason a report names, or `other` when it is unknown or switched off.
pub(crate) fn find(key: &str) -> Reason {
    let all = all();
    all.iter().find(|r| r.key == key && r.enabled).or_else(|| all.iter().find(|r| r.key == "other")).cloned().unwrap_or(
        Reason {
            key: "other".into(),
            label: "Something else".into(),
            hint: String::new(),
            weight: 1.0,
            needs_note: true,
            illegal: false,
            message_user: false,
            enabled: true,
            position: 0,
            plugin: None,
        },
    )
}

pub(crate) async fn load(state: &AppState) -> Result<(), AppError> {
    let rows = db::reasons(state.db()?).await.db()?;
    bf_web::theme::set_report_reasons(
        rows.iter()
            .filter(|r| r.enabled)
            .map(|r| (r.key.clone(), r.label.clone(), r.needs_note || r.message_user))
            .collect(),
    );
    *REASONS.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(rows);
    state.microcache.clear();
    Ok(())
}

fn check(r: &Reason) -> Result<(), AppError> {
    let key_ok = !r.key.is_empty()
        && r.key.len() <= 40
        && r.key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    let label_ok = !r.label.trim().is_empty() && r.label.chars().count() <= 60 && r.hint.chars().count() <= 200;
    if !key_ok || !label_ok || !(0.0..=10.0).contains(&r.weight) {
        return Err(AppError::Validation(
            "A reason needs a key (a-z, 0-9, _), a label of up to 60 characters and a weight from 0 to 10.".into(),
        ));
    }
    if r.key == "other" && r.message_user {
        return Err(AppError::Validation("\"other\" is the fallback reason and files a normal report.".into()));
    }
    if r.key == "other" && !r.enabled {
        return Err(AppError::Validation("\"other\" is the fallback reason and stays on.".into()));
    }
    Ok(())
}

/// Report-reason changes go to the audit log with the reason before and after.
async fn audit_reason(
    state: &AppState,
    who: Option<i64>,
    what: &str,
    key: &str,
    before: Option<&Reason>,
    after: Option<&Reason>,
) {
    let snap = |r: Option<&Reason>| r.map(|r| serde_json::to_value(r).unwrap_or_default());
    crate::moderation::audit(
        state,
        who,
        &format!("report_reason.{what}"),
        format!("report_reason:{key}"),
        serde_json::json!({ "before": snap(before), "after": snap(after) }),
    )
    .await;
}

async fn list(State(state): State<AppState>, v: Viewer, headers: HeaderMap) -> Result<Response, AppError> {
    crate::admin::api::admin(&state, &v, &headers, false)?;
    Ok(Json(db::reasons(state.db()?).await.db()?).into_response())
}

async fn save(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Json(r): Json<Reason>,
) -> Result<Response, AppError> {
    let who = crate::admin::api::admin(&state, &v, &headers, true)?;
    check(&r)?;
    let before = all().iter().find(|x| x.key == r.key).cloned();
    db::save(state.db()?, &r).await.db()?;
    audit_reason(&state, who, if before.is_some() { "update" } else { "create" }, &r.key, before.as_ref(), Some(&r))
        .await;
    load(&state).await?;
    Ok(StatusCode::CREATED.into_response())
}

async fn update(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(key): Path<String>,
    Json(mut r): Json<Reason>,
) -> Result<Response, AppError> {
    let who = crate::admin::api::admin(&state, &v, &headers, true)?;
    r.key = key;
    check(&r)?;
    let before = all().iter().find(|x| x.key == r.key).cloned();
    db::save(state.db()?, &r).await.db()?;
    audit_reason(&state, who, if before.is_some() { "update" } else { "create" }, &r.key, before.as_ref(), Some(&r))
        .await;
    load(&state).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn remove(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> Result<Response, AppError> {
    let who = crate::admin::api::admin(&state, &v, &headers, true)?;
    let before = all().iter().find(|x| x.key == key).cloned();
    if !db::delete(state.db()?, &key).await.db()? {
        return Err(AppError::NotFound);
    }
    audit_reason(&state, who, "delete", &key, before.as_ref(), None).await;
    load(&state).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
