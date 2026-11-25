//! Suggestions for the administrators (`bf_core::suggest`): what the rules say about this site now.
//!
//! The rules are pure; this gathers their facts (database counts, cached for a few minutes, and
//! what the process has seen) and keeps the dismissals. Applying one is the ordinary settings
//! call from the panel, so it is audited, sudo-gated and CSRF-checked like any other change.

use crate::admin::api::{Admin, AdminRead, AdminWrite};
use crate::http::ratelimit;
use crate::http::render::DbResult;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Json, Router};
use bf_core::AppError;
use bf_core::suggest::{Facts, IDS};
use bf_db::admin::suggestions::{self as store, Counts};
use serde::Deserialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/admin/suggestions", get(list))
        .route("/api/v1/admin/suggestions/{id}/dismiss", delete(restore).post(dismiss))
}

/// The counts cost a few scans, and the dashboard asks on every visit.
static COUNTS: Mutex<Option<(Instant, Counts)>> = Mutex::new(None);
const FRESH: Duration = Duration::from_secs(300);

async fn counts(pool: &sqlx::PgPool) -> Result<Counts, AppError> {
    if let Some((at, c)) = COUNTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref()
        && at.elapsed() < FRESH
    {
        return Ok(c.clone());
    }
    let c = store::counts(pool, &ratelimit::CLOUDFLARE).await.db()?;
    *COUNTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some((Instant::now(), c.clone()));
    Ok(c)
}

#[derive(Deserialize)]
struct ListQuery {
    /// List the dismissed ones instead, so they can be restored.
    #[serde(default)]
    dismissed: bool,
}

async fn list(
    State(state): State<AppState>,
    Admin(_, v): AdminRead,
    Query(q): Query<ListQuery>,
) -> Result<Response, AppError> {
    let pool = state.db()?;
    let c = counts(pool).await?;
    let facts = Facts {
        base_url: state.config.base_url.clone(),
        mail_transport: std::env::var("BF_SMTP_URL").is_ok() || std::env::var("BF_MAIL_DIR").is_ok(),
        trusted_proxies: ratelimit::has_trusted_proxies(),
        unlisted_proxy_requests: ratelimit::unlisted_proxy_requests(),
        members: c.members,
        topics: c.topics,
        admins_without_2fa: c.admins_without_2fa,
        recent_sessions: c.recent_sessions,
        cloudflare_sessions: c.sessions_in_networks,
    };
    let dismissed = store::dismissed(pool).await.db()?;
    let all = bf_core::suggest::evaluate(&state.settings(), &facts, &v.t());
    let shown: Vec<_> = all.into_iter().filter(|s| dismissed.iter().any(|d| d == s.id) == q.dismissed).collect();
    Ok(Json(shown).into_response())
}

async fn dismiss(
    State(state): State<AppState>,
    Admin(..): AdminWrite,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    if !IDS.contains(&id.as_str()) {
        return Err(AppError::NotFound);
    }
    store::dismiss(state.db()?, &id).await.db()?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn restore(
    State(state): State<AppState>,
    Admin(..): AdminWrite,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    if !IDS.contains(&id.as_str()) {
        return Err(AppError::NotFound);
    }
    store::restore(state.db()?, &id).await.db()?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
