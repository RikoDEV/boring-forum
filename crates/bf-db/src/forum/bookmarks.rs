//! Bookmarks and bookmark reminders.

use chrono::{DateTime, Utc};
use sqlx::PgPool;

type Error = sqlx::Error;

/// Toggles a bookmark; returns whether the post is now bookmarked.
pub async fn toggle_bookmark(pool: &PgPool, user_id: i64, post_id: i64) -> Result<bool, Error> {
    let removed = sqlx::query("DELETE FROM bookmarks WHERE user_id = $1 AND post_id = $2")
        .bind(user_id)
        .bind(post_id)
        .execute(pool)
        .await?
        .rows_affected()
        > 0;
    if !removed {
        sqlx::query("INSERT INTO bookmarks (user_id, post_id) VALUES ($1, $2)")
            .bind(user_id)
            .bind(post_id)
            .execute(pool)
            .await?;
    }
    Ok(!removed)
}

pub async fn bookmarked_posts(pool: &PgPool, user_id: i64, post_ids: &[i64]) -> Result<Vec<i64>, Error> {
    sqlx::query_scalar("SELECT post_id FROM bookmarks WHERE user_id = $1 AND post_id = ANY($2)")
        .bind(user_id)
        .bind(post_ids)
        .fetch_all(pool)
        .await
}

/// A member's bookmarks in forums they can still read: (item, slug, title, number, when).
pub async fn bookmarks(
    pool: &PgPool,
    user_id: i64,
    nodes: &[i64],
) -> Result<Vec<(i64, String, String, i32, DateTime<Utc>)>, Error> {
    sqlx::query_as(
        "SELECT i.id, i.slug, i.title, p.post_number, b.created_at
         FROM bookmarks b JOIN posts p ON p.id = b.post_id JOIN items i ON i.id = p.item_id
         WHERE b.user_id = $1 AND i.node_id = ANY($2) AND p.state = 'visible' AND i.state = 'visible'
         ORDER BY b.created_at DESC LIMIT 100",
    )
    .bind(user_id)
    .bind(nodes)
    .fetch_all(pool)
    .await
}

/// Bookmarks the post if needed and sets when to remind the member.
pub async fn set_reminder(pool: &PgPool, user: i64, post: i64, seconds: i64) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO bookmarks (user_id, post_id, remind_at)
         VALUES ($1, $2, now() + make_interval(secs => $3::double precision))
         ON CONFLICT (user_id, post_id) DO UPDATE SET remind_at = EXCLUDED.remind_at",
    )
    .bind(user)
    .bind(post)
    .bind(seconds as f64)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Takes reminders that are due, so each is sent once: (member, post).
pub async fn take_due_reminders(pool: &PgPool, limit: i64) -> Result<Vec<(i64, i64)>, Error> {
    sqlx::query_as(
        "UPDATE bookmarks b SET remind_at = NULL
         FROM (SELECT user_id, post_id FROM bookmarks WHERE remind_at <= now() ORDER BY remind_at LIMIT $1) d
         WHERE b.user_id = d.user_id AND b.post_id = d.post_id AND b.remind_at <= now()
         RETURNING b.user_id, b.post_id",
    )
    .bind(limit)
    .fetch_all(pool)
    .await
}
