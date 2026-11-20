//! Read-only reports: analytics, statistics, audit log, mail log, search report, bounced addresses and failed logins.

use crate::admin::api::{Admin, AdminWrite};
use crate::http::render::DbResult;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bf_core::AppError;
use bf_db::admin::{audit, dashboard};
use serde::Deserialize;
use serde_json::json;

use super::constant_time_eq;

#[derive(Deserialize)]
pub(super) struct AnalyticsQuery {
    days: Option<i32>,
}

admin_get!(analytics(state, q: AnalyticsQuery) dashboard::analytics(state.db()?, q.days.unwrap_or(30)).await.db()?);

admin_get!(stats(state) dashboard::stats(state.db()?).await.db()?);

#[derive(Deserialize)]
pub(super) struct AuditQuery {
    #[serde(default)]
    pub(super) action: String,
    #[serde(default)]
    pub(super) actor: String,
    pub(super) before: Option<i64>,
}

admin_get!(audit(state, q: AuditQuery) audit::audit_log(state.db()?, q.action.trim(), q.actor.trim(), q.before, 100).await.db()?);

admin_get!(mail_log(state) audit::mail_log(state.db()?, 200).await.db()?);

admin_get!(search_report(state) bf_db::forum::search::search_report(state.db()?)
    .await
    .db()?
    .into_iter()
    .map(|(t, n, r)| json!({ "term": t, "searches": n, "avg_results": r }))
    .collect::<Vec<_>>());

admin_get!(bad_emails(state) bf_db::platform::mail::bad_emails(state.db()?)
    .await
    .db()?
    .into_iter()
    .map(|(id, username, email, bounces)| json!({ "id": id, "username": username, "email": email, "bounces": bounces }))
    .collect::<Vec<_>>());

pub(super) async fn clear_bad_email(
    State(state): State<AppState>,
    Admin(..): AdminWrite,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    bf_db::platform::mail::clear_bad_email(state.db()?, id).await.found()?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize)]
pub(super) struct BounceQuery {
    pub(super) token: Option<String>,
}

/// `POST /api/v1/webhooks/bounce`, called by the mail provider (see `bf_core::bounce`
/// for the formats) with `Authorization: Bearer $BF_BOUNCE_TOKEN`, or `?token=` for providers
/// that cannot set headers. Disabled without a token of 32+ characters.
pub(super) async fn bounce_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<BounceQuery>,
    body: axum::body::Bytes,
) -> Result<Response, AppError> {
    let expected = std::env::var("BF_BOUNCE_TOKEN").ok().filter(|t| t.len() >= 32).ok_or(AppError::NotFound)?;
    let bearer =
        headers.get(header::AUTHORIZATION).and_then(|h| h.to_str().ok()).and_then(|h| h.strip_prefix("Bearer "));
    if !bearer.or(q.token.as_deref()).is_some_and(|g| constant_time_eq(g.as_bytes(), expected.as_bytes())) {
        return Err(AppError::Forbidden);
    }
    // SNS posts JSON as text/plain, so the body is parsed by hand.
    let value: serde_json::Value =
        serde_json::from_slice(&body).map_err(|_| AppError::BadRequest("Expected a JSON body.".into()))?;
    if let Some(url) = bf_core::bounce::sns_confirm_url(&value) {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(AppError::internal)?;
        client.get(url).send().await.and_then(reqwest::Response::error_for_status).map_err(AppError::internal)?;
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    let limit = state.settings().int("email_bounce_limit") as i32;
    let pool = state.db()?;
    // Unknown addresses get the same answer, so the hook cannot probe for members.
    for email in bf_core::bounce::permanent_addresses(&value).iter().take(100) {
        bf_db::platform::mail::record_bounce(pool, email, limit).await.db()?;
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

admin_get!(failed_logins(state) bf_db::accounts::security::failed_logins(state.db()?, 200).await.db()?);

admin_get!(traffic(state, q: AnalyticsQuery) bf_db::admin::traffic::report(state.db()?, q.days.unwrap_or(30)).await.db()?);
