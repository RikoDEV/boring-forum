//! Live updates over server-sent events.

use crate::forum::topics::readable_topic;
use crate::http::render::DbResult;
use crate::http::render::pool;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::Router;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use bf_core::AppError;
use bf_core::perms::{Actor, Perms};
use serde::Deserialize;
use std::sync::{Arc, RwLock};

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/live", get(live))
}

#[derive(Deserialize)]
struct LiveQuery {
    channels: String,
}

/// One hub channel carries every forum's new topics and replies; each stream keeps only the
/// forums its viewer may read (`forum:3` for one, `forums` for all of them).
pub(crate) const ACTIVITY: &str = "activity";

/// How often an open stream re-checks that its viewer may still read what it carries.
const CHECK_EVERY: std::time::Duration = std::time::Duration::from_secs(30);

/// A stream is ended after this long; the browser reconnects, and is checked again.
const MAX_LIFE: std::time::Duration = std::time::Duration::from_secs(3600);

/// Open streams one address and one member may hold. A stream is a socket, a receiver and a
/// re-check loop, so unlimited ones are a way to use up a server.
const PER_IP: u32 = 30;
const PER_USER: u32 = 12;

static OPEN: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, u32>>> =
    std::sync::LazyLock::new(Default::default);

/// One open stream's place under a limit; given back when the stream ends or is dropped.
struct Slot(String);

impl Slot {
    fn take(key: String, max: u32) -> Option<Slot> {
        let mut open = OPEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let n = open.entry(key.clone()).or_insert(0);
        if *n >= max {
            return None;
        }
        *n += 1;
        Some(Slot(key))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut open = OPEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(n) = open.get_mut(&self.0) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                open.remove(&self.0);
            }
        }
    }
}

/// The forums the viewer may read among those a stream asked for.
async fn visible_forums(state: &AppState, actor: &Actor, all: bool, named: &[i64]) -> Result<Vec<i64>, AppError> {
    if !all && named.is_empty() {
        return Ok(Vec::new());
    }
    let tree = state.forum.get(state.db()?).await?;
    let mut list = if all { tree.readable(actor) } else { Vec::new() };
    list.extend(named.iter().copied().filter(|id| tree.perms(actor, *id).has(Perms::READ)));
    Ok(list)
}

/// The viewer as they are now: a signed-in member's session and groups are loaded again. None
/// once the session has ended or the account is suspended.
async fn current(state: &AppState, v: &Viewer) -> Option<Viewer> {
    let (Some(digest), Some(pool)) = (&v.session_digest, &state.pool) else { return Some(v.clone()) };
    let u = match bf_db::accounts::sessions::session_user(pool, digest).await {
        Ok(Some(u)) => u,
        Ok(None) => return None,
        // A database hiccup is not a revocation.
        Err(_) => return Some(v.clone()),
    };
    let mut now = v.clone();
    now.actor = crate::http::viewer::actor_for(&u);
    now.actor.denied = crate::platform::plugins::denials_cached(u.id.get());
    now.user = Some(u);
    Some(now)
}

/// SSE: `GET /api/v1/live?channels=item:12,user,global`. Each channel is checked.
async fn live(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Query(q): Query<LiveQuery>,
) -> Result<Response, AppError> {
    let too_many = || AppError::RateLimited { retry_after_secs: 30 };
    let mut slots =
        vec![Slot::take(format!("ip:{}", crate::http::ratelimit::ip_key(v.ip)), PER_IP).ok_or_else(too_many)?];
    if let Some(u) = &v.user {
        slots.push(Slot::take(format!("user:{}", u.id), PER_USER).ok_or_else(too_many)?);
    }
    let mut channels = Vec::new();
    // Forums a stream may carry: every readable one (`forums`) and the named ones (`forum:3`).
    let mut all_forums = false;
    let mut named_forums: Vec<i64> = Vec::new();
    for c in q.channels.split(',').take(10) {
        match c.split_once(':') {
            None if c == "global" => channels.push("global".to_owned()),
            None if c == "user" => channels.push(format!("user:{}", v.require_user()?.id)),
            Some(("conversation", id)) => {
                let id: i64 = id.parse().map_err(|_| AppError::BadRequest("bad channel".into()))?;
                let user = v.require_user()?;
                bf_db::community::messages::participant_of(state.db()?, id, user.id.get())
                    .await
                    .db()?
                    .ok_or(AppError::NotFound)?;
                channels.push(format!("conversation:{id}"));
            }
            Some(("chat", id)) => {
                let id: i64 = id.parse().map_err(|_| AppError::BadRequest("bad channel".into()))?;
                crate::community::chat::viewable(&state, &v, id).await?;
                channels.push(format!("chat:{id}"));
            }
            Some(("item", id)) => {
                let id: i64 = id.parse().map_err(|_| AppError::BadRequest("bad channel".into()))?;
                readable_topic(&state, &v, id).await?;
                channels.push(format!("item:{id}"));
            }
            None if c == "forums" => {
                all_forums = true;
            }
            Some(("forum", id)) => {
                let id: i64 = id.parse().map_err(|_| AppError::BadRequest("bad channel".into()))?;
                let tree = state.forum.get(state.db()?).await?;
                if !tree.perms(&v.actor, id).has(Perms::READ) {
                    return Err(AppError::NotFound);
                }
                named_forums.push(id);
            }
            _ => return Err(AppError::BadRequest("unknown channel".into())),
        }
    }
    let last = headers.get("last-event-id").and_then(|h| h.to_str().ok()).and_then(|s| s.parse().ok());
    let activity = all_forums || !named_forums.is_empty();
    if activity {
        channels.push(ACTIVITY.to_owned());
    }
    // Readable now, and refreshed by the loop below while the stream is open.
    let forums = Arc::new(RwLock::new(visible_forums(&state, &v.actor, all_forums, &named_forums).await?));
    let seen = forums.clone();
    let keep = move |m: &bf_realtime::Message| {
        m.channel != ACTIVITY
            || m.data["node_id"]
                .as_i64()
                .is_some_and(|n| seen.read().unwrap_or_else(std::sync::PoisonError::into_inner).contains(&n))
    };
    let mut res = state.hub.sse(&channels, last, keep).into_response();
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    // Access was only checked on subscribing: keep checking, and end the stream once the member
    // loses it. The client reconnects and is refused.
    let watched: Vec<String> = channels.iter().filter(|c| *c != "global" && *c != ACTIVITY).cloned().collect();
    let need_checks = !watched.is_empty() || activity;
    {
        let (state2, v2) = (state.clone(), v.clone());
        let revoked = async move {
            // The slots are kept for as long as this future (and so the stream) lives.
            let _slots = slots;
            let check = async move {
                if !need_checks {
                    std::future::pending::<()>().await;
                }
                let mut v2 = v2;
                loop {
                    tokio::time::sleep(CHECK_EVERY).await;
                    // Their groups and session as they are now, not as they were on connecting.
                    let Some(now) = current(&state2, &v2).await else { return };
                    v2 = now;
                    if activity && let Ok(list) = visible_forums(&state2, &v2.actor, all_forums, &named_forums).await {
                        *forums.write().unwrap_or_else(std::sync::PoisonError::into_inner) = list;
                    }
                    for c in &watched {
                        let Some((kind, id)) = c.split_once(':') else { continue };
                        let Ok(id) = id.parse::<i64>() else { continue };
                        let refused =
                            |r: Result<_, AppError>| matches!(r, Err(AppError::NotFound | AppError::Forbidden));
                        let allowed = match kind {
                            "chat" => !refused(crate::community::chat::viewable(&state2, &v2, id).await.map(|_| ())),
                            "item" => !refused(readable_topic(&state2, &v2, id).await.map(|_| ())),
                            "conversation" => match (pool(&state2), v2.user.as_ref()) {
                                (Ok(p), Some(u)) => !matches!(
                                    bf_db::community::messages::participant_of(p, id, u.id.get()).await,
                                    Ok(None)
                                ),
                                _ => false,
                            },
                            _ => true,
                        };
                        if !allowed {
                            return;
                        }
                    }
                }
            };
            let _ = tokio::time::timeout(MAX_LIFE, check).await;
        };
        let (parts, body) = res.into_parts();
        let body = axum::body::Body::from_stream(futures_util::StreamExt::take_until(
            body.into_data_stream(),
            Box::pin(revoked),
        ));
        res = Response::from_parts(parts, body);
    }
    Ok(res)
}
