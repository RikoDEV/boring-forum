//! Permalinks: redirects for addresses the forum does not know (a forum moved here from elsewhere).

use chrono::{DateTime, Utc};
use serde::Serialize;
pub use sqlx::Error;
use sqlx::{FromRow, PgPool};

/// A redirect as stored: the old address, and where it points.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Permalink {
    pub id: i64,
    pub url: String,
    /// `forum`, `topic`, `post`, `tag`, `member` or `url`.
    pub kind: String,
    /// What the first three point at.
    pub target_id: Option<i64>,
    /// What the others point at: a tag's address name, a member's name, or an address.
    pub target: Option<String>,
    pub created_at: DateTime<Utc>,
}

macro_rules! cols {
    () => {
        "id, url, kind, target_id, target, created_at"
    };
}

/// The permalinks whose address or target contains `search`, by address.
pub async fn list(pool: &PgPool, search: &str, limit: i64) -> Result<Vec<Permalink>, Error> {
    sqlx::query_as(concat!("SELECT ", cols!(), " FROM permalinks
         WHERE $1 = '' OR url ILIKE '%' || $1 || '%' OR target ILIKE '%' || $1 || '%' OR target_id::text = $1
         ORDER BY lower(url) LIMIT $2"))
    .bind(search)
    .bind(limit)
    .fetch_all(pool)
    .await
}

pub async fn count(pool: &PgPool) -> Result<i64, Error> {
    sqlx::query_scalar("SELECT count(*) FROM permalinks").fetch_one(pool).await
}

/// The permalink for one of these addresses (already normalised), the longest first: an address
/// with its query string beats the same one without.
pub async fn find(pool: &PgPool, urls: &[String]) -> Result<Option<Permalink>, Error> {
    sqlx::query_as(concat!("SELECT ", cols!(), " FROM permalinks WHERE lower(url) = ANY($1) ORDER BY length(url) DESC LIMIT 1"))
    .bind(urls.iter().map(|u| u.to_lowercase()).collect::<Vec<_>>())
    .fetch_optional(pool)
    .await
}

pub async fn insert(pool: &PgPool, url: &str, kind: &str, target_id: Option<i64>, target: Option<&str>) -> Result<Permalink, Error> {
    sqlx::query_as(concat!("INSERT INTO permalinks (url, kind, target_id, target) VALUES ($1, $2, $3, $4) RETURNING ", cols!()))
    .bind(url)
    .bind(kind)
    .bind(target_id)
    .bind(target)
    .fetch_one(pool)
    .await
}

pub async fn update(pool: &PgPool, id: i64, url: &str, kind: &str, target_id: Option<i64>, target: Option<&str>) -> Result<bool, Error> {
    Ok(sqlx::query("UPDATE permalinks SET url = $2, kind = $3, target_id = $4, target = $5 WHERE id = $1")
        .bind(id)
        .bind(url)
        .bind(kind)
        .bind(target_id)
        .bind(target)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

crate::sql::delete_by_id!(delete, "permalinks");

pub async fn url_taken(pool: &PgPool, url: &str, except: Option<i64>) -> Result<bool, Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM permalinks WHERE lower(url) = lower($1) AND ($2::bigint IS NULL OR id <> $2))")
        .bind(url)
        .bind(except)
        .fetch_one(pool)
        .await
}

/// (id, slug, title) of the topics among `ids` that exist.
pub async fn topics(pool: &PgPool, ids: &[i64]) -> Result<Vec<(i64, String, String)>, Error> {
    sqlx::query_as("SELECT id, slug, title FROM items WHERE id = ANY($1) AND deleted_at IS NULL")
        .bind(ids)
        .fetch_all(pool)
        .await
}

/// (post id, topic id, topic title) of the posts among `ids` that exist.
pub async fn posts(pool: &PgPool, ids: &[i64]) -> Result<Vec<(i64, i64, String)>, Error> {
    sqlx::query_as(
        "SELECT p.id, i.id, i.title FROM posts p JOIN items i ON i.id = p.item_id
         WHERE p.id = ANY($1) AND p.deleted_at IS NULL AND i.deleted_at IS NULL",
    )
    .bind(ids)
    .fetch_all(pool)
    .await
}

pub async fn tag_exists(pool: &PgPool, slug: &str) -> Result<bool, Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM tags WHERE slug = $1)").bind(slug).fetch_one(pool).await
}

/// A tag's address name, from its name or its address name.
pub async fn tag_slug(pool: &PgPool, name: &str) -> Result<Option<String>, Error> {
    sqlx::query_scalar("SELECT slug FROM tags WHERE slug = lower($1) OR lower(name) = lower($1) ORDER BY (slug = lower($1)) DESC LIMIT 1")
        .bind(name)
        .fetch_optional(pool)
        .await
}

/// A member's name as written on their account, if there is one.
pub async fn member_name(pool: &PgPool, name: &str) -> Result<Option<String>, Error> {
    sqlx::query_scalar("SELECT username FROM users WHERE username_lower = lower($1)").bind(name).fetch_optional(pool).await
}
