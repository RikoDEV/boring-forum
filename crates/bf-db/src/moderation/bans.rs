//! IP and email bans, flood checks and new-device detection.

use crate::admin::audit::{Entry, record};
use chrono::{DateTime, Utc};
use serde::Serialize;
pub use sqlx::Error;
use sqlx::{FromRow, PgPool};
use std::net::IpAddr;

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Ban {
    pub id: i64,
    pub kind: String,
    pub value: String,
    pub reason: Option<String>,
    pub created_at: DateTime<Utc>,
    /// After this moment the ban no longer applies. `None`: until lifted.
    pub expires_at: Option<DateTime<Utc>>,
}

macro_rules! live {
    () => {
        "(expires_at IS NULL OR expires_at > now())"
    };
}

pub async fn list(pool: &PgPool) -> Result<Vec<Ban>, Error> {
    sqlx::query_as(concat!(
        "SELECT id, kind, value, reason, created_at, expires_at FROM bans WHERE ",
        live!(),
        " ORDER BY id DESC"
    ))
    .fetch_all(pool)
    .await
}

pub async fn add(
    pool: &PgPool,
    kind: &str,
    value: &str,
    reason: Option<&str>,
    actor: Option<i64>,
) -> Result<i64, Error> {
    add_until(pool, kind, value, reason, actor, None).await
}

/// A ban that lapses at `expires` (`None`: until lifted). Banning again replaces reason and expiry.
pub async fn add_until(
    pool: &PgPool,
    kind: &str,
    value: &str,
    reason: Option<&str>,
    actor: Option<i64>,
    expires: Option<DateTime<Utc>>,
) -> Result<i64, Error> {
    let mut tx = pool.begin().await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO bans (kind, value, reason, created_by, expires_at) VALUES ($1, lower($2), $3, $4, $5)
         ON CONFLICT (kind, value) DO UPDATE SET reason = EXCLUDED.reason, expires_at = EXCLUDED.expires_at RETURNING id",
    )
    .bind(kind)
    .bind(value)
    .bind(reason)
    .bind(actor)
    .bind(expires)
    .fetch_one(&mut *tx)
    .await?;
    record(
        &mut *tx,
        Entry::new("ban.add")
            .by(actor)
            .on(format!("ban:{id}"))
            .details(serde_json::json!({ "kind": kind, "value": value, "expires_at": expires })),
    )
    .await?;
    tx.commit().await?;
    Ok(id)
}

crate::sql::delete_by_id!(remove, "bans");

/// Is this IP inside a banned address or network?
pub async fn ip_banned(pool: &PgPool, ip: IpAddr) -> Result<bool, Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM bans WHERE kind = 'ip' AND $1::inet <<= value::inet AND (expires_at IS NULL OR expires_at > now()))
           AND NOT EXISTS (SELECT 1 FROM bans WHERE kind = 'ip_allow' AND $1::inet <<= value::inet)")
        .bind(sqlx::types::ipnetwork::IpNetwork::from(ip))
        .fetch_one(pool)
        .await
}

/// Is this address on the allow list? Such addresses are never banned or capped.
pub async fn ip_allowed(pool: &PgPool, ip: IpAddr) -> Result<bool, Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM bans WHERE kind = 'ip_allow' AND $1::inet <<= value::inet)")
        .bind(sqlx::types::ipnetwork::IpNetwork::from(ip))
        .fetch_one(pool)
        .await
}

/// Does this address or network touch the allow list: inside an allowed network, or holding one?
pub async fn allow_listed(pool: &PgPool, net: sqlx::types::ipnetwork::IpNetwork) -> Result<bool, Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM bans WHERE kind = 'ip_allow' AND ($1::inet <<= value::inet OR value::inet <<= $1::inet))",
    )
    .bind(net)
    .fetch_one(pool)
    .await
}

/// Is this email banned, either exactly or by `@domain` / `*@domain`?
pub async fn email_banned(pool: &PgPool, email: &str) -> Result<bool, Error> {
    let lower = email.trim().to_lowercase();
    let domain = lower.rsplit_once('@').map(|(_, d)| d.to_owned()).unwrap_or_default();
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM bans WHERE kind = 'email' AND (expires_at IS NULL OR expires_at > now())
           AND (value = $1 OR value = '@' || $2 OR value = '*@' || $2))",
    )
    .bind(&lower)
    .bind(&domain)
    .fetch_one(pool)
    .await
}

pub async fn asn_banned(pool: &PgPool, asn: &str) -> Result<bool, Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM bans WHERE kind = 'asn' AND value = $1 AND (expires_at IS NULL OR expires_at > now()))")
        .bind(asn)
        .fetch_one(pool)
        .await
}
