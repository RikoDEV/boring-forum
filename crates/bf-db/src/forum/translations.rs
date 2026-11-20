//! Content translations.

use chrono::{DateTime, Utc};
pub use sqlx::Error;
use sqlx::{FromRow, PgPool};

#[derive(Debug, Clone, FromRow)]
pub struct Translation {
    pub target_id: i64,
    pub locale: String,
    pub title: String,
    pub raw: String,
    pub cooked: String,
    pub translator: Option<String>,
    /// Made by the AI, not by a person.
    pub machine: bool,
    pub updated_at: DateTime<Utc>,
}

pub async fn any(pool: &PgPool) -> Result<bool, Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM content_translations)").fetch_one(pool).await
}

/// Translations of some posts into one language.
pub async fn for_posts(pool: &PgPool, posts: &[i64], locale: &str) -> Result<Vec<Translation>, Error> {
    sqlx::query_as(
        "SELECT t.target_id, t.locale, t.title, t.raw, t.cooked, u.username AS translator, t.machine, t.updated_at
         FROM content_translations t LEFT JOIN users u ON u.id = t.translator_id
         WHERE t.kind = 'post' AND t.target_id = ANY($1) AND t.locale = $2
           AND NOT (t.machine AND EXISTS (SELECT 1 FROM post_revisions r
                                          WHERE r.post_id = t.target_id AND r.created_at > t.updated_at))",
    )
    .bind(posts)
    .bind(locale)
    .fetch_all(pool)
    .await
}

/// Every version of one item.
pub async fn of(pool: &PgPool, kind: &str, target: i64) -> Result<Vec<Translation>, Error> {
    sqlx::query_as(
        "SELECT t.target_id, t.locale, t.title, t.raw, t.cooked, u.username AS translator, t.machine, t.updated_at
         FROM content_translations t LEFT JOIN users u ON u.id = t.translator_id
         WHERE t.kind = $1 AND t.target_id = $2 ORDER BY t.locale",
    )
    .bind(kind)
    .bind(target)
    .fetch_all(pool)
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn save(
    pool: &PgPool,
    kind: &str,
    target: i64,
    locale: &str,
    title: &str,
    raw: &str,
    cooked: &str,
    by: i64,
) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO content_translations (kind, target_id, locale, title, raw, cooked, translator_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (kind, target_id, locale) DO UPDATE SET title = $4, raw = $5, cooked = $6, translator_id = $7, machine = false, updated_at = now()",
    )
    .bind(kind)
    .bind(target)
    .bind(locale)
    .bind(title)
    .bind(raw)
    .bind(cooked)
    .bind(by)
    .execute(pool)
    .await
    .map(|_| ())
}

pub async fn delete(pool: &PgPool, kind: &str, target: i64, locale: &str) -> Result<bool, Error> {
    Ok(sqlx::query("DELETE FROM content_translations WHERE kind = $1 AND target_id = $2 AND locale = $3")
        .bind(kind)
        .bind(target)
        .bind(locale)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

/// A translation made by the model. A person's own version is never replaced by it.
#[allow(clippy::too_many_arguments)]
pub async fn save_machine(
    pool: &PgPool,
    target: i64,
    locale: &str,
    title: &str,
    raw: &str,
    cooked: &str,
    by: i64,
) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO content_translations (kind, target_id, locale, title, raw, cooked, translator_id, machine)
         VALUES ('post', $1, $2, $3, $4, $5, $6, true)
         ON CONFLICT (kind, target_id, locale) DO UPDATE SET title = $3, raw = $4, cooked = $5, translator_id = $6,
           updated_at = now() WHERE content_translations.machine",
    )
    .bind(target)
    .bind(locale)
    .bind(title)
    .bind(raw)
    .bind(cooked)
    .bind(by)
    .execute(pool)
    .await
    .map(|_| ())
}

/// One stored version of a post that still matches it: a machine translation made before the
/// post's latest edit counts as missing (it may hold text that was since removed), and
/// `save_machine` then replaces it.
pub async fn one_current(
    pool: &PgPool,
    post: i64,
    locale: &str,
) -> Result<Option<(String, String, String, bool)>, Error> {
    sqlx::query_as(
        "SELECT t.title, t.raw, t.cooked, t.machine FROM content_translations t
         WHERE t.kind = 'post' AND t.target_id = $1 AND t.locale = $2
           AND NOT (t.machine AND EXISTS (SELECT 1 FROM post_revisions r
                                          WHERE r.post_id = t.target_id AND r.created_at > t.updated_at))",
    )
    .bind(post)
    .bind(locale)
    .fetch_optional(pool)
    .await
}

/// One stored version of a post, whoever made it.
pub async fn one(pool: &PgPool, post: i64, locale: &str) -> Result<Option<(String, String, String, bool)>, Error> {
    sqlx::query_as(
        "SELECT title, raw, cooked, machine FROM content_translations WHERE kind = 'post' AND target_id = $1 AND locale = $2",
    )
    .bind(post)
    .bind(locale)
    .fetch_optional(pool)
    .await
}
