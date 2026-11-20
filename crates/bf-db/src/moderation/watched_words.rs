//! Watched words.

pub use sqlx::Error;
use sqlx::{FromRow, PgPool};

#[derive(Debug, Clone, FromRow, serde::Serialize)]
pub struct WatchedWord {
    pub id: i64,
    pub action: String,
    pub pattern: String,
    pub replacement: Option<String>,
}

pub async fn watched_words(pool: &PgPool) -> Result<Vec<WatchedWord>, Error> {
    sqlx::query_as("SELECT id, action, pattern, replacement FROM watched_words ORDER BY action, pattern")
        .fetch_all(pool)
        .await
}

pub async fn add_watched_word(
    pool: &PgPool,
    action: &str,
    pattern: &str,
    replacement: Option<&str>,
) -> Result<i64, Error> {
    sqlx::query_scalar(
        "INSERT INTO watched_words (action, pattern, replacement) VALUES ($1, lower($2), $3)
         ON CONFLICT (action, pattern) DO UPDATE SET replacement = EXCLUDED.replacement RETURNING id",
    )
    .bind(action)
    .bind(pattern)
    .bind(replacement)
    .fetch_one(pool)
    .await
}

/// Adds many words with one action (and replacement). Words that already have this action keep
/// their place and take the new replacement. Returns (added, already there).
pub async fn import_watched_words(
    pool: &PgPool,
    action: &str,
    replacement: Option<&str>,
    patterns: &[String],
) -> Result<(i64, i64), Error> {
    let (added, total): (i64, i64) = sqlx::query_as(
        "WITH done AS (
           INSERT INTO watched_words (action, pattern, replacement)
           SELECT $1, p, $3 FROM unnest($2::text[]) AS t(p)
           ON CONFLICT (action, pattern) DO UPDATE SET replacement = EXCLUDED.replacement
           RETURNING (xmax = 0) AS inserted)
         SELECT count(*) FILTER (WHERE inserted), count(*) FROM done",
    )
    .bind(action)
    .bind(patterns)
    .bind(replacement)
    .fetch_one(pool)
    .await?;
    Ok((added, total - added))
}

crate::sql::delete_by_id!(delete_watched_word, "watched_words");
