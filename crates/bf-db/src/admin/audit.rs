//! Audit log retention and forwarding.
//!
//! The table itself is append-only: the baseline schema puts triggers on it that refuse UPDATE,
//! DELETE and TRUNCATE unless the transaction sets `bf.audit_maintenance`. Only the two
//! retention functions here do that, and both say in SQL exactly which rows they touch.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{FromRow, PgPool};

type Error = sqlx::Error;

/// Clears the IP of audit rows older than `days`, keeping the rest of the event (IPs are
/// kept for a limited time). Returns how many rows lost their IP.
pub async fn clear_old_ips(pool: &PgPool, days: i64) -> Result<u64, Error> {
    let n: i64 = sqlx::query_scalar("SELECT audit_clear_old_ips($1::int)").bind(days as i32).fetch_one(pool).await?;
    Ok(n as u64)
}

/// Deletes audit rows older than `days`. Separate from [`clear_old_ips`] because the two have
/// different horizons: the IP goes early, the event itself is usually kept much longer.
pub async fn prune(pool: &PgPool, days: i64) -> Result<u64, Error> {
    let n: i64 = sqlx::query_scalar("SELECT audit_prune($1::int)").bind(days as i32).fetch_one(pool).await?;
    Ok(n as u64)
}

/// One event as it is shipped to an external log system.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Event {
    pub id: i64,
    pub actor_id: Option<i64>,
    pub action: String,
    pub target: Option<String>,
    pub details: serde_json::Value,
    #[serde(serialize_with = "ip_as_string")]
    pub ip: Option<sqlx::types::ipnetwork::IpNetwork>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// `IpNetwork` has no `Serialize`, and a bare address reads better in a log line than a /32.
fn ip_as_string<S: serde::Serializer>(ip: &Option<sqlx::types::ipnetwork::IpNetwork>, s: S) -> Result<S::Ok, S::Error> {
    match ip {
        Some(ip) => s.serialize_str(&ip.ip().to_string()),
        None => s.serialize_none(),
    }
}

/// The next batch of events after the stored cursor.
pub async fn events_after(pool: &PgPool, after: i64, limit: i64) -> Result<Vec<Event>, Error> {
    sqlx::query_as(
        "SELECT id, actor_id, action, target, details, ip, created_at
         FROM audit_log WHERE id > $1 AND created_at < now() - interval '1 minute' ORDER BY id LIMIT $2",
    )
    .bind(after)
    .bind(limit)
    .fetch_all(pool)
    .await
}

pub async fn forward_cursor(pool: &PgPool) -> Result<i64, Error> {
    sqlx::query_scalar("SELECT last_id FROM audit_forward_state WHERE id = 1").fetch_one(pool).await
}

/// Moves the cursor forward. Only ever increases, so a late or duplicated batch cannot rewind it
/// and cause the same events to be shipped again.
pub async fn set_forward_cursor(pool: &PgPool, last_id: i64) -> Result<(), Error> {
    sqlx::query("UPDATE audit_forward_state SET last_id = GREATEST(last_id, $1), updated_at = now() WHERE id = 1")
        .bind(last_id)
        .execute(pool)
        .await
        .map(|_| ())
}

#[derive(Debug, Serialize, FromRow)]
pub struct AuditRow {
    pub id: i64,
    pub actor: Option<String>,
    pub action: String,
    pub target: Option<String>,
    /// The member a `user:<id>` target names, so the log can link them.
    pub target_name: Option<String>,
    pub details: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

/// Staff action log with optional filters; keyset on id for paging.
pub async fn audit_log(
    pool: &PgPool,
    action: &str,
    actor: &str,
    before: Option<i64>,
    limit: i64,
) -> Result<Vec<AuditRow>, Error> {
    sqlx::query_as(
        "SELECT a.id, u.username AS actor, a.action, a.target, tu.username AS target_name, a.details, a.created_at
         FROM audit_log a LEFT JOIN users u ON u.id = a.actor_id
         LEFT JOIN users tu ON tu.id = CASE WHEN a.target ~ '^user:[0-9]{1,18}$' THEN substr(a.target, 6)::bigint END
         WHERE ($1 = '' OR a.action LIKE $1 || '%') AND ($2 = '' OR u.username_lower = lower($2))
           AND ($3::bigint IS NULL OR a.id < $3)
         ORDER BY a.id DESC LIMIT $4",
    )
    .bind(action)
    .bind(actor)
    .bind(before)
    .bind(limit)
    .fetch_all(pool)
    .await
}

#[derive(Debug, Serialize, FromRow)]
pub struct MailRow {
    pub id: i64,
    pub to_addr: String,
    pub subject: String,
    pub created_at: DateTime<Utc>,
    pub sent_at: Option<DateTime<Utc>>,
    pub attempts: i16,
    pub last_error: Option<String>,
}

/// Recent outgoing mail with delivery status, for the admin mail log.
pub async fn mail_log(pool: &PgPool, limit: i64) -> Result<Vec<MailRow>, Error> {
    sqlx::query_as(
        "SELECT id, to_addr, subject, created_at, sent_at, attempts, last_error FROM mail_outbox ORDER BY id DESC LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// One line of the staff log: who did what to what. This is the only way a line is written (a
/// test fails when another file says `INSERT INTO audit_log`), so what every line carries is
/// decided here. The log is append-only and outlives accounts: personal values (an address, a
/// name) go into `details` masked, never as they are.
#[derive(Debug, Clone)]
pub struct Entry {
    actor: Option<i64>,
    action: String,
    target: Option<String>,
    details: serde_json::Value,
    ip: Option<std::net::IpAddr>,
}

impl Entry {
    /// A line for `action`, such as `user.ban`. Add who, what and the details with the methods.
    pub fn new(action: impl Into<String>) -> Self {
        Self { actor: None, action: action.into(), target: None, details: serde_json::json!({}), ip: None }
    }

    /// The member who did it; none for the system. Takes a plain id, an optional one, or a `UserId`.
    #[must_use]
    pub fn by(mut self, actor: impl Actor) -> Self {
        self.actor = actor.actor_id();
        self
    }

    /// What it was done to, as `kind:id` (`user:5`, `post:12`).
    #[must_use]
    pub fn on(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self
    }

    #[must_use]
    pub fn details(mut self, details: serde_json::Value) -> Self {
        self.details = details;
        self
    }

    /// The address it came from, for the events that keep one (sign-ins); the retention job
    /// clears it after `audit_ip_days`.
    #[must_use]
    pub fn from_ip(mut self, ip: Option<std::net::IpAddr>) -> Self {
        self.ip = ip;
        self
    }
}

/// Whatever names who did something: an id, an optional id, or a member's `UserId`.
pub trait Actor {
    fn actor_id(self) -> Option<i64>;
}

impl Actor for i64 {
    fn actor_id(self) -> Option<i64> {
        Some(self)
    }
}

impl Actor for Option<i64> {
    fn actor_id(self) -> Option<i64> {
        self
    }
}

impl Actor for bf_core::ids::UserId {
    fn actor_id(self) -> Option<i64> {
        Some(self.get())
    }
}

impl Actor for Option<bf_core::ids::UserId> {
    fn actor_id(self) -> Option<i64> {
        self.map(bf_core::ids::UserId::get)
    }
}

/// Writes a line. Takes a pool or a transaction, so a change and its record commit together.
pub async fn record<'e, E: sqlx::PgExecutor<'e>>(db: E, entry: Entry) -> Result<(), Error> {
    sqlx::query("INSERT INTO audit_log (actor_id, action, target, details, ip) VALUES ($1, $2, $3, $4, $5)")
        .bind(entry.actor)
        .bind(entry.action)
        .bind(entry.target)
        .bind(entry.details)
        .bind(entry.ip.map(sqlx::types::ipnetwork::IpNetwork::from))
        .execute(db)
        .await
        .map(|_| ())
}

pub async fn audit(pool: &PgPool, actor: i64, action: &str, target: &str) -> Result<(), Error> {
    record(pool, Entry::new(action).by(actor).on(target)).await
}
