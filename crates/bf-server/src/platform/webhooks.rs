//! API keys, outgoing webhooks and text overrides.

use crate::admin::api::admin;
use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::platform::outbound::forbidden_ip;
use crate::platform::outbound::loopback_allowed;
use crate::platform::outbound::valid_url;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Json, Router};
use bf_core::AppError;
use bf_db::admin::notices;
use bf_db::platform::api;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/admin/webhooks", get(list_webhooks).post(create_webhook))
        .route("/api/v1/admin/webhooks/{id}", delete(delete_webhook))
        .route("/api/v1/admin/webhooks/{id}/test", axum::routing::post(test_webhook))
        .route("/api/v1/admin/webhook-deliveries/{id}/replay", axum::routing::post(replay_delivery))
        .route("/api/v1/admin/webhook-deliveries", get(deliveries))
}

/// Events an admin can subscribe a webhook to. Every one is also a plugin hook (`plugins::HOOKS`;
/// a test keeps the two lists in step). Private events (messages, chat, logins, erasure) are
/// plugin hooks only and never leave the site through a webhook.
pub const EVENTS: [&str; 8] = [
    "topic.created",
    "post.created",
    "post.edited",
    "topic.moved",
    "user.registered",
    "topic.deleted",
    "post.deleted",
    "moderation.action",
];

const MAX_ATTEMPTS: i32 = 8;

#[derive(Deserialize)]
struct NewWebhook {
    url: String,
    events: Vec<String>,
    /// Forums the hook is limited to; empty or absent means every forum.
    #[serde(default)]
    node_ids: Vec<i64>,
}

async fn create_webhook(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Json(b): Json<NewWebhook>,
) -> Result<Response, AppError> {
    let who = admin(&state, &v, &headers, true)?;
    if valid_url(&b.url).is_none() {
        return Err(AppError::Validation("Use an https address.".into()));
    }
    if b.events.is_empty() || !b.events.iter().all(|e| e == "*" || EVENTS.contains(&e.as_str())) {
        return Err(AppError::invalid("Events must be * or from: {n}.", &[("n", &EVENTS.join(", "))]));
    }
    let secret = bf_core::auth::random_token();
    let pool = state.db()?;
    let mut nodes = b.node_ids.clone();
    nodes.sort_unstable();
    nodes.dedup();
    if api::forum_ids(pool, &nodes).await.db()? != nodes {
        return Err(AppError::Validation("Choose forums that exist.".into()));
    }
    let id = api::create_webhook(pool, &b.url, &secret, &b.events, &nodes).await.db()?;
    let scope = if nodes.is_empty() { "all forums".to_owned() } else { format!("forums {nodes:?}") };
    audit(pool, who, "webhook.create", format!("webhook:{id} ({scope})")).await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id, "secret": secret }))).into_response())
}

async fn list_webhooks(State(state): State<AppState>, v: Viewer, headers: HeaderMap) -> Result<Response, AppError> {
    admin(&state, &v, &headers, false)?;
    Ok(Json(json!({ "webhooks": api::webhooks(state.db()?).await.db()?, "events": EVENTS })).into_response())
}

async fn delete_webhook(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let who = admin(&state, &v, &headers, true)?;
    let pool = state.db()?;
    if !api::delete_webhook(pool, id).await.db()? {
        return Err(AppError::NotFound);
    }
    audit(pool, who, "webhook.delete", format!("webhook:{id}")).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn deliveries(State(state): State<AppState>, v: Viewer, headers: HeaderMap) -> Result<Response, AppError> {
    admin(&state, &v, &headers, false)?;
    Ok(Json(api::deliveries(state.db()?).await.db()?).into_response())
}

#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct TestResult {
    /// The receiver answered with a 2xx.
    ok: bool,
    /// The HTTP status the receiver answered with, if it did.
    status: Option<i32>,
    error: Option<String>,
}

async fn audit(pool: &sqlx::PgPool, actor: Option<i64>, action: &str, target: String) -> Result<(), AppError> {
    bf_db::admin::audit::record(pool, bf_db::admin::audit::Entry::new(action).by(actor.unwrap_or(0)).on(target))
        .await
        .db()
}

/// Sends one signed `webhook.test` delivery to the address now and says how the receiver answered.
/// Nothing is queued, so it does not touch the delivery log.
#[utoipa::path(post, path = "/api/v1/admin/webhooks/{id}/test", tag = "admin", security(("admin_token" = [])),
    params(("id" = i64, Path)),
    responses((status = 200, body = TestResult), (status = 403, body = bf_core::error::Problem), (status = 404, body = bf_core::error::Problem)))]
pub async fn test_webhook(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let who = admin(&state, &v, &headers, true)?;
    let pool = state.db()?;
    let (url, secret) = api::webhook_target(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    audit(pool, who, "webhook.test", format!("webhook:{id}")).await?;
    let due = api::Due {
        id: 0,
        url,
        secret,
        event: "webhook.test".into(),
        payload: json!({ "message": "A test delivery from Boring Forum." }),
        attempts: 0,
    };
    let out = match deliver(&due).await {
        Ok(status) => TestResult { ok: true, status: Some(status), error: None },
        Err((status, error)) => TestResult { ok: false, status, error: Some(error) },
    };
    Ok(Json(out).into_response())
}

/// Queues a delivery again, whatever became of it, with its attempts reset.
#[utoipa::path(post, path = "/api/v1/admin/webhook-deliveries/{id}/replay", tag = "admin", security(("admin_token" = [])),
    params(("id" = i64, Path)),
    responses((status = 204), (status = 403, body = bf_core::error::Problem), (status = 404, body = bf_core::error::Problem)))]
pub async fn replay_delivery(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let who = admin(&state, &v, &headers, true)?;
    let pool = state.db()?;
    if !api::replay_delivery(pool, id).await.db()? {
        return Err(AppError::NotFound);
    }
    audit(pool, who, "webhook.replay", format!("webhook_delivery:{id}")).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Queues an event; a failure here must never fail the action that caused it.
pub(crate) async fn emit(state: &AppState, event: &str, payload: serde_json::Value) {
    crate::platform::plugins::dispatch(state, event, &payload);
    emit_external(state, event, payload).await;
}

/// For events whose plugin hooks have already run (or must not run: a plugin's own post must not
/// start `post.created` in plugins again): webhooks and federation only.
pub(crate) async fn emit_external(state: &AppState, event: &str, payload: serde_json::Value) {
    crate::federation::on_event(state, event, &payload).await;
    if let Some(pool) = &state.pool
        && let Err(e) = api::enqueue_event(pool, event, &payload).await
    {
        tracing::warn!(error = %e, event, "webhook enqueue failed");
    }
}

/// `moderation.action`: `{action, actor, target_type, target_id, detail}`. Actions: lock, unlock,
/// pin, unpin, unlist, list, archive, unarchive, feature, unfeature, move, delete_topic,
/// delete_post, hide_post, show_post, silence, suspend, lift, spammer, ban, unban.
pub(crate) async fn moderated(
    state: &AppState,
    action: &str,
    actor: i64,
    target_type: &str,
    target_id: i64,
    detail: serde_json::Value,
) {
    let payload = json!({
        "action": action, "actor": actor, "target_type": target_type, "target_id": target_id, "detail": detail,
    });
    emit(state, "moderation.action", payload).await;
}

/// An action only plugins hear about (no webhook, no federation): logins, chat, messages, votes.
#[expect(clippy::needless_pass_by_value, reason = "callers build the payload in place with json!")]
pub(crate) fn fire(state: &AppState, hook: &str, payload: serde_json::Value) {
    crate::platform::plugins::dispatch(state, hook, &payload);
}

/// One attempt. Resolves the host itself and pins the checked address for the request, so
/// DNS cannot point it at an internal address between the check and the connection.
async fn deliver(d: &api::Due) -> Result<i32, (Option<i32>, String)> {
    let fail = |e: String| (None, e);
    let u = valid_url(&d.url).ok_or_else(|| fail("invalid address".into()))?;
    let host = u.host_str().ok_or_else(|| fail("no host".into()))?.to_owned();
    let port = u.port_or_known_default().unwrap_or(443);
    let addrs: Vec<_> =
        tokio::net::lookup_host((host.as_str(), port)).await.map_err(|e| fail(e.to_string()))?.collect();
    let addr = addrs
        .into_iter()
        .find(|a| loopback_allowed() || !forbidden_ip(a.ip()))
        .ok_or_else(|| fail("address not allowed".into()))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .resolve(&host, addr)
        .user_agent("boring-forum-webhooks")
        .build()
        .map_err(|e| fail(e.to_string()))?;
    let body = serde_json::to_vec(&json!({ "event": d.event, "delivery": d.id, "data": d.payload }))
        .map_err(|e| fail(e.to_string()))?;
    // Signed with the send time inside the MAC, so a captured body cannot be replayed later
    //. Receivers should refuse a timestamp more than ~5 minutes from their own clock.
    let now = chrono::Utc::now().timestamp();
    let res = client
        .post(u)
        .header("content-type", "application/json")
        .header("x-bf-event", &d.event)
        .header("x-bf-delivery", d.id.to_string())
        .header("x-bf-timestamp", now.to_string())
        .header("x-bf-signature", bf_core::auth::webhook_signature(&d.secret, now, &body))
        .body(body)
        .send()
        .await
        .map_err(|e| fail(e.without_url().to_string()))?;
    let status = i32::from(res.status().as_u16());
    if res.status().is_success() { Ok(status) } else { Err((Some(status), format!("status {status}"))) }
}

/// Retries with exponential backoff: 2, 4, 8 … minutes, then gives up.
pub fn spawn_webhooks(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(bf_core::scaled(Duration::from_secs(2)));
        loop {
            tick.tick().await;
            let Some(pool) = &state.pool else { return };
            let due = match api::due_deliveries(pool, 20).await {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!(error = %e, "webhook queue read failed");
                    continue;
                }
            };
            for d in due {
                let saved = match deliver(&d).await {
                    Ok(status) => api::delivery_done(pool, d.id, status).await,
                    Err((status, error)) => {
                        let attempt = d.attempts + 1;
                        let retry = (attempt < MAX_ATTEMPTS).then(|| 60 * 2_i32.pow(attempt as u32));
                        if retry.is_none() {
                            let text = format!("A webhook to {} keeps failing", d.url);
                            if let Err(e) = notices::add_notice(pool, "webhook", &text).await {
                                tracing::warn!(error = %e, "notice failed");
                            }
                        }
                        api::delivery_failed(pool, d.id, status, &error, retry).await
                    }
                };
                if let Err(e) = saved {
                    tracing::warn!(error = %e, "webhook status update failed");
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_webhook_event_is_a_plugin_hook() {
        for e in super::EVENTS {
            assert!(crate::platform::plugins::HOOKS.contains(&e), "{e} is not in HOOKS");
        }
    }
}
