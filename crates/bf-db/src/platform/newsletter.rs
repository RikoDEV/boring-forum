//! Newsletter opt-ins and their sync state.

use chrono::{DateTime, Utc};
pub use sqlx::Error;
use sqlx::PgPool;

pub async fn subscribed(pool: &PgPool, user: i64) -> Result<bool, Error> {
    sqlx::query_scalar("SELECT newsletter FROM users WHERE id = $1").bind(user).fetch_one(pool).await
}

/// Sets the choice; a change marks the member for the next sync.
pub async fn set(pool: &PgPool, user: i64, on: bool) -> Result<(), Error> {
    sqlx::query("UPDATE users SET newsletter = $2, newsletter_changed_at = now() WHERE id = $1 AND newsletter <> $2")
        .bind(user)
        .bind(on)
        .execute(pool)
        .await
        .map(|_| ())
}

/// An unsubscribe that came from Mailchimp: already true there, so nothing to send back.
pub async fn unsubscribe_email(pool: &PgPool, email: &str) -> Result<u64, Error> {
    Ok(sqlx::query(
        "UPDATE users SET newsletter = false, newsletter_changed_at = now(), newsletter_synced_at = now()
         WHERE email_lower = lower($1) AND newsletter",
    )
    .bind(email)
    .execute(pool)
    .await?
    .rows_affected())
}

/// Members whose choice changed since the last sync: (id, email, username, subscribed, changed).
pub async fn pending(pool: &PgPool, limit: i64) -> Result<Vec<(i64, String, String, bool, DateTime<Utc>)>, Error> {
    sqlx::query_as(
        "SELECT id, email, username, newsletter, newsletter_changed_at FROM users
         WHERE newsletter_changed_at IS NOT NULL AND (newsletter_synced_at IS NULL OR newsletter_synced_at < newsletter_changed_at)
           AND email_verified
         ORDER BY newsletter_changed_at LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await
}

crate::sql::set_col!(
/// Marks a member synced as of the change that was sent, so a newer change is sent again.
    synced, "users", "newsletter_synced_at", i64, DateTime<Utc>);

/// (subscribed, waiting to sync), for the admin page.
pub async fn counts(pool: &PgPool) -> Result<(i64, i64), Error> {
    sqlx::query_as(
        "SELECT count(*) FILTER (WHERE newsletter),
                count(*) FILTER (WHERE newsletter_changed_at IS NOT NULL
                                   AND (newsletter_synced_at IS NULL OR newsletter_synced_at < newsletter_changed_at))
         FROM users",
    )
    .fetch_one(pool)
    .await
}
