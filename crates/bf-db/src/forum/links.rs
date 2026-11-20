//! Links in posts: click counts, download counts and cached link previews.

use sqlx::{FromRow, PgPool};

type Error = sqlx::Error;

/// Makes `links` the links of a post, for edits as well as new posts: a link the text no longer
/// has goes away, one it still has keeps its click count, and a new one is added.
pub async fn record_links(pool: &PgPool, item_id: i64, number: i32, links: &[String]) -> Result<(), Error> {
    sqlx::query(
        "DELETE FROM post_links l USING posts p
         WHERE p.id = l.post_id AND p.item_id = $1 AND p.post_number = $2 AND NOT (l.url = ANY($3::text[]))",
    )
    .bind(item_id)
    .bind(number)
    .bind(links)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO post_links (post_id, url)
         SELECT p.id, u FROM posts p, unnest($3::text[]) AS u WHERE p.item_id = $1 AND p.post_number = $2
         ON CONFLICT DO NOTHING",
    )
    .bind(item_id)
    .bind(number)
    .bind(links)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Where these URLs were already posted, in forums the viewer can read: (url, item, slug, title, number).
pub async fn links_seen(
    pool: &PgPool,
    urls: &[String],
    nodes: &[i64],
) -> Result<Vec<(String, i64, String, String, i32)>, Error> {
    sqlx::query_as(
        "SELECT DISTINCT ON (l.url) l.url, i.id, i.slug, i.title, p.post_number
         FROM post_links l JOIN posts p ON p.id = l.post_id JOIN items i ON i.id = p.item_id
         WHERE l.url = ANY($1) AND i.node_id = ANY($2) AND p.state = 'visible' AND i.state = 'visible' AND i.listed
         ORDER BY l.url, p.id",
    )
    .bind(urls)
    .bind(nodes)
    .fetch_all(pool)
    .await
}

/// Recorded link clicks in a topic's visible posts: (post id, url, clicks).
pub async fn link_clicks(pool: &PgPool, item: i64) -> Result<Vec<(i64, String, i32)>, Error> {
    sqlx::query_as(
        "SELECT l.post_id, l.url, l.clicks FROM post_links l JOIN posts p ON p.id = l.post_id
         WHERE p.item_id = $1 AND p.state = 'visible' AND l.clicks > 0 LIMIT 500",
    )
    .bind(item)
    .fetch_all(pool)
    .await
}

pub async fn count_link_click(pool: &PgPool, post: i64, url: &str) -> Result<(), Error> {
    sqlx::query("UPDATE post_links SET clicks = clicks + 1 WHERE post_id = $1 AND url = $2")
        .bind(post)
        .bind(url)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Download counts for attachments by storage key.
pub async fn download_counts(pool: &PgPool, keys: &[String]) -> Result<Vec<(String, i32)>, Error> {
    sqlx::query_as(
        "SELECT storage_key, max(download_count) FROM uploads WHERE storage_key = ANY($1) GROUP BY storage_key",
    )
    .bind(keys)
    .fetch_all(pool)
    .await
}

#[derive(Debug, Clone, Default, FromRow)]
pub struct Preview {
    pub ok: bool,
    pub title: String,
    pub description: String,
    pub image: String,
    pub site: String,
}

/// Whether a visible post in one of `nodes` (the forums guests can read) links to `url`: only
/// those addresses are fetched for a preview, so a private or deleted post cannot make the
/// server fetch for anyone.
pub async fn is_posted_link(pool: &PgPool, url: &str, nodes: &[i64]) -> Result<bool, Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM post_links l JOIN posts p ON p.id = l.post_id JOIN items i ON i.id = p.item_id
         WHERE l.url = $1 AND i.node_id = ANY($2) AND p.state = 'visible' AND i.state = 'visible' AND i.listed)",
    )
    .bind(url)
    .bind(nodes)
    .fetch_one(pool)
    .await
}

/// A good preview is kept for a week, a failed attempt for a day.
pub async fn cached(pool: &PgPool, url: &str) -> Result<Option<Preview>, Error> {
    sqlx::query_as(
        "SELECT ok, title, description, image, site FROM link_previews
         WHERE url = $1 AND fetched_at > now() - CASE WHEN ok THEN interval '7 days' ELSE interval '1 day' END",
    )
    .bind(url)
    .fetch_optional(pool)
    .await
}

pub async fn store(pool: &PgPool, url: &str, p: &Preview) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO link_previews (url, ok, title, description, image, site) VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (url) DO UPDATE SET ok = $2, title = $3, description = $4, image = $5, site = $6, fetched_at = now()",
    )
    .bind(url)
    .bind(p.ok)
    .bind(&p.title)
    .bind(&p.description)
    .bind(&p.image)
    .bind(&p.site)
    .execute(pool)
    .await
    .map(|_| ())
}
