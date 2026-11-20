//! Topic summaries: top replies, posters and shared links.

use sqlx::PgPool;

type Error = sqlx::Error;

/// Most-liked posts by a member in forums the viewer can read.
pub async fn top_posts_of(
    pool: &PgPool,
    user: i64,
    nodes: &[i64],
) -> Result<Vec<(i64, String, String, i32, i32)>, Error> {
    sqlx::query_as(
        "SELECT i.id, i.slug, i.title, p.post_number, p.like_count FROM posts p JOIN items i ON i.id = p.item_id
         WHERE p.author_id = $1 AND p.state = 'visible' AND i.state = 'visible' AND i.node_id = ANY($2) AND p.like_count > 0
         ORDER BY p.like_count DESC, p.id DESC LIMIT 20",
    )
    .bind(user)
    .bind(nodes)
    .fetch_all(pool)
    .await
}

/// A topic's most-liked replies, for the summary view.
/// (participants, reactions, characters written) in a topic.
pub async fn topic_totals(pool: &PgPool, item: i64) -> Result<(i64, i64, i64), Error> {
    sqlx::query_as(
        "SELECT count(DISTINCT author_id), coalesce(sum(like_count), 0)::bigint, coalesce(sum(length(raw)), 0)::bigint
         FROM posts WHERE item_id = $1 AND state = 'visible' AND action IS NULL",
    )
    .bind(item)
    .fetch_one(pool)
    .await
}

pub async fn topic_posters(pool: &PgPool, item: i64) -> Result<Vec<(String, i64)>, Error> {
    sqlx::query_as(
        "SELECT u.username, count(*) FROM posts p JOIN users u ON u.id = p.author_id
         WHERE p.item_id = $1 AND p.state = 'visible' AND p.action IS NULL
         GROUP BY u.username ORDER BY count(*) DESC, u.username LIMIT 5",
    )
    .bind(item)
    .fetch_all(pool)
    .await
}

pub async fn topic_shared_links(pool: &PgPool, item: i64) -> Result<Vec<(String, i32)>, Error> {
    sqlx::query_as(
        "SELECT l.url, sum(l.clicks)::int FROM post_links l JOIN posts p ON p.id = l.post_id
         WHERE p.item_id = $1 AND p.state = 'visible' GROUP BY l.url ORDER BY sum(l.clicks) DESC, l.url LIMIT 8",
    )
    .bind(item)
    .fetch_all(pool)
    .await
}

pub async fn top_replies(pool: &PgPool, item: i64) -> Result<Vec<(i32, Option<String>, String, i32)>, Error> {
    sqlx::query_as(
        "SELECT p.post_number, u.username, p.cooked, p.like_count FROM posts p LEFT JOIN users u ON u.id = p.author_id
         WHERE p.item_id = $1 AND p.state = 'visible' AND p.action IS NULL AND p.post_number > 1 AND p.like_count > 0
         ORDER BY p.like_count DESC, p.post_number LIMIT 10",
    )
    .bind(item)
    .fetch_all(pool)
    .await
}

/// The opening post's HTML and likes, for the topic preview card; None when it is not visible.
pub async fn first_post(pool: &PgPool, item: i64) -> Result<Option<(String, i32)>, Error> {
    sqlx::query_as("SELECT cooked, like_count FROM posts WHERE item_id = $1 AND post_number = 1 AND state = 'visible'")
        .bind(item)
        .fetch_optional(pool)
        .await
}
