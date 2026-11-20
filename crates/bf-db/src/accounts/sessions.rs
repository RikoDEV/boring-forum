//! Sign-in sessions, sudo mode and per-device session records.

use crate::accounts::users::USER_COLS;
use crate::accounts::users::User;
use crate::admin::audit::{Entry, record};
use bf_core::ids::UserId;
use chrono::{DateTime, Utc};
pub use sqlx::Error;
use sqlx::{FromRow, PgPool};
use std::net::IpAddr;

pub async fn create_session(
    pool: &PgPool,
    digest: &str,
    user_id: UserId,
    ip: Option<std::net::IpAddr>,
) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO sessions (token_digest, user_id, expires_at, ip, sudo_until)
         VALUES ($1, $2, now() + interval '30 days', $3, now() + make_interval(mins => $4))",
    )
    .bind(digest)
    .bind(user_id)
    .bind(ip.map(sqlx::types::ipnetwork::IpNetwork::from))
    .bind(SUDO_MINUTES)
    .execute(pool)
    .await
    .map(|_| ())
}

/// A session an administrator opens as a member. It is marked on the row, so it never has sudo
/// (its window starts closed, and `confirm_session` will not open it), whatever cookies the
/// browser holds.
pub async fn create_impersonation_session(
    pool: &PgPool,
    digest: &str,
    user_id: UserId,
    admin_id: i64,
    ip: Option<std::net::IpAddr>,
) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO sessions (token_digest, user_id, expires_at, ip, sudo_until, impersonated, impersonated_by)
         VALUES ($1, $2, now() + interval '1 hour', $3, now(), true, $4)",
    )
    .bind(digest)
    .bind(user_id)
    .bind(ip.map(sqlx::types::ipnetwork::IpNetwork::from))
    .bind(admin_id)
    .execute(pool)
    .await
    .map(|_| ())
}

/// How long a sign-in or a password confirmation unlocks high-value actions.
pub const SUDO_MINUTES: i32 = 30;

/// Replaces a session's token and opens its sudo window, after proving who it is.
pub async fn confirm_session(pool: &PgPool, old: &str, new: &str) -> Result<(), Error> {
    sqlx::query(
        "UPDATE sessions SET token_digest = $2, sudo_until = now() + make_interval(mins => $3)
         WHERE token_digest = $1 AND NOT impersonated",
    )
    .bind(old)
    .bind(new)
    .bind(SUDO_MINUTES)
    .execute(pool)
    .await
    .map(|_| ())
}

/// A new token for the same session, keeping its sudo window, after its rights changed.
pub async fn rotate_session(pool: &PgPool, old: &str, new: &str) -> Result<(), Error> {
    sqlx::query("UPDATE sessions SET token_digest = $2 WHERE token_digest = $1")
        .bind(old)
        .bind(new)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn session_user(pool: &PgPool, digest: &str) -> Result<Option<User>, Error> {
    session_user_idle(pool, digest, 0).await
}

/// `session_user`, and a session unused for more than `idle_minutes` is not a session at all
/// (0 keeps it until it expires). The last use is the one `touch_session` already keeps, so no
/// extra write is made.
pub async fn session_user_idle(pool: &PgPool, digest: &str, idle_minutes: i64) -> Result<Option<User>, Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {USER_COLS}, coalesce(s.sudo_until > now(), false) AND NOT s.impersonated AS sudo,
                s.impersonated, s.impersonated_by FROM sessions s JOIN users u ON u.id = s.user_id
         WHERE s.token_digest = $1 AND s.expires_at > now()
         AND ($2 <= 0 OR s.last_used_at > now() - make_interval(mins => $2::int))
         AND (u.suspended_until IS NULL OR u.suspended_until < now())"
    )))
    .bind(digest)
    .bind(idle_minutes.clamp(0, i64::from(i32::MAX)))
    .fetch_optional(pool)
    .await
}

pub async fn delete_session(pool: &PgPool, digest: &str) -> Result<(), Error> {
    sqlx::query("DELETE FROM sessions WHERE token_digest = $1").bind(digest).execute(pool).await.map(|_| ())
}

#[derive(Debug, Clone, FromRow)]
pub struct SessionRow {
    pub token_digest: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: DateTime<Utc>,
    pub ip: Option<sqlx::types::ipnetwork::IpNetwork>,
    pub user_agent: Option<String>,
    pub country: Option<String>,
}

pub async fn sessions(pool: &PgPool, user_id: UserId) -> Result<Vec<SessionRow>, Error> {
    sqlx::query_as(
        "SELECT token_digest, created_at, last_used_at, ip, user_agent, country FROM sessions
         WHERE user_id = $1 AND expires_at > now() ORDER BY last_used_at DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
}

/// Signs out all of the user's sessions except `keep`.
pub async fn revoke_other_sessions(pool: &PgPool, user_id: UserId, keep: &str) -> Result<u64, Error> {
    Ok(sqlx::query("DELETE FROM sessions WHERE user_id = $1 AND token_digest <> $2")
        .bind(user_id)
        .bind(keep)
        .execute(pool)
        .await?
        .rows_affected())
}

/// Revokes one of the user's own sessions; the user id check stops revoking others'.
pub async fn revoke_session(pool: &PgPool, user_id: UserId, digest: &str) -> Result<bool, Error> {
    Ok(sqlx::query("DELETE FROM sessions WHERE user_id = $1 AND token_digest = $2")
        .bind(user_id)
        .bind(digest)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

/// Has this account signed in from this IP before (any session, past or present)?
pub async fn known_ip(pool: &PgPool, user_id: UserId, ip: IpAddr) -> Result<bool, Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM sessions WHERE user_id = $1 AND ip = $2)
             OR EXISTS (SELECT 1 FROM audit_log WHERE actor_id = $1 AND action = 'login' AND ip = $2)",
    )
    .bind(user_id)
    .bind(sqlx::types::ipnetwork::IpNetwork::from(ip))
    .fetch_one(pool)
    .await
}

pub async fn record_login(pool: &PgPool, user_id: UserId, ip: Option<IpAddr>) -> Result<(), Error> {
    sqlx::query("UPDATE users SET last_seen_at = now() WHERE id = $1").bind(user_id).execute(pool).await?;
    record(pool, Entry::new("login").by(user_id).on(format!("user:{user_id}")).from_ip(ip)).await
}

/// Ends sessions idle for longer than `minutes` (0 keeps them until they expire).
pub async fn expire_idle_sessions(pool: &PgPool, minutes: i64) -> Result<u64, Error> {
    if minutes <= 0 {
        return Ok(0);
    }
    Ok(sqlx::query("DELETE FROM sessions WHERE last_used_at < now() - make_interval(mins => $1::int)")
        .bind(minutes)
        .execute(pool)
        .await?
        .rows_affected())
}

/// Marks a session as used (at most once a minute, to keep writes cheap).
pub async fn touch_session(
    pool: &PgPool,
    digest: &str,
    user_agent: Option<&str>,
    country: Option<&str>,
) -> Result<(), Error> {
    sqlx::query(
        "WITH s AS (UPDATE sessions SET last_used_at = now(), user_agent = coalesce(left($2, 300), user_agent), country = coalesce(left($3, 8), country)
                    WHERE token_digest = $1 AND (last_used_at < now() - interval '1 minute' OR user_agent IS NULL) RETURNING user_id)
         , seen AS (UPDATE users SET last_seen_at = now() WHERE id IN (SELECT user_id FROM s) RETURNING id)
         INSERT INTO user_visits (user_id, day) SELECT id, current_date FROM seen ON CONFLICT DO NOTHING",
    )
    .bind(digest)
    .bind(user_agent)
    .bind(country)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Whether a session still exists and has not expired (the administrator's own, when coming back
/// from impersonating).
pub async fn session_alive(pool: &PgPool, digest: &str) -> Result<bool, Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM sessions WHERE token_digest = $1 AND expires_at > now())")
        .bind(digest)
        .fetch_one(pool)
        .await
}
