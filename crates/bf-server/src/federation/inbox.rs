//! The shared and per-actor inboxes: signature verification and handling of each incoming activity.

use crate::state::AppState;
use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use bf_db::federation;
use serde_json::{Value, json};

use super::*;

pub(super) fn problem(status: StatusCode, detail: &str) -> Response {
    (status, Json(json!({ "error": detail }))).into_response()
}

/// The address the request came from (`X-Forwarded-For` only from a trusted proxy); unknown in
/// unit tests, where every request then shares one source.
pub(super) struct Source(String);

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for Source {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut axum::http::request::Parts, _: &S) -> Result<Self, Self::Rejection> {
        let peer = parts.extensions.get::<axum::extract::ConnectInfo<std::net::SocketAddr>>().map(|c| c.0.ip());
        Ok(Source(peer.map_or_else(String::new, |p| crate::http::ratelimit::client_ip(p, &parts.headers).to_string())))
    }
}

pub(super) async fn inbox(
    State(state): State<AppState>,
    Source(source): Source,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !enabled(&state) {
        return StatusCode::NOT_FOUND.into_response();
    }
    if body.len() > MAX_INBOX {
        return problem(StatusCode::PAYLOAD_TOO_LARGE, "too large");
    }
    let activity: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return problem(StatusCode::BAD_REQUEST, "not JSON"),
    };
    let Some(actor_id) = str_of(&activity, "actor").map(str::to_owned) else {
        return problem(StatusCode::BAD_REQUEST, "no actor");
    };
    if blocked(&state, &actor_id) {
        metrics::counter!("bf_federation_inbox_total", "result" => "blocked").increment(1);
        return StatusCode::ACCEPTED.into_response();
    }
    let claimed = domain_of(&actor_id).unwrap_or_default();
    // The key is fetched from the host in `keyId`, which need not be the claimed actor's: a
    // sender that names a new actor host each time still uses up the key host's budget.
    let key_host = headers
        .get("signature")
        .and_then(|v| v.to_str().ok())
        .and_then(parse_signature)
        .and_then(|p| domain_of(&p.key_id))
        .filter(|h| *h != claimed)
        .unwrap_or_default();
    // Failures are charged to the sender's address together with the domain it names, never to the
    // domain alone: anyone can claim a domain, and the real server's signed deliveries (from
    // another address) must not be refused because someone else's forgeries named it.
    let claimed_bucket = format!("{source}|{claimed}");
    let key_bucket = format!("{source}|{key_host}");
    if !throttle::allow(&claimed_bucket, throttle::Kind::Unverified)
        || !throttle::allow(&format!("{source}|"), throttle::Kind::Unverified)
        || (!key_host.is_empty() && !throttle::allow(&key_bucket, throttle::Kind::Unverified))
    {
        metrics::counter!("bf_federation_inbox_total", "result" => "throttled").increment(1);
        return throttled();
    }
    let actor = match verify(&state, &method, &uri, &headers, &body, &actor_id).await {
        Ok(a) => a,
        Err(e) => {
            throttle::failed(&claimed_bucket);
            throttle::failed(&format!("{source}|"));
            if !key_host.is_empty() {
                throttle::failed(&key_bucket);
            }
            // A Delete of an account whose key is gone cannot be verified; it is dropped quietly.
            if activity["type"] == "Delete" && str_of(&activity, "object") == Some(actor_id.as_str()) {
                return StatusCode::ACCEPTED.into_response();
            }
            metrics::counter!("bf_federation_inbox_total", "result" => "unverified").increment(1);
            tracing::info!(actor = %actor_id, error = %e, "federation: signature not accepted");
            return problem(StatusCode::UNAUTHORIZED, "signature not accepted");
        }
    };
    // The real per-domain throttle: it counts only signed activities, so a forged `actor` cannot
    // use up another server's allowance. The sender retries on 429.
    if !throttle::allow(&actor.domain, throttle::Kind::Verified) {
        metrics::counter!("bf_federation_inbox_total", "result" => "throttled").increment(1);
        return throttled();
    }
    // Without an id there is nothing to deduplicate on, so a captured request could be replayed.
    let Some(id) = activity.get("id").and_then(Value::as_str) else {
        return problem(StatusCode::BAD_REQUEST, "activity without id");
    };
    let Some(pool) = &state.pool else { return problem(StatusCode::SERVICE_UNAVAILABLE, "no database") };
    if domain_of(id).as_deref() != Some(actor.domain.as_str()) {
        return problem(StatusCode::BAD_REQUEST, "activity id on another host");
    }
    match federation::first_sight(pool, id).await {
        Ok(true) => {}
        Ok(false) => return StatusCode::ACCEPTED.into_response(),
        // Failing open would let a replay through; the sender retries on 503.
        Err(e) => {
            tracing::warn!(error = %e, "federation: seen-activity check failed");
            return problem(StatusCode::SERVICE_UNAVAILABLE, "try again later");
        }
    }
    let result = handle(&state, &actor, &activity).await;
    metrics::counter!("bf_federation_inbox_total", "result" => if result.is_ok() { "ok" } else { "refused" })
        .increment(1);
    if let Err(e) = result {
        tracing::info!(actor = %actor.uri, error = %e, "federation: activity not applied");
        // A failure may be transient (a remote fetch): let a resend of the same id be handled.
        if let Err(e) = federation::forget_seen(pool, id).await {
            tracing::warn!(error = %e, "federation: activity id not released");
        }
    }
    StatusCode::ACCEPTED.into_response()
}

fn throttled() -> Response {
    let mut r = problem(StatusCode::TOO_MANY_REQUESTS, "slow down");
    r.headers_mut().insert(axum::http::header::RETRY_AFTER, axum::http::HeaderValue::from_static("60"));
    r
}

/// The budget for requests whose signature failed before, for the signed fetches checked in
/// `extended` (keyed on the host the key is fetched from).
pub(super) fn unverified_allowed(domain: &str) -> bool {
    throttle::allow(domain, throttle::Kind::Unverified)
}

pub(super) fn unverified_failed(domain: &str) {
    throttle::failed(domain);
}

/// Per-domain request budgets for the inboxes, in memory and per minute.
mod throttle {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    /// Signed activities one remote domain may deliver per minute.
    const VERIFIED_PER_MIN: u32 = 600;
    /// Requests per minute from one claimed domain whose signature failed before.
    const FAILED_PER_MIN: u32 = 30;
    const WINDOW: Duration = Duration::from_secs(60);
    const MAX_DOMAINS: usize = 20_000;

    #[derive(Clone, Copy, PartialEq)]
    pub(super) enum Kind {
        Unverified,
        Verified,
    }

    #[derive(Default)]
    struct Counts {
        started: Option<Instant>,
        verified: u32,
        failed: u32,
    }

    fn table() -> &'static Mutex<HashMap<String, Counts>> {
        static T: OnceLock<Mutex<HashMap<String, Counts>>> = OnceLock::new();
        T.get_or_init(Default::default)
    }

    fn with<R>(domain: &str, f: impl FnOnce(&mut Counts) -> R) -> R {
        let mut t = table().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        if t.len() > MAX_DOMAINS {
            t.retain(|_, c| c.started.is_some_and(|s| now.duration_since(s) < WINDOW));
            // All still fresh: a flood of invented domains. Start over, so the table stays small.
            if t.len() > MAX_DOMAINS {
                t.clear();
            }
        }
        let c = t.entry(domain.to_ascii_lowercase()).or_default();
        if c.started.is_none_or(|s| now.duration_since(s) >= WINDOW) {
            *c = Counts { started: Some(now), ..Counts::default() };
        }
        f(c)
    }

    /// Counts one request; false once the domain is over its budget for this minute.
    pub(super) fn allow(domain: &str, kind: Kind) -> bool {
        with(domain, |c| match kind {
            Kind::Unverified => c.failed < FAILED_PER_MIN,
            Kind::Verified => {
                c.verified = c.verified.saturating_add(1);
                c.verified <= VERIFIED_PER_MIN
            }
        })
    }

    pub(super) fn failed(domain: &str) {
        with(domain, |c| c.failed = c.failed.saturating_add(1));
    }
}

/// How far a signed request's `Date` may be from now, either way. Short, because a captured
/// request can be replayed until it expires.
const MAX_CLOCK_SKEW_SECONDS: i64 = 300;

pub(super) fn date_fresh(date: &str, now: chrono::DateTime<chrono::Utc>) -> Result<(), &'static str> {
    let when = chrono::DateTime::parse_from_rfc2822(date).map_err(|_| "bad date")?;
    if (now - when.with_timezone(&chrono::Utc)).num_seconds().abs() > MAX_CLOCK_SKEW_SECONDS {
        return Err("date too far from now");
    }
    Ok(())
}

/// The signed `Host` must be this site's, or a request signed for another server could be replayed here.
pub(super) fn host_matches(header: Option<&str>, ours: &str) -> bool {
    header.is_some_and(|h| h.trim().eq_ignore_ascii_case(ours))
}

/// Whether a `Digest` header carries the SHA-256 of `body`: the base64 is decoded and the bytes
/// compared in constant time.
pub(super) fn digest_matches(header: &str, body: &[u8]) -> bool {
    use base64::Engine;
    use rsa::sha2::{Digest, Sha256};
    let ours = Sha256::digest(body);
    header.split(',').any(|part| {
        let Some((algorithm, value)) = part.trim().split_once('=') else { return false };
        if !algorithm.eq_ignore_ascii_case("sha-256") {
            return false;
        }
        let Ok(theirs) = base64::engine::general_purpose::STANDARD.decode(value) else { return false };
        theirs.len() == ours.len() && theirs.iter().zip(ours.iter()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
    })
}

/// Checks the HTTP signature, digest and date, and that the key belongs to the activity's actor.
pub(super) async fn verify(
    state: &AppState,
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    body: &[u8],
    actor_id: &str,
) -> Result<federation::RemoteActor, String> {
    let sig = headers.get("signature").and_then(|v| v.to_str().ok()).ok_or("unsigned")?;
    let p = parse_signature(sig).ok_or("bad signature header")?;
    for required in ["(request-target)", "host", "date", "digest"] {
        if !p.headers.iter().any(|h| h == required) {
            return Err(format!("{required} not signed"));
        }
    }
    let digest = headers.get("digest").and_then(|v| v.to_str().ok()).ok_or("no digest")?;
    if !digest_matches(digest, body) {
        return Err("digest mismatch".into());
    }
    if !host_matches(headers.get("host").and_then(|v| v.to_str().ok()), &host(state)) {
        return Err("signed for another host".into());
    }
    let date = headers.get("date").and_then(|v| v.to_str().ok()).ok_or("no date")?;
    date_fresh(date, chrono::Utc::now())?;
    let msg = signing_string(&p, method, uri, headers).ok_or("signed header missing")?;
    let key_owner = p.key_id.split('#').next().unwrap_or_default().to_owned();
    let pool = state.pool.as_ref().ok_or("no database")?;
    let cached = federation::actor_by_key(pool, &p.key_id).await.ok().flatten();
    let mut actor = match cached {
        Some(a) => a,
        None => remote_actor(state, &key_owner, true).await?,
    };
    if !verify_with(&actor.public_pem, &msg, &p.signature) {
        // The key may have been rotated: fetch once more, unless that was just done.
        if chrono::Utc::now() - actor.fetched_at < chrono::Duration::seconds(60) {
            return Err("signature does not verify".into());
        }
        actor = remote_actor(state, &actor.uri, true).await?;
        if !verify_with(&actor.public_pem, &msg, &p.signature) {
            return Err("signature does not verify".into());
        }
    }
    if actor.uri != actor_id {
        return Err("signed by someone other than the actor".into());
    }
    Ok(actor)
}

/// (kind, id) of a local actor or note URI.
pub(super) fn local_target(state: &AppState, uri: &str) -> Option<(&'static str, i64)> {
    let rest = uri.strip_prefix(&format!("{}/ap/", base(state)))?;
    let (kind, id) = rest.split_once('/')?;
    let id: i64 = id.split(['/', '#', '?']).next()?.parse().ok()?;
    match kind {
        "u" => Some(("user", id)),
        "c" => Some(("forum", id)),
        "p" => Some(("post", id)),
        _ => None,
    }
}

/// The local post an `inReplyTo` names: one of ours, or a remote reply that became a post.
pub(super) async fn local_post_for(state: &AppState, uri: &str) -> Option<i64> {
    if let Some(("post", id)) = local_target(state, uri) {
        return Some(id);
    }
    let pool = state.pool.as_ref()?;
    if let Ok(Some((post, _))) = federation::object(pool, uri).await {
        return Some(post);
    }
    // A link to the topic page itself.
    let b = base(state);
    if uri.starts_with(&b) {
        let topic = crate::forum::onebox::internal_topic(uri, &b)?;
        let n = uri.split_once("#post-").and_then(|(_, n)| n.parse().ok()).unwrap_or(1);
        return federation::post_id_at(pool, topic, n).await.ok().flatten();
    }
    None
}

/// Routes one verified activity to the code for its type; unknown types are accepted and ignored.
pub(super) async fn handle(state: &AppState, actor: &federation::RemoteActor, a: &Value) -> Result<(), String> {
    let kind = a.get("type").and_then(Value::as_str).unwrap_or_default();
    let object = a.get("object").cloned().unwrap_or(Value::Null);
    match kind {
        "Follow" => handle_follow(state, actor, a).await,
        "Undo" => handle_undo(state, actor, &object).await,
        "Like" | "EmojiReact" => handle_like(state, actor, a).await,
        "Create" => {
            if extended::is_vote(&object) {
                return extended::vote(state, actor, &object).await;
            }
            if !is_public(&object) {
                return extended::inbound_dm(state, actor, &object).await;
            }
            create_reply(state, actor, &object).await
        }
        "Accept" => extended::answered(state, actor, a, true).await,
        "Reject" => extended::answered(state, actor, a, false).await,
        "Announce" => extended::announced(state, actor, &object).await,
        "Move" => extended::moved(state, actor, a).await,
        "Update" => handle_update(state, actor, &object).await,
        "Delete" => handle_delete(state, actor, a).await,
        "Flag" => handle_flag(state, actor, a).await,
        "Block" => handle_block(state, actor, a).await,
        // Boosts, acknowledgements and the rest need nothing from a forum.
        _ => Ok(()),
    }
}

/// Records the follower and answers with an `Accept`, so the remote server shows the follow as done.
async fn handle_follow(state: &AppState, actor: &federation::RemoteActor, a: &Value) -> Result<(), String> {
    let pool = state.pool.as_ref().ok_or("no database")?;
    let target = str_of(a, "object").ok_or("follow of nothing")?;
    let (local_type, id) = local_target(state, target).filter(|(t, _)| *t != "post").ok_or("not a local actor")?;
    let exists = match local_type {
        "user" => federation::local_user(pool, id).await.map_err(|e| e.to_string())?.is_some(),
        _ => public_forum(state, id).await,
    };
    if !exists {
        return Err("no such actor".into());
    }
    let follow_id = a.get("id").and_then(Value::as_str).ok_or("follow without id")?;
    federation::add_follower(pool, local_type, id, actor.id, follow_id).await.map_err(|e| e.to_string())?;
    // A person followed from elsewhere hears of it like a local follow, through the shared sender
    // (the follower's shadow account stands for them; suspension and ignore lists apply there).
    if local_type == "user" {
        let by = federation::ensure_shadow_user(pool, actor).await.map_err(|e| e.to_string())?;
        let name = format!("{}@{}", actor.username, actor.domain);
        let data = json!({ "by": name, "url": bf_core::slug::profile_path(by, &name), "group": format!("follower:{by}") });
        crate::community::notifications::send(state, by, &[id], "follower", &data).await;
    }
    let me = actor_uri(state, local_type, id);
    let accept = json!({
        "@context": context(), "id": format!("{me}/accept/{}", actor.id), "type": "Accept", "actor": me,
        "object": { "id": follow_id, "type": "Follow", "actor": actor.uri, "object": me },
    });
    enqueue(state, std::slice::from_ref(&actor.inbox), (local_type, id), &accept).await;
    Ok(())
}

/// Undo of a follow or of a like; any other undone activity needs nothing here.
async fn handle_undo(state: &AppState, actor: &federation::RemoteActor, object: &Value) -> Result<(), String> {
    let pool = state.pool.as_ref().ok_or("no database")?;
    match object.get("type").and_then(Value::as_str) {
        Some("Follow") => {
            let target = str_of(object, "object").ok_or("undo of nothing")?;
            if let Some((t, id)) = local_target(state, target).filter(|(t, _)| *t != "post") {
                federation::remove_follower(pool, t, id, actor.id).await.map_err(|e| e.to_string())?;
            }
            Ok(())
        }
        Some("Like") => {
            let target = str_of(object, "object").ok_or("undo of nothing")?;
            let post = local_post_for(state, target).await.ok_or("not a post here")?;
            if let Some(user) = actor.user_id {
                bf_db::forum::reactions::set_reaction(pool, post, user, None).await.map_err(|e| e.to_string())?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// A like on a public post here becomes a reaction by the sender's shadow account.
async fn handle_like(state: &AppState, actor: &federation::RemoteActor, a: &Value) -> Result<(), String> {
    let pool = state.pool.as_ref().ok_or("no database")?;
    let target = str_of(a, "object").ok_or("like of nothing")?;
    let post = local_post_for(state, target).await.ok_or("not a post here")?;
    let (p, _) = federable_post(state, post).await.ok_or("not a public post")?;
    let user = federation::ensure_shadow_user(pool, actor).await.map_err(|e| e.to_string())?;
    // A suspended or silenced shadow account does not react, as it does not reply.
    if restricted_here(pool, user).await? {
        return Err("account restricted here".into());
    }
    let first = bf_db::forum::reactions::reacted_at(pool, post, user).await.map_err(|e| e.to_string())?.is_none();
    // The daily allowance of local reactions applies to a remote member's likes too.
    bf_db::forum::reactions::set_reaction_capped(pool, post, user, Some(1), Some(state.settings().int("reactions_per_day")))
        .await
        .map_err(|e| e.to_string())?
        .ok_or("daily reaction limit reached")?;
    // The post's author hears about it like about a local like (their ignore list and notification
    // settings apply inside).
    if let (true, Some(author)) = (first, p.author_id)
        && let Ok(Some(t)) = bf_db::forum::topics::topic(pool, p.item_id).await
    {
        let url = crate::forum::topics::post_url(state, t.id, &t.slug, p.post_number);
        let liker = format!("{}@{}", actor.username, actor.domain);
        if bf_db::community::notifications::like(pool, author, &liker, user, post, &t.title, &url)
            .await
            .unwrap_or(false)
        {
            state.hub.publish(&format!("user:{author}"), "notification", json!({ "kind": "like" }));
        }
    }
    state.microcache.clear();
    Ok(())
}

/// An actor refresh, or an edit of a post by the same remote author.
async fn handle_update(state: &AppState, actor: &federation::RemoteActor, object: &Value) -> Result<(), String> {
    let pool = state.pool.as_ref().ok_or("no database")?;
    let id = object.get("id").and_then(Value::as_str).ok_or("update of nothing")?;
    if matches!(object.get("type").and_then(Value::as_str), Some("Person" | "Service" | "Group" | "Application")) {
        remote_actor(state, &actor.uri, true).await?;
        return Ok(());
    }
    let (post, owner) = federation::object(pool, id).await.map_err(|e| e.to_string())?.ok_or("unknown object")?;
    if owner != actor.id {
        return Err("update by someone else".into());
    }
    let raw = remote_text(object)?;
    let editor = actor.user_id.ok_or("no account")?;
    // What stops a new reply stops an edit too: a restricted account, a closed topic, watched words.
    if restricted_here(pool, editor).await? {
        return Err("account restricted here".into());
    }
    let p = federation::post(pool, post).await.map_err(|e| e.to_string())?.ok_or("no such post")?;
    let t = bf_db::forum::topics::topic(pool, p.item_id).await.map_err(|e| e.to_string())?.ok_or("no topic")?;
    if t.locked || t.archived {
        return Err("topic closed".into());
    }
    let tree = state.forum.get(pool).await.map_err(|e| e.to_string())?;
    let (raw, flagged) = match bf_core::watched::apply(&tree.words, &raw) {
        bf_core::watched::Outcome::Blocked(_) => return Err("blocked by watched words".into()),
        bf_core::watched::Outcome::Allow { text, needs_approval, .. } => (text, needs_approval),
    };
    // The same text filters as for a local edit; a refusal leaves the old text in place.
    let raw = crate::platform::plugins::before_post_system(state, editor, &raw).await.ok_or("refused by a plugin")?;
    let cooked = crate::platform::plugins::cook(state, &raw).await;
    bf_db::forum::posts::edit_post(
        pool,
        post,
        editor,
        &raw,
        &cooked.html,
        bf_cook::COOK_VERSION,
        Some("Edited on the author's server"),
    )
    .await
    .map_err(|e| e.to_string())?;
    if flagged {
        // The edit added a word that needs approval: the post waits for a moderator again.
        bf_db::forum::posts::requeue_post(pool, post).await.map_err(|e| e.to_string())?;
        state.forum.invalidate();
    } else {
        crate::platform::webhooks::emit(
            state,
            "post.edited",
            serde_json::json!({
                "topic_id": p.item_id, "post_number": p.post_number,
                "editor": format!("{}@{}", actor.username, actor.domain), "remote": true,
            }),
        )
        .await;
    }
    state.microcache.clear();
    Ok(())
}

/// Whether a shadow account is suspended or silenced on this site.
pub(super) async fn restricted_here(pool: &sqlx::PgPool, user: i64) -> Result<bool, String> {
    sqlx::query_scalar::<_, bool>(
        "SELECT coalesce(suspended_until > now(), false) OR coalesce(silenced_until > now(), false) FROM users WHERE id = $1",
    )
    .bind(user)
    .fetch_one(pool)
    .await
    .map_err(|e| e.to_string())
}

/// The sender deleting their whole account, or one of their posts.
async fn handle_delete(state: &AppState, actor: &federation::RemoteActor, a: &Value) -> Result<(), String> {
    let pool = state.pool.as_ref().ok_or("no database")?;
    let id = str_of(a, "object").ok_or("delete of nothing")?;
    if id == actor.uri {
        // The account is gone: stop sending to it, and keep its posts read-only.
        federation::remove_actor_follows(pool, actor.id).await.map_err(|e| e.to_string())?;
        if let Some(user) = actor.user_id {
            sqlx::query("UPDATE users SET suspended_until = 'infinity' WHERE id = $1 AND remote")
                .bind(user)
                .execute(pool)
                .await
                .map_err(|e| e.to_string())?;
        }
        return Ok(());
    }
    let Some((post, owner)) = federation::object(pool, id).await.map_err(|e| e.to_string())? else {
        return Ok(());
    };
    if owner != actor.id {
        return Err("delete by someone else".into());
    }
    let by = actor.user_id.ok_or("no account")?;
    bf_db::forum::posts::soft_delete_post(pool, post, by).await.map_err(|e| e.to_string())?;
    state.forum.invalidate();
    state.microcache.clear();
    Ok(())
}

/// A report from a remote server (or member) about content here: it lands in the review queue as
/// a report on the post, filed by the sender's shadow account. `object` lists the reported actor
/// and the reported posts; only posts that are public and local count. A remote report weighs
/// nothing toward `flag_hide_score`: one server can invent any number of reporters, so only
/// members' reports hide a post. An instance may file `federation_posts_per_hour` of them an hour.
async fn handle_flag(state: &AppState, actor: &federation::RemoteActor, a: &Value) -> Result<(), String> {
    let pool = state.pool.as_ref().ok_or("no database")?;
    let objects: Vec<&str> = match a.get("object") {
        Some(Value::String(s)) => vec![s.as_str()],
        Some(Value::Array(list)) => list.iter().filter_map(|v| v.as_str().or_else(|| str_of(v, "id"))).collect(),
        _ => Vec::new(),
    };
    let note: String = a.get("content").and_then(Value::as_str).unwrap_or_default().chars().take(500).collect();
    let note = format!("Federated report from {}: {}", actor.domain, note.trim());
    let limit = state.settings().int("federation_posts_per_hour");
    if federation::flags_from_domain_last_hour(pool, &actor.domain).await.map_err(|e| e.to_string())? >= limit {
        return Err("instance over its hourly limit".into());
    }
    let reporter = federation::ensure_shadow_user(pool, actor).await.map_err(|e| e.to_string())?;
    let mut filed = 0;
    for uri in objects.into_iter().take(10) {
        let Some(post) = local_post_for(state, uri).await else { continue };
        if federable_post(state, post).await.is_none() {
            continue;
        }
        let Some(target) = bf_db::forum::posts::post_ref(pool, post).await.map_err(|e| e.to_string())? else {
            continue;
        };
        bf_db::moderation::review::report(pool, reporter, &target, "inappropriate", Some(&note), 0.0)
            .await
            .map_err(|e| e.to_string())?;
        crate::community::notifications::tell_reviewers(state, target.node_id, reporter).await;
        filed += 1;
    }
    if filed == 0 {
        return Err("flag of nothing here".into());
    }
    state.microcache.clear();
    Ok(())
}

/// The sender blocks one of our actors: stop delivering that actor's activities to it.
async fn handle_block(state: &AppState, actor: &federation::RemoteActor, a: &Value) -> Result<(), String> {
    let pool = state.pool.as_ref().ok_or("no database")?;
    let target = str_of(a, "object").ok_or("block of nothing")?;
    let (kind, id) = local_target(state, target).filter(|(t, _)| *t != "post").ok_or("not a local actor")?;
    federation::remove_follower(pool, kind, id, actor.id).await.map_err(|e| e.to_string())
}

/// Addressed to everyone (`as:Public`).
pub(super) fn is_public(object: &Value) -> bool {
    ["to", "cc"].iter().any(|k| match object.get(*k) {
        Some(Value::Array(v)) => {
            v.iter().any(|x| x.as_str().is_some_and(|s| s == AS_PUBLIC || s == "as:Public" || s == "Public"))
        }
        Some(Value::String(s)) => s == AS_PUBLIC || s == "as:Public" || s == "Public",
        _ => false,
    })
}

/// A remote Note answering a post here becomes a reply by the author's shadow account.
pub(super) async fn create_reply(
    state: &AppState,
    actor: &federation::RemoteActor,
    object: &Value,
) -> Result<(), String> {
    let pool = state.pool.as_ref().ok_or("no database")?;
    if !matches!(object.get("type").and_then(Value::as_str), Some("Note" | "Article" | "Page")) {
        return Ok(());
    }
    let id = object.get("id").and_then(Value::as_str).ok_or("object without id")?;
    if domain_of(id).as_deref() != Some(actor.domain.as_str())
        || str_of(object, "attributedTo") != Some(actor.uri.as_str())
    {
        return Err("object not by its sender".into());
    }
    // Only public replies: direct messages and followers-only posts are not for a forum.
    if !is_public(object) {
        return Err("not public".into());
    }
    if federation::object(pool, id).await.map_err(|e| e.to_string())?.is_some() {
        return Ok(());
    }
    let parent_uri = str_of(object, "inReplyTo").ok_or("not a reply")?;
    let parent = local_post_for(state, parent_uri).await.ok_or("not a reply to a post here")?;
    let p = federation::post(pool, parent).await.map_err(|e| e.to_string())?.ok_or("no such post")?;
    if p.state != "visible" || p.topic_state != "visible" || !public_forum(state, p.node_id).await {
        return Err("post not public".into());
    }
    let t = bf_db::forum::topics::topic(pool, p.item_id).await.map_err(|e| e.to_string())?.ok_or("no topic")?;
    if t.locked || t.archived {
        return Err("topic closed".into());
    }
    let limit = state.settings().int("federation_posts_per_hour");
    if federation::objects_from_domain_last_hour(pool, &actor.domain).await.map_err(|e| e.to_string())? >= limit {
        return Err("instance over its hourly limit".into());
    }
    let raw = remote_text(object)?;
    let user = federation::ensure_shadow_user(pool, actor).await.map_err(|e| e.to_string())?;
    if restricted_here(pool, user).await? {
        return Err("account restricted here".into());
    }
    // The site's posting rules apply to remote replies as to members' (permission, trust level,
    // first-post approval, watched words, slow mode, limits); a refusal is dropped quietly.
    let tree = state.forum.get(pool).await.map_err(|e| e.to_string())?;
    let screened = crate::forum::posting::screen_federated(state, user, p.node_id, Some(p.item_id), &raw).await?;
    let raw = screened.text;
    let pending = screened.pending || tree.node(p.node_id).is_some_and(|n| n.approve_mode != "none");
    // Remote text goes through the plugins' text filters too; a refusal drops the reply.
    let raw = crate::platform::plugins::before_post_system(state, user, &raw).await.ok_or("refused by a plugin")?;
    let cooked = crate::platform::plugins::cook(state, &raw).await;
    crate::forum::posting::check_federated_cooked(state, &screened.user, &raw, &cooked).await?;
    let post = bf_db::forum::posts::NewPost {
        author_id: user,
        raw: &raw,
        cooked: &cooked.html,
        cooked_version: bf_cook::COOK_VERSION,
        ip: None,
        pending,
    };
    let number =
        bf_db::forum::posts::create_reply(pool, p.item_id, p.node_id, &post).await.map_err(|e| e.to_string())?;
    bf_db::forum::posts::set_reply_to(pool, p.item_id, number, p.post_number).await.map_err(|e| e.to_string())?;
    let new_id =
        federation::post_id_at(pool, p.item_id, number).await.map_err(|e| e.to_string())?.ok_or("post not found")?;
    federation::add_object(pool, id, new_id, actor.id).await.map_err(|e| e.to_string())?;
    state.forum.invalidate();
    if !pending {
        let by = format!("{}@{}", actor.username, actor.domain);
        crate::forum::posting::after_system_post(state, user, &by, &t.title, &t.slug, t.id, t.node_id, number).await;
    }
    metrics::counter!("bf_federation_replies_total").increment(1);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::federation::signatures::digest_header;

    #[test]
    fn the_signed_host_must_be_ours() {
        assert!(host_matches(Some("forum.example"), "forum.example"));
        assert!(host_matches(Some("Forum.Example"), "forum.example"));
        assert!(!host_matches(Some("other.example"), "forum.example"));
        assert!(!host_matches(None, "forum.example"));
    }

    #[test]
    fn dates_outside_five_minutes_are_refused() {
        let now = chrono::DateTime::parse_from_rfc2822("Mon, 09 Nov 2026 12:00:00 GMT").unwrap().to_utc();
        assert!(date_fresh("Mon, 09 Nov 2026 12:03:00 GMT", now).is_ok());
        assert!(date_fresh("Mon, 09 Nov 2026 11:57:00 GMT", now).is_ok());
        assert!(date_fresh("Mon, 09 Nov 2026 11:50:00 GMT", now).is_err());
        assert!(date_fresh("Mon, 09 Nov 2026 12:10:00 GMT", now).is_err());
        assert!(date_fresh("yesterday", now).is_err());
    }

    #[test]
    fn digests_compare_exactly() {
        let body = br#"{"type":"Follow"}"#;
        let good = digest_header(body);
        assert!(digest_matches(&good, body));
        assert!(digest_matches(&format!("sha-256={}", &good["SHA-256=".len()..]), body));
        assert!(digest_matches(&format!("SHA-512=abc, {good}"), body));
        // Base64 is case-sensitive: a differently cased value is another digest.
        assert!(!digest_matches(&good.to_ascii_lowercase(), body));
        assert!(!digest_matches(&good, b"other"));
        assert!(!digest_matches("SHA-256=", body));
    }
}
