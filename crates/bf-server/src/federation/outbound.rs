//! Everything that leaves the site: signed fetches, the remote-actor cache, the domain block list and the delivery queue with its retrying worker.

use crate::state::AppState;
use bf_db::federation;
use serde_json::Value;
use std::time::Duration;

use super::*;

/// A signed GET of an ActivityPub document, as the instance actor (for servers that require
/// signed fetches).
pub(super) async fn fetch(state: &AppState, uri: &str) -> Result<Value, String> {
    if !crate::platform::outbound::federation_url_ok(uri) || blocked(state, uri) {
        return Err("address not allowed".into());
    }
    let url = url::Url::parse(uri).map_err(|e| e.to_string())?;
    let (_, key) = key_for(state, "instance", 0).await.map_err(|_| "no instance key".to_owned())?;
    let mut headers = sign(&key, &format!("{}/ap/actor#main-key", base(state)), "GET", &url, None);
    headers.push((
        "accept".to_owned(),
        r#"application/activity+json, application/ld+json; profile="https://www.w3.org/ns/activitystreams""#.to_owned(),
    ));
    let (status, body) =
        crate::platform::outbound::request_public(reqwest::Method::GET, uri, &headers, None, MAX_FETCH).await?;
    if !(200..300).contains(&status) {
        return Err(format!("status {status}"));
    }
    serde_json::from_slice(&body).map_err(|e| e.to_string())
}

pub(super) fn str_of<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    match v.get(key)? {
        Value::String(s) => Some(s),
        Value::Object(o) => o.get("id").and_then(Value::as_str),
        _ => None,
    }
}

/// The host of a URI, lowercase and without the root dot (`example.com.` is `example.com`, so
/// it cannot slip past the block list or get limits of its own).
pub(super) fn domain_of(uri: &str) -> Option<String> {
    url::Url::parse(uri).ok().and_then(|u| u.host_str().map(|h| h.trim_end_matches('.').to_ascii_lowercase()))
}

/// `federation_blocked_domains`: one domain per line (or comma), subdomains included.
pub(super) fn blocked(state: &AppState, uri: &str) -> bool {
    let Some(d) = domain_of(uri) else { return true };
    state
        .settings()
        .str("federation_blocked_domains")
        .split(|c: char| c == '\n' || c == ',' || c.is_whitespace())
        .map(|b| b.trim().trim_start_matches("*.").to_ascii_lowercase())
        .filter(|b| !b.is_empty())
        .any(|b| d == b || d.ends_with(&format!(".{b}")))
}

/// An actor document is trusted only for the host it was fetched from: its id, its key's id, its
/// inbox and its shared inbox must all be there, or a server could serve another host's actor id
/// with its own key and take over that actor's cached key.
pub(super) fn check_actor_hosts(fetched_from: &str, doc: &Value) -> Result<(), &'static str> {
    let id = doc.get("id").and_then(Value::as_str).ok_or("actor without id")?;
    let host = domain_of(id).ok_or("actor without host")?;
    if domain_of(fetched_from).as_deref() != Some(host.as_str()) {
        return Err("actor id on another host");
    }
    let on_host = |u: &str| domain_of(u).as_deref() == Some(host.as_str());
    if !doc.get("publicKey").and_then(|k| str_of(k, "id")).is_some_and(on_host) {
        return Err("key id on another host");
    }
    if !doc.get("inbox").and_then(Value::as_str).is_some_and(on_host) {
        return Err("inbox on another host");
    }
    if let Some(shared) = doc.pointer("/endpoints/sharedInbox").and_then(Value::as_str)
        && !on_host(shared)
    {
        return Err("shared inbox on another host");
    }
    Ok(())
}

/// Actor and key lookups the server makes per minute, whoever asks for them.
const FETCHES_PER_MIN: u32 = 120;

/// True while this minute's budget of actor lookups lasts.
fn fetch_budget() -> bool {
    use std::sync::Mutex;
    static WINDOW: Mutex<Option<(std::time::Instant, u32)>> = Mutex::new(None);
    let mut w = WINDOW.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let now = std::time::Instant::now();
    match &mut *w {
        Some((start, n)) if now.duration_since(*start) < std::time::Duration::from_secs(60) => {
            *n += 1;
            *n <= FETCHES_PER_MIN
        }
        _ => {
            *w = Some((now, 1));
            true
        }
    }
}

/// The remote actor behind a URI, from the cache or fetched (and refreshed when `refresh`).
pub(super) async fn remote_actor(
    state: &AppState,
    uri: &str,
    refresh: bool,
) -> Result<federation::RemoteActor, String> {
    let pool = state.pool.as_ref().ok_or("no database")?;
    if !refresh && let Ok(Some(a)) = federation::actor_by_uri(pool, uri).await {
        return Ok(a);
    }
    // One budget for every fetch an inbox request can start: per-domain limits do not hold when
    // each forged request names a new domain, and each fetch is a signed GET to a host of the
    // sender's choosing.
    if !fetch_budget() {
        return Err("too many actor lookups just now".into());
    }
    let doc = fetch(state, uri).await?;
    // A key document points at its owner, whose own document is fetched from the owner's host.
    let (doc, fetched_from) = if doc.get("inbox").is_none() {
        match doc.get("owner").and_then(Value::as_str) {
            Some(owner) if owner != uri => (fetch(state, owner).await?, owner),
            _ => return Err("not an actor".into()),
        }
    } else {
        (doc, uri)
    };
    check_actor_hosts(fetched_from, &doc)?;
    let id = doc.get("id").and_then(Value::as_str).ok_or("actor without id")?;
    let inbox = doc.get("inbox").and_then(Value::as_str).ok_or("actor without inbox")?;
    let key = doc.get("publicKey").ok_or("actor without key")?;
    let key_id = key.get("id").and_then(Value::as_str).ok_or("key without id")?;
    let pem = key.get("publicKeyPem").and_then(Value::as_str).ok_or("key without pem")?;
    let domain = domain_of(id).ok_or("actor without host")?;
    if !crate::platform::outbound::federation_url_ok(inbox) {
        return Err("inbox not allowed".into());
    }
    let shared = doc.pointer("/endpoints/sharedInbox").and_then(Value::as_str);
    let username: String = doc
        .get("preferredUsername")
        .and_then(Value::as_str)
        .unwrap_or("someone")
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .take(30)
        .collect();
    let username = if username.is_empty() { "someone".to_owned() } else { username };
    let name: Option<String> =
        doc.get("name").and_then(Value::as_str).map(|n| strip_html(n).chars().take(60).collect());
    let kind: String = doc.get("type").and_then(Value::as_str).unwrap_or("Person").chars().take(20).collect();
    federation::upsert_actor(
        pool,
        &federation::NewActor {
            uri: id,
            domain: &domain,
            kind: &kind,
            username: &username,
            display_name: name.as_deref(),
            url: doc.get("url").and_then(Value::as_str),
            inbox,
            shared_inbox: shared,
            key_id,
            public_pem: pem,
        },
    )
    .await
    .map_err(|e| e.to_string())
}

pub(super) async fn enqueue(state: &AppState, inboxes: &[String], signer: (&str, i64), activity: &Value) {
    let Some(pool) = &state.pool else { return };
    let mut seen = std::collections::HashSet::new();
    for inbox in inboxes {
        if !seen.insert(inbox.clone()) || blocked(state, inbox) || inbox.starts_with(&format!("{}/", base(state))) {
            continue;
        }
        if let Err(e) = federation::enqueue(pool, inbox, signer.0, signer.1, activity).await {
            tracing::warn!(error = %e, "federation delivery not queued");
        }
    }
}

/// Sends queued activities; retries with backoff (1, 2, 4 … minutes), then gives up.
pub fn spawn_worker(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(bf_core::scaled(Duration::from_secs(2)));
        loop {
            tick.tick().await;
            let Some(pool) = state.pool.clone() else { return };
            if !enabled(&state) || state.settings().bool("offline") {
                continue;
            }
            let due = match federation::due(&pool, 50).await {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!(error = %e, "federation queue read failed");
                    continue;
                }
            };
            send_batch(&state, &pool, due).await;
        }
    });
}

/// Deliveries in flight at once, and at most this many to one host.
const GLOBAL_CONCURRENCY: usize = 16;
const HOST_CONCURRENCY: usize = 2;
/// Claimed deliveries are leased for 5 minutes (`federation::due`); a delivery not started by
/// this point is handed back so another round (or node) can take it.
const BATCH_BUDGET: Duration = Duration::from_secs(200);

/// Sends a claimed batch concurrently, so one slow or black-holed host cannot stall the rest.
async fn send_batch(state: &AppState, pool: &sqlx::PgPool, due: Vec<federation::Delivery>) {
    let global = std::sync::Arc::new(tokio::sync::Semaphore::new(GLOBAL_CONCURRENCY));
    let mut per_host: std::collections::HashMap<String, std::sync::Arc<tokio::sync::Semaphore>> =
        std::collections::HashMap::new();
    let started = tokio::time::Instant::now();
    let mut tasks = tokio::task::JoinSet::new();
    for d in due {
        let host = domain_of(&d.inbox).unwrap_or_default();
        let host_gate = per_host
            .entry(host)
            .or_insert_with(|| std::sync::Arc::new(tokio::sync::Semaphore::new(HOST_CONCURRENCY)))
            .clone();
        let (state, pool, global) = (state.clone(), pool.clone(), global.clone());
        tasks.spawn(async move {
            let (Ok(_host), Ok(_slot)) = (host_gate.acquire().await, global.acquire().await) else { return };
            if started.elapsed() > BATCH_BUDGET {
                // Out of time: release the claim instead of letting the lease run out.
                if let Err(e) = federation::release(&pool, d.id).await {
                    tracing::warn!(error = %e, "federation delivery not released");
                }
                return;
            }
            send_one(&state, &pool, &d).await;
        });
    }
    while tasks.join_next().await.is_some() {}
}

async fn send_one(state: &AppState, pool: &sqlx::PgPool, d: &federation::Delivery) {
    // A domain blocked after the activity was queued must not receive it.
    if blocked(state, &d.inbox) {
        tracing::info!(inbox = %d.inbox, "federation delivery dropped: domain blocked");
        if let Err(e) = federation::delivered(pool, d.id).await {
            tracing::warn!(error = %e, "federation delivery status not saved");
        }
        return;
    }
    let saved = match deliver(state, d).await {
        Ok(()) => federation::delivered(pool, d.id).await,
        // 4xx other than 429 will not get better by retrying.
        Err((Some(status), _)) if (400..500).contains(&status) && status != 429 => {
            federation::delivered(pool, d.id).await
        }
        Err((_, e)) if d.attempts + 1 >= MAX_ATTEMPTS => {
            tracing::warn!(inbox = %d.inbox, error = %e, "federation delivery given up");
            federation::delivered(pool, d.id).await
        }
        Err((_, e)) => federation::retry(pool, d.id, 60 * 2_i64.pow(d.attempts as u32), &e).await,
    };
    metrics::counter!("bf_federation_deliveries_total", "ok" => if saved.is_ok() { "true" } else { "false" })
        .increment(1);
    if let Err(e) = saved {
        tracing::warn!(error = %e, "federation delivery status not saved");
    }
}

pub(super) async fn deliver(state: &AppState, d: &federation::Delivery) -> Result<(), (Option<u16>, String)> {
    let url = url::Url::parse(&d.inbox).map_err(|e| (None, e.to_string()))?;
    let (_, key) = key_for(state, &d.signer_type, d.signer_id).await.map_err(|e| (None, e.to_string()))?;
    let body = serde_json::to_vec(&d.activity).map_err(|e| (None, e.to_string()))?;
    let headers =
        sign(&key, &format!("{}#main-key", actor_uri(state, &d.signer_type, d.signer_id)), "POST", &url, Some(&body));
    let (status, _) = crate::platform::outbound::request_public_truncated(
        reqwest::Method::POST,
        &d.inbox,
        &headers,
        Some(body),
        64 * 1024,
    )
    .await
    .map_err(|e| (None, e))?;
    if (200..300).contains(&status) { Ok(()) } else { Err((Some(status), format!("status {status}"))) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn actor(host: &str) -> Value {
        json!({
            "id": format!("https://{host}/users/alice"),
            "inbox": format!("https://{host}/users/alice/inbox"),
            "endpoints": { "sharedInbox": format!("https://{host}/inbox") },
            "publicKey": { "id": format!("https://{host}/users/alice#main-key"), "publicKeyPem": "pem" },
        })
    }

    #[test]
    fn an_actor_on_its_own_host_is_accepted() {
        let doc = actor("remote.example");
        assert_eq!(check_actor_hosts("https://remote.example/users/alice", &doc), Ok(()));
        assert_eq!(check_actor_hosts("https://remote.example/users/alice#main-key", &doc), Ok(()));
    }

    #[test]
    fn another_hosts_actor_id_with_our_own_key_is_refused() {
        // evil.example serves victim.example's id, but names its own key id.
        let mut doc = actor("victim.example");
        doc["publicKey"]["id"] = json!("https://evil.example/key#main-key");
        assert!(check_actor_hosts("https://evil.example/fake", &doc).is_err());
        // Even with every field on the victim's host, a document fetched from evil.example is refused.
        assert_eq!(
            check_actor_hosts("https://evil.example/fake", &actor("victim.example")),
            Err("actor id on another host")
        );
    }

    #[test]
    fn an_owner_document_must_come_from_the_owners_host() {
        assert!(check_actor_hosts("https://victim.example/users/alice", &actor("victim.example")).is_ok());
        assert!(check_actor_hosts("https://evil.example/users/alice", &actor("victim.example")).is_err());
    }

    #[test]
    fn inboxes_must_be_on_the_actors_host() {
        let mut doc = actor("remote.example");
        doc["inbox"] = json!("https://other.example/inbox");
        assert_eq!(check_actor_hosts("https://remote.example/users/alice", &doc), Err("inbox on another host"));
        let mut doc = actor("remote.example");
        doc["endpoints"]["sharedInbox"] = json!("https://other.example/inbox");
        assert_eq!(check_actor_hosts("https://remote.example/users/alice", &doc), Err("shared inbox on another host"));
        let mut doc = actor("remote.example");
        doc.as_object_mut().unwrap().remove("endpoints");
        assert!(check_actor_hosts("https://remote.example/users/alice", &doc).is_ok());
    }
}
