//! Sitemap entries and RSS, Atom and JSON feeds.

use chrono::{DateTime, Utc};
pub use sqlx::Error;
use sqlx::{FromRow, PgPool};

/// `limit` topics from the `offset`th, newest first: one sitemap file's share.
pub async fn sitemap_topics(
    pool: &PgPool,
    visible_nodes: &[i64],
    offset: i64,
    limit: i64,
) -> Result<Vec<(i64, String, DateTime<Utc>)>, Error> {
    sqlx::query_as(
        "SELECT id, slug, last_posted_at FROM items WHERE state = 'visible' AND listed AND node_id = ANY($1)
         ORDER BY id DESC OFFSET $2 LIMIT $3",
    )
    .bind(visible_nodes)
    .bind(offset)
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// How many topics the sitemap lists.
pub async fn sitemap_topic_count(pool: &PgPool, visible_nodes: &[i64]) -> Result<i64, Error> {
    sqlx::query_scalar("SELECT count(*) FROM items WHERE state = 'visible' AND listed AND node_id = ANY($1)")
        .bind(visible_nodes)
        .fetch_one(pool)
        .await
}

/// One feed entry. Only ever filled from forums guests can read.
#[derive(Debug, Clone, FromRow)]
pub struct FeedRow {
    pub id: i64,
    pub slug: String,
    pub title: String,
    pub number: i32,
    pub cooked: String,
    pub created_at: DateTime<Utc>,
}

/// The newest topics in some forums, optionally with one tag.
pub async fn feed_topics(pool: &PgPool, nodes: &[i64], tag: Option<&str>) -> Result<Vec<FeedRow>, Error> {
    sqlx::query_as(
        "SELECT i.id, i.slug, i.title, p.post_number AS number, p.cooked, p.created_at
         FROM items i JOIN posts p ON p.item_id = i.id AND p.post_number = 1
         WHERE i.node_id = ANY($1) AND i.state = 'visible' AND i.listed AND p.state = 'visible'
           AND ($2::text IS NULL OR EXISTS (SELECT 1 FROM item_tags it JOIN tags tg ON tg.id = it.tag_id
                                            WHERE it.item_id = i.id AND tg.slug = $2))
         ORDER BY p.created_at DESC LIMIT 30",
    )
    .bind(nodes)
    .bind(tag)
    .fetch_all(pool)
    .await
}

/// The newest posts of one topic; empty if guests cannot read it.
pub async fn feed_posts(pool: &PgPool, item: i64, nodes: &[i64]) -> Result<Vec<FeedRow>, Error> {
    sqlx::query_as(
        "SELECT i.id, i.slug, i.title, p.post_number AS number, p.cooked, p.created_at
         FROM items i JOIN posts p ON p.item_id = i.id
         WHERE i.id = $1 AND i.node_id = ANY($2) AND i.state = 'visible' AND i.listed
           AND p.state = 'visible' AND p.action IS NULL
         ORDER BY p.post_number DESC LIMIT 30",
    )
    .bind(item)
    .bind(nodes)
    .fetch_all(pool)
    .await
}

/// Tags with at least one listed topic in the forums guests can read: (slug, newest activity).
pub async fn sitemap_tags(pool: &PgPool, visible_nodes: &[i64]) -> Result<Vec<(String, DateTime<Utc>)>, Error> {
    sqlx::query_as(
        "SELECT tg.slug, max(i.last_posted_at)
         FROM tags tg JOIN item_tags it ON it.tag_id = tg.id
         JOIN items i ON i.id = it.item_id AND i.state = 'visible' AND i.listed AND i.node_id = ANY($1)
         GROUP BY tg.slug ORDER BY tg.slug LIMIT 5000",
    )
    .bind(visible_nodes)
    .fetch_all(pool)
    .await
}

/// Members whose profile guests may open and search engines may list: public to everyone,
/// approved, not suspended, and with posts (a profile without them is `noindex`).
/// Usernames, the most active first. No last-seen time is given out as `lastmod`.
pub async fn sitemap_profiles(pool: &PgPool, limit: i64) -> Result<Vec<String>, Error> {
    sqlx::query_scalar(
        "SELECT username FROM users
         WHERE profile_visibility = 'everyone' AND approved AND post_count > 0
           AND coalesce(suspended_until <= now(), true)
         ORDER BY post_count DESC, id LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await
}
