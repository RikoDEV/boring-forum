//! API keys, webhooks and text overrides.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{FromRow, PgPool};

type Error = sqlx::Error;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct ApiKey {
    pub id: i64,
    pub name: String,
    pub scope: String,
    pub last_used_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    /// Scopes plugins declared that this key also holds.
    pub plugin_scopes: Vec<String>,
}

/// The lifetime a new key gets: 90 days unless asked otherwise, a year at most.
pub fn key_days(asked: Option<i32>) -> i32 {
    asked.unwrap_or(90).clamp(1, 365)
}

pub async fn create_key(
    pool: &PgPool,
    user: i64,
    name: &str,
    digest: &str,
    scope: &str,
    days: i32,
) -> Result<i64, Error> {
    sqlx::query_scalar(
        "INSERT INTO api_keys (user_id, name, key_digest, scope, expires_at)
         VALUES ($1, $2, $3, $4, now() + make_interval(days => $5)) RETURNING id",
    )
    .bind(user)
    .bind(name)
    .bind(digest)
    .bind(scope)
    .bind(days)
    .fetch_one(pool)
    .await
}

crate::sql::set_col!(
/// Puts plugin scopes on a key the member just made.
    set_key_scopes, "api_keys", "plugin_scopes", i64, &[String]);

pub async fn keys(pool: &PgPool, user: i64) -> Result<Vec<ApiKey>, Error> {
    sqlx::query_as("SELECT id, name, scope, last_used_at, expires_at, plugin_scopes FROM api_keys WHERE user_id = $1 AND oauth_client IS NULL ORDER BY id DESC")
        .bind(user)
        .fetch_all(pool)
        .await
}

pub async fn delete_key(pool: &PgPool, user: i64, id: i64) -> Result<bool, Error> {
    Ok(sqlx::query("DELETE FROM api_keys WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

/// (user id, scope) for a key, marking it used at most once a minute.
pub async fn key_owner(pool: &PgPool, digest: &str) -> Result<Option<(i64, String)>, Error> {
    let found: Option<(i64, String)> = sqlx::query_as(
        "SELECT k.user_id, k.scope FROM api_keys k JOIN users u ON u.id = k.user_id
         WHERE k.key_digest = $1 AND (k.expires_at IS NULL OR k.expires_at > now())
           AND (u.suspended_until IS NULL OR u.suspended_until < now())",
    )
    .bind(digest)
    .fetch_optional(pool)
    .await?;
    if found.is_some() {
        sqlx::query(
            "UPDATE api_keys SET last_used_at = now() WHERE key_digest = $1
               AND (last_used_at IS NULL OR last_used_at < now() - interval '1 minute')",
        )
        .bind(digest)
        .execute(pool)
        .await?;
    }
    Ok(found)
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Webhook {
    pub id: i64,
    pub url: String,
    pub events: Vec<String>,
    pub active: bool,
    /// Forums the hook is limited to; empty means every forum.
    pub node_ids: Vec<i64>,
}

pub async fn create_webhook(
    pool: &PgPool,
    url: &str,
    secret: &str,
    events: &[String],
    node_ids: &[i64],
) -> Result<i64, Error> {
    // The signing key is encrypted at rest; `due_deliveries` opens it to sign a body.
    sqlx::query_scalar("INSERT INTO webhooks (url, secret, events, node_ids) VALUES ($1, $2, $3, $4) RETURNING id")
        .bind(url)
        .bind(bf_core::secretbox::seal(secret))
        .bind(events)
        .bind(node_ids)
        .fetch_one(pool)
        .await
}

pub async fn webhooks(pool: &PgPool) -> Result<Vec<Webhook>, Error> {
    sqlx::query_as("SELECT id, url, events, active, node_ids FROM webhooks ORDER BY id").fetch_all(pool).await
}

crate::sql::delete_by_id!(delete_webhook, "webhooks");

/// The topic and forums an event's payload belongs to, so a forum-scoped hook can be matched.
fn event_scope(event: &str, payload: &serde_json::Value) -> (Option<i64>, Vec<i64>) {
    let int = |v: &serde_json::Value, k: &str| v.get(k).and_then(serde_json::Value::as_i64);
    let mut nodes: Vec<i64> = int(payload, "node_id").into_iter().collect();
    let mut topic = int(payload, "topic_id");
    if event == "topic.moved" {
        nodes.extend(int(payload, "from"));
        nodes.extend(int(payload, "to"));
    }
    if event == "moderation.action" {
        let detail = payload.get("detail").unwrap_or(&serde_json::Value::Null);
        match payload.get("target_type").and_then(serde_json::Value::as_str) {
            Some("topic") => topic = topic.or_else(|| int(payload, "target_id")),
            Some("post") => topic = topic.or_else(|| int(detail, "topic_id")),
            _ => {}
        }
        if payload.get("action").and_then(serde_json::Value::as_str) == Some("move") {
            nodes.extend(int(detail, "from"));
            nodes.extend(int(detail, "to"));
        }
    }
    (topic, nodes)
}

/// Queues an event for every active webhook subscribed to it. A hook limited to forums gets only
/// events that resolve to one of them; an event with no forum (a registration, a ban) never
/// reaches a scoped hook.
pub async fn enqueue_event(pool: &PgPool, event: &str, payload: &serde_json::Value) -> Result<(), Error> {
    let (topic, nodes) = event_scope(event, payload);
    sqlx::query(
        "INSERT INTO webhook_deliveries (webhook_id, event, payload)
         SELECT id, $1, $2 FROM webhooks
          WHERE active AND ($1 = ANY(events) OR '*' = ANY(events))
            AND (cardinality(node_ids) = 0
                 OR node_ids && (ARRAY(SELECT node_id FROM items WHERE id = $3) || $4::bigint[]))",
    )
    .bind(event)
    .bind(payload)
    .bind(topic)
    .bind(nodes)
    .execute(pool)
    .await
    .map(|_| ())
}

#[derive(Debug, Clone, FromRow)]
pub struct Due {
    pub id: i64,
    pub url: String,
    pub secret: String,
    pub event: String,
    pub payload: serde_json::Value,
    pub attempts: i32,
}

/// Claims up to `limit` deliveries that are due. Each is leased for ten minutes (a batch of
/// twenty at ten seconds apiece fits), so on several nodes a delivery goes to one sender and a
/// crash retries it; `delivery_done` and `delivery_failed` replace the lease with the outcome.
pub async fn due_deliveries(pool: &PgPool, limit: i64) -> Result<Vec<Due>, Error> {
    let mut rows: Vec<Due> = sqlx::query_as(
        "UPDATE webhook_deliveries d SET next_at = now() + interval '10 minutes'
         FROM webhooks w
         WHERE w.id = d.webhook_id
           AND d.id IN (SELECT id FROM webhook_deliveries
                        WHERE status = 'pending' AND next_at <= now() ORDER BY id LIMIT $1 FOR UPDATE SKIP LOCKED)
         RETURNING d.id, w.url, w.secret, d.event, d.payload, d.attempts",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    rows.sort_by_key(|d| d.id);
    let mut out = Vec::with_capacity(rows.len());
    for mut d in rows {
        if let Some(secret) = bf_core::secretbox::open(&d.secret) {
            d.secret = secret;
            out.push(d);
            continue;
        }
        // Signing with the sealed text would send a signature the receiver cannot verify, so
        // the delivery waits for the right key, out of the way of the deliveries behind it.
        tracing::error!(delivery = d.id, "webhook secret cannot be decrypted; check BF_SECRET_KEY");
        sqlx::query("UPDATE webhook_deliveries SET next_at = now() + interval '1 hour' WHERE id = $1")
            .bind(d.id)
            .execute(pool)
            .await?;
    }
    Ok(out)
}

pub async fn delivery_done(pool: &PgPool, id: i64, status: i32) -> Result<(), Error> {
    sqlx::query("UPDATE webhook_deliveries SET status = 'delivered', attempts = attempts + 1, last_status = $2, last_error = NULL WHERE id = $1")
        .bind(id)
        .bind(status)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Records a failure: retried after `retry_secs`, or marked failed when that is `None`.
pub async fn delivery_failed(
    pool: &PgPool,
    id: i64,
    status: Option<i32>,
    error: &str,
    retry_secs: Option<i32>,
) -> Result<(), Error> {
    sqlx::query(
        "UPDATE webhook_deliveries SET attempts = attempts + 1, last_status = $2, last_error = $3,
           status = CASE WHEN $4::int IS NULL THEN 'failed' ELSE 'pending' END,
           next_at = now() + make_interval(secs => coalesce($4, 0)) WHERE id = $1",
    )
    .bind(id)
    .bind(status)
    .bind(error)
    .bind(retry_secs)
    .execute(pool)
    .await
    .map(|_| ())
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Delivery {
    pub id: i64,
    pub webhook_id: i64,
    pub event: String,
    pub status: String,
    pub attempts: i32,
    pub last_status: Option<i32>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub async fn deliveries(pool: &PgPool) -> Result<Vec<Delivery>, Error> {
    sqlx::query_as(
        "SELECT id, webhook_id, event, status, attempts, last_status, last_error, created_at
         FROM webhook_deliveries ORDER BY id DESC LIMIT 100",
    )
    .fetch_all(pool)
    .await
}

/// A webhook's address and signing key (opened), for a test delivery. Inactive hooks too.
pub async fn webhook_target(pool: &PgPool, id: i64) -> Result<Option<(String, String)>, Error> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT url, secret FROM webhooks WHERE id = $1").bind(id).fetch_optional(pool).await?;
    Ok(row.and_then(|(url, sealed)| bf_core::secretbox::open(&sealed).map(|secret| (url, secret))))
}

/// Puts a delivery back in the queue, as if it had just been made. False when there is none.
pub async fn replay_delivery(pool: &PgPool, id: i64) -> Result<bool, Error> {
    Ok(sqlx::query(
        "UPDATE webhook_deliveries SET status = 'pending', attempts = 0, next_at = now(), last_status = NULL, last_error = NULL
         WHERE id = $1",
    )
    .bind(id)
    .execute(pool)
    .await?
    .rows_affected()
        > 0)
}

/// Which of these ids are forums (not categories or other nodes).
pub async fn forum_ids(pool: &PgPool, ids: &[i64]) -> Result<Vec<i64>, Error> {
    sqlx::query_scalar("SELECT id FROM nodes WHERE id = ANY($1) AND kind = 'forum' ORDER BY id")
        .bind(ids)
        .fetch_all(pool)
        .await
}
