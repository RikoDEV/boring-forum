//! Invitations: codes, validity and who invited whom.

use bf_core::ids::UserId;
use chrono::{DateTime, Utc};
pub use sqlx::Error;
use sqlx::PgPool;

pub async fn create_invite(
    pool: &PgPool,
    digest: &str,
    by: Option<i64>,
    note: Option<&str>,
    days: i32,
) -> Result<i64, Error> {
    sqlx::query_scalar(
        "INSERT INTO invites (code_digest, created_by, note, expires_at)
         VALUES ($1, $2, $3, now() + make_interval(days => $4)) RETURNING id",
    )
    .bind(digest)
    .bind(by)
    .bind(note)
    .bind(days)
    .fetch_one(pool)
    .await
}

pub async fn invite_valid(pool: &PgPool, digest: &str) -> Result<bool, Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM invites WHERE code_digest = $1 AND used_at IS NULL AND expires_at > now())",
    )
    .bind(digest)
    .fetch_one(pool)
    .await
}

/// Takes the invite atomically, so one code can never register two accounts.
pub async fn use_invite(pool: &PgPool, digest: &str, user: i64) -> Result<bool, Error> {
    Ok(sqlx::query("UPDATE invites SET used_at = now(), used_by = $2 WHERE code_digest = $1 AND used_at IS NULL AND expires_at > now()")
        .bind(digest)
        .bind(user)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

pub async fn list_invites(pool: &PgPool) -> Result<Vec<(i64, Option<String>, Option<String>, DateTime<Utc>)>, Error> {
    sqlx::query_as(
        "SELECT i.id, i.note, u.username, i.expires_at FROM invites i LEFT JOIN users u ON u.id = i.used_by
         ORDER BY i.id DESC LIMIT 200",
    )
    .fetch_all(pool)
    .await
}

pub async fn delete_invite(pool: &PgPool, id: i64) -> Result<bool, Error> {
    Ok(sqlx::query("DELETE FROM invites WHERE id = $1 AND used_at IS NULL")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

/// Who created the invite a new member just used.
pub async fn invite_owner(pool: &PgPool, new_member: UserId) -> Result<Option<UserId>, Error> {
    sqlx::query_scalar("SELECT created_by FROM invites WHERE used_by = $1")
        .bind(new_member)
        .fetch_optional(pool)
        .await
        .map(Option::flatten)
}
