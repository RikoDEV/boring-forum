//! Uploads (files stored on disk, metadata here), avatars and composer drafts.

use chrono::{DateTime, Utc};
pub use sqlx::Error;
use sqlx::{FromRow, PgPool};

#[derive(Debug, Clone, FromRow)]
pub struct Upload {
    pub id: i64,
    pub sha256: String,
    pub storage_key: String,
    pub filename: String,
    pub mime: String,
    pub size: i64,
    pub width: Option<i32>,
    pub height: Option<i32>,
    /// Who uploaded it; None once that account is gone.
    pub user_id: Option<i64>,
}

pub struct NewUpload<'a> {
    pub sha256: &'a str,
    pub user_id: i64,
    pub storage_key: &'a str,
    pub filename: &'a str,
    pub mime: &'a str,
    pub size: i64,
    pub width: Option<i32>,
    pub height: Option<i32>,
}

macro_rules! cols {
    () => {
        "id, sha256, storage_key, filename, mime, size, width, height, user_id"
    };
}

/// Records an upload. Storage keys are content-addressed (`{sha256}.{ext}`), so the
/// same bytes uploaded again return the existing row instead of a second copy.
pub async fn insert(pool: &PgPool, u: &NewUpload<'_>) -> Result<Upload, Error> {
    sqlx::query_as(concat!("INSERT INTO uploads (sha256, user_id, storage_key, filename, mime, size, width, height)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         ON CONFLICT (storage_key) DO UPDATE SET storage_key = EXCLUDED.storage_key
         RETURNING ", cols!()))
    .bind(u.sha256)
    .bind(u.user_id)
    .bind(u.storage_key)
    .bind(u.filename)
    .bind(u.mime)
    .bind(u.size)
    .bind(u.width)
    .bind(u.height)
    .fetch_one(pool)
    .await
}

/// Bytes uploaded by a user in the last day, for the per-group daily quota. Counted from every
/// upload the user made, also of files that were already stored from someone else's upload.
pub async fn bytes_today(pool: &PgPool, user_id: i64) -> Result<i64, Error> {
    sqlx::query_scalar(
        "SELECT coalesce(sum(size), 0)::bigint FROM upload_quota WHERE user_id = $1 AND created_at > now() - interval '1 day'",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
}

/// Counts `size` bytes against the user's daily `limit`; false (and nothing counted) if that
/// would go over. Checking and counting happen under one lock per user, so parallel uploads
/// cannot all pass the check before any of them is counted.
pub async fn reserve_quota(pool: &PgPool, user_id: i64, size: i64, limit: i64) -> Result<bool, Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('upload_quota:' || $1::text))")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    let used: i64 = sqlx::query_scalar(
        "SELECT coalesce(sum(size), 0)::bigint FROM upload_quota WHERE user_id = $1 AND created_at > now() - interval '1 day'",
    )
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await?;
    if used.saturating_add(size) > limit {
        return Ok(false);
    }
    sqlx::query("DELETE FROM upload_quota WHERE user_id = $1 AND created_at < now() - interval '2 days'")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO upload_quota (user_id, size) VALUES ($1, $2)")
        .bind(user_id)
        .bind(size)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

pub async fn by_key(pool: &PgPool, key: &str) -> Result<Option<Upload>, Error> {
    sqlx::query_as(concat!("SELECT ", cols!(), " FROM uploads WHERE storage_key = $1"))
        .bind(key)
        .fetch_optional(pool)
        .await
}

/// Where a file is used, as (kind, id): a post, a private message or a chat message. Empty for a
/// file that is only an avatar, an emoji and the like, or not used yet.
pub async fn uses(pool: &PgPool, id: i64) -> Result<Vec<(String, i64)>, Error> {
    sqlx::query_as("SELECT kind, ref_id FROM upload_uses WHERE upload_id = $1 LIMIT 200")
        .bind(id)
        .fetch_all(pool)
        .await
}

/// The forum and state of posts: (post id, forum id, state). A post in a topic that is not
/// visible (deleted, held) has its topic's state: a visible reply there is not public either.
pub async fn post_homes(pool: &PgPool, ids: &[i64]) -> Result<Vec<(i64, i64, String)>, Error> {
    sqlx::query_as(
        "SELECT p.id, i.node_id, CASE WHEN i.state::text = 'visible' THEN p.state::text ELSE i.state::text END
         FROM posts p JOIN items i ON i.id = p.item_id WHERE p.id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(pool)
    .await
}

/// The forums of posts whose earlier versions (edit history) name this file, for a file no
/// current text uses. Empty for an avatar or a cover, which is public whatever else it was.
pub async fn revision_homes(pool: &PgPool, upload_id: i64, key: &str) -> Result<Vec<i64>, Error> {
    sqlx::query_scalar(
        "SELECT DISTINCT i.node_id FROM post_revisions r JOIN posts p ON p.id = r.post_id JOIN items i ON i.id = p.item_id
         WHERE strpos(r.raw, $2) > 0
           AND NOT EXISTS (SELECT 1 FROM users x WHERE x.avatar_upload_id = $1 OR x.cover_upload_id = $1)
         LIMIT 50",
    )
    .bind(upload_id)
    .bind(key)
    .fetch_all(pool)
    .await
}

/// The private conversations that hold these messages.
pub async fn message_homes(pool: &PgPool, ids: &[i64]) -> Result<Vec<i64>, Error> {
    sqlx::query_scalar("SELECT DISTINCT conversation_id FROM conversation_messages WHERE id = ANY($1)")
        .bind(ids)
        .fetch_all(pool)
        .await
}

/// The chat channels that hold these messages.
pub async fn chat_homes(pool: &PgPool, ids: &[i64]) -> Result<Vec<i64>, Error> {
    sqlx::query_scalar("SELECT DISTINCT channel_id FROM chat_messages WHERE id = ANY($1) AND deleted_at IS NULL")
        .bind(ids)
        .fetch_all(pool)
        .await
}

pub async fn count_download(pool: &PgPool, id: i64) -> Result<(), Error> {
    sqlx::query("UPDATE uploads SET download_count = download_count + 1 WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .map(|_| ())
}

crate::sql::set_col!(set_avatar, "users", "avatar_upload_id", i64, Option<i64>);

/// The uploaded avatar's storage key for a username, if any.
/// The address to look a Gravatar up by: only a verified address of an approved account, so a
/// member cannot show someone else's picture by typing their email.
pub async fn gravatar_email(pool: &PgPool, username: &str) -> Result<Option<String>, Error> {
    sqlx::query_scalar(
        "SELECT email FROM users WHERE username_lower = lower($1) AND email_verified_at IS NOT NULL AND approved
           AND NOT remote AND email NOT LIKE '%.invalid' AND profile_visibility = 'everyone'",
    )
    .bind(username)
    .fetch_optional(pool)
    .await
}

pub async fn avatar_key(pool: &PgPool, username: &str) -> Result<Option<String>, Error> {
    sqlx::query_scalar(
        "SELECT up.storage_key FROM users u JOIN uploads up ON up.id = u.avatar_upload_id WHERE u.username_lower = lower($1)",
    )
    .bind(username)
    .fetch_optional(pool)
    .await
}

#[derive(Debug, Clone, FromRow)]
pub struct Draft {
    pub data: serde_json::Value,
    pub updated_at: DateTime<Utc>,
}

pub async fn draft(pool: &PgPool, user_id: i64, context: &str) -> Result<Option<Draft>, Error> {
    sqlx::query_as("SELECT data, updated_at FROM drafts WHERE user_id = $1 AND context = $2")
        .bind(user_id)
        .bind(context)
        .fetch_optional(pool)
        .await
}

pub async fn save_draft(pool: &PgPool, user_id: i64, context: &str, data: &serde_json::Value) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO drafts (user_id, context, data, updated_at) VALUES ($1, $2, $3, now())
         ON CONFLICT (user_id, context) DO UPDATE SET data = EXCLUDED.data, updated_at = now()",
    )
    .bind(user_id)
    .bind(context)
    .bind(data)
    .execute(pool)
    .await?;
    // A member keeps this many drafts; the least recently saved go first.
    sqlx::query(
        "DELETE FROM drafts WHERE user_id = $1 AND context NOT IN
           (SELECT context FROM drafts WHERE user_id = $1 ORDER BY updated_at DESC LIMIT 50)",
    )
    .bind(user_id)
    .execute(pool)
    .await
    .map(|_| ())
}

pub async fn delete_draft(pool: &PgPool, user_id: i64, context: &str) -> Result<(), Error> {
    sqlx::query("DELETE FROM drafts WHERE user_id = $1 AND context = $2")
        .bind(user_id)
        .bind(context)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn drafts(pool: &PgPool, user_id: i64) -> Result<Vec<(String, serde_json::Value, DateTime<Utc>)>, Error> {
    sqlx::query_as("SELECT context, data, updated_at FROM drafts WHERE user_id = $1 ORDER BY updated_at DESC LIMIT 50")
        .bind(user_id)
        .fetch_all(pool)
        .await
}
