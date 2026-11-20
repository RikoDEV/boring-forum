//! Reading state: read positions, views and topic timelines.

pub use sqlx::Error;
use sqlx::PgPool;

/// The viewer's read position in a topic (0 when never read).
pub async fn read_position(pool: &PgPool, user_id: i64, item_id: i64) -> Result<i32, Error> {
    sqlx::query_scalar(
        "SELECT coalesce((SELECT last_read_post_number FROM read_markers WHERE user_id = $1 AND item_id = $2), 0)",
    )
    .bind(user_id)
    .bind(item_id)
    .fetch_one(pool)
    .await
}

/// Adds batched view counts, `(item_id, views)` pairs, in one statement.
pub async fn add_views(pool: &PgPool, ids: &[i64], counts: &[i32]) -> Result<(), Error> {
    sqlx::query(
        "UPDATE items SET view_count = view_count + v.c
         FROM unnest($1::bigint[], $2::int[]) AS v(id, c) WHERE items.id = v.id",
    )
    .bind(ids)
    .bind(counts)
    .execute(pool)
    .await
    .map(|_| ())
}

pub async fn mark_read(pool: &PgPool, user_id: i64, item_id: i64, post_number: i32) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO read_markers (user_id, item_id, last_read_post_number) VALUES ($1, $2, $3)
         ON CONFLICT (user_id, item_id) DO UPDATE
         SET last_read_post_number = greatest(read_markers.last_read_post_number, EXCLUDED.last_read_post_number)",
    )
    .bind(user_id)
    .bind(item_id)
    .bind(post_number)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Up to about 200 evenly spaced (post number, time) points for the timeline scrubber.
pub async fn topic_dates(pool: &PgPool, topic_id: i64) -> Result<Vec<(i32, chrono::DateTime<chrono::Utc>)>, Error> {
    sqlx::query_as(
        "SELECT post_number, created_at FROM posts p WHERE item_id = $1 AND p.state = 'visible' AND (post_number - 1) % greatest(1,
           (SELECT highest_post_number FROM items WHERE id = $1) / 200) = 0 ORDER BY post_number LIMIT 400",
    )
    .bind(topic_id)
    .fetch_all(pool)
    .await
}
