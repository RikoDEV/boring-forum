//! Profile visitors.

use chrono::{DateTime, Utc};
use sqlx::PgPool;

type Error = sqlx::Error;

pub async fn set_show_visitors(pool: &PgPool, user: i64, on: bool) -> Result<(), Error> {
    sqlx::query("UPDATE users SET show_visitors = $2 WHERE id = $1").bind(user).bind(on).execute(pool).await?;
    if !on {
        sqlx::query("DELETE FROM profile_visits WHERE profile_id = $1").bind(user).execute(pool).await?;
    }
    Ok(())
}

/// Recorded only for profiles whose owner opted in.
pub async fn count_profile_view(pool: &PgPool, profile: i64) -> Result<(), Error> {
    sqlx::query("UPDATE users SET profile_views = profile_views + 1 WHERE id = $1")
        .bind(profile)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn record_visit(pool: &PgPool, profile: i64, visitor: i64) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO profile_visits (profile_id, visitor_id) SELECT id, $2 FROM users WHERE id = $1 AND show_visitors
         ON CONFLICT (profile_id, visitor_id) DO UPDATE SET visited_at = now()",
    )
    .bind(profile)
    .bind(visitor)
    .execute(pool)
    .await
    .map(|_| ())
}

pub async fn visitors(pool: &PgPool, profile: i64) -> Result<Vec<(String, DateTime<Utc>)>, Error> {
    sqlx::query_as(
        "SELECT u.username, v.visited_at FROM profile_visits v JOIN users u ON u.id = v.visitor_id
         WHERE v.profile_id = $1 ORDER BY v.visited_at DESC LIMIT 50",
    )
    .bind(profile)
    .fetch_all(pool)
    .await
}
