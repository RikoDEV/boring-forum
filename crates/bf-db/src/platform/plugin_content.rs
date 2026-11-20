//! What a plugin content app keeps in the core besides its own tables: documents it offers to
//! site search, entries it holds for a moderator, and ranges of its stored files.

use chrono::{DateTime, Utc};
pub use sqlx::Error;
use sqlx::PgPool;

/// One document in site search, as a plugin indexed it.
pub struct SearchDoc<'a> {
    pub plugin: &'a str,
    pub kind: &'a str,
    pub reference: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    pub url: &'a str,
    pub author: Option<&'a str>,
    pub perm: Option<&'a str>,
}

/// Documents a plugin may keep in the index.
pub const MAX_DOCS_PER_PLUGIN: i64 = 50_000;

/// Adds or replaces a document. False when the plugin is at its limit and this one is new.
pub async fn index_doc(pool: &PgPool, d: &SearchDoc<'_>) -> Result<bool, Error> {
    let mut tx = pool.begin().await?;
    let there: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM plugin_search_docs WHERE plugin = $1 AND type = $2 AND ref = $3)")
            .bind(d.plugin)
            .bind(d.kind)
            .bind(d.reference)
            .fetch_one(&mut *tx)
            .await?;
    if !there {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM plugin_search_docs WHERE plugin = $1")
            .bind(d.plugin)
            .fetch_one(&mut *tx)
            .await?;
        if n >= MAX_DOCS_PER_PLUGIN {
            return Ok(false);
        }
    }
    sqlx::query(
        "INSERT INTO plugin_search_docs (plugin, type, ref, title, body, url, author, perm)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         ON CONFLICT (plugin, type, ref) DO UPDATE SET title = EXCLUDED.title, body = EXCLUDED.body,
           url = EXCLUDED.url, author = EXCLUDED.author, perm = EXCLUDED.perm, updated_at = now()",
    )
    .bind(d.plugin)
    .bind(d.kind)
    .bind(d.reference)
    .bind(d.title)
    .bind(d.body)
    .bind(d.url)
    .bind(d.author)
    .bind(d.perm)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(true)
}

pub async fn remove_doc(pool: &PgPool, plugin: &str, kind: &str, reference: &str) -> Result<bool, Error> {
    Ok(sqlx::query("DELETE FROM plugin_search_docs WHERE plugin = $1 AND type = $2 AND ref = $3")
        .bind(plugin)
        .bind(kind)
        .bind(reference)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

/// Bytes the plugin's documents and held entries take, for its storage quota.
pub async fn content_bytes(pool: &PgPool, plugin: &str) -> Result<i64, Error> {
    sqlx::query_scalar(
        "SELECT COALESCE((SELECT sum(octet_length(title) + octet_length(body) + octet_length(url) + 64)
                            FROM plugin_search_docs WHERE plugin = $1), 0)::bigint
              + COALESCE((SELECT sum(octet_length(title) + octet_length(excerpt) + octet_length(url) + 64)
                            FROM plugin_review_items WHERE plugin = $1), 0)::bigint",
    )
    .bind(plugin)
    .fetch_one(pool)
    .await
}

#[derive(Debug, sqlx::FromRow)]
pub struct DocHit {
    pub plugin: String,
    pub kind: String,
    pub title: String,
    pub url: String,
    pub author: Option<String>,
    pub snippet: String,
    pub updated_at: DateTime<Utc>,
}

/// Documents matching `q` that this viewer may find: the entry has no permission key, or the
/// member's groups hold it (administrators always do). `plugins` limits it to enabled plugins.
pub async fn search_docs(
    pool: &PgPool,
    q: &str,
    plugins: &[String],
    user: Option<i64>,
    limit: i64,
) -> Result<Vec<DocHit>, Error> {
    sqlx::query_as(
        "SELECT d.plugin, d.type AS kind, d.title, d.url, d.author, d.updated_at,
                ts_headline('simple', d.body, query, 'MaxFragments=1,MaxWords=30,MinWords=10,StartSel=\"\",StopSel=\"\"') AS snippet
         FROM plugin_search_docs d, websearch_to_tsquery('simple', $1) query
         WHERE d.plugin = ANY($2) AND d.search_vector @@ query
           AND (d.perm IS NULL OR ($3::bigint IS NOT NULL AND (
                EXISTS (SELECT 1 FROM plugin_permission_grants g
                        WHERE g.plugin = d.plugin AND g.perm = d.perm
                          AND (g.group_id = (SELECT primary_group_id FROM users WHERE id = $3)
                               OR g.group_id IN (SELECT group_id FROM user_secondary_groups WHERE user_id = $3)))
                OR EXISTS (SELECT 1 FROM groups g WHERE g.is_admin
                           AND (g.id = (SELECT primary_group_id FROM users WHERE id = $3)
                                OR g.id IN (SELECT group_id FROM user_secondary_groups WHERE user_id = $3)))
           )))
         ORDER BY ts_rank(d.search_vector, query) DESC, d.updated_at DESC LIMIT $4",
    )
    .bind(q)
    .bind(plugins)
    .bind(user)
    .bind(limit)
    .fetch_all(pool)
    .await
}

#[derive(Debug, sqlx::FromRow)]
pub struct ReviewItem {
    pub id: i64,
    pub plugin: String,
    pub kind: String,
    pub reference: String,
    pub title: String,
    pub excerpt: String,
    pub url: String,
    pub submitted_by: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Entries held for review per plugin, so a plugin cannot fill the queue.
pub const MAX_PENDING_PER_PLUGIN: i64 = 500;

/// What a plugin puts in the queue.
pub struct Held<'a> {
    pub plugin: &'a str,
    pub kind: &'a str,
    pub reference: &'a str,
    pub title: &'a str,
    pub excerpt: &'a str,
    pub url: &'a str,
    pub by: Option<i64>,
}

/// Puts an entry in the queue. `Ok(None)` when it is already waiting or the plugin has too many.
pub async fn hold(pool: &PgPool, h: &Held<'_>) -> Result<Option<i64>, Error> {
    sqlx::query_scalar(
        "INSERT INTO plugin_review_items (plugin, type, ref, title, excerpt, url, submitted_by)
         SELECT $1, $2, $3, $4, $5, $6, $7
          WHERE (SELECT count(*) FROM plugin_review_items WHERE plugin = $1 AND status = 'pending') < $8
         ON CONFLICT (plugin, type, ref) WHERE status = 'pending' DO NOTHING RETURNING id",
    )
    .bind(h.plugin)
    .bind(h.kind)
    .bind(h.reference)
    .bind(h.title)
    .bind(h.excerpt)
    .bind(h.url)
    .bind(h.by)
    .bind(MAX_PENDING_PER_PLUGIN)
    .fetch_optional(pool)
    .await
}

pub async fn pending(pool: &PgPool, limit: i64) -> Result<Vec<ReviewItem>, Error> {
    sqlx::query_as(
        "SELECT r.id, r.plugin, r.type AS kind, r.ref AS reference, r.title, r.excerpt, r.url,
                (SELECT username FROM users u WHERE u.id = r.submitted_by) AS submitted_by, r.created_at
         FROM plugin_review_items r JOIN plugins p ON p.id = r.plugin AND p.enabled
         WHERE r.status = 'pending' ORDER BY r.created_at LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// Closes one pending entry; returns (plugin, type, ref) so the plugin can be told, or `None`
/// if it was already decided.
pub async fn decide(pool: &PgPool, id: i64, approve: bool, moderator: i64) -> Result<Option<(String, String, String)>, Error> {
    sqlx::query_as(
        "UPDATE plugin_review_items SET status = CASE WHEN $2 THEN 'approved' ELSE 'rejected' END,
                decided_by = $3, decided_at = now()
         WHERE id = $1 AND status = 'pending' RETURNING plugin, type, ref",
    )
    .bind(id)
    .bind(approve)
    .bind(moderator)
    .fetch_optional(pool)
    .await
}

/// A plugin withdraws its own pending entry (the author deleted it).
pub async fn withdraw(pool: &PgPool, plugin: &str, kind: &str, reference: &str) -> Result<bool, Error> {
    Ok(sqlx::query("DELETE FROM plugin_review_items WHERE plugin = $1 AND type = $2 AND ref = $3 AND status = 'pending'")
        .bind(plugin)
        .bind(kind)
        .bind(reference)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

/// Size and type of a stored file, without its bytes.
pub async fn file_head(pool: &PgPool, plugin: &str, path: &str) -> Result<Option<(String, i64, DateTime<Utc>)>, Error> {
    sqlx::query_as("SELECT content_type, octet_length(data)::bigint, created_at FROM plugin_files WHERE plugin = $1 AND path = $2")
        .bind(plugin)
        .bind(path)
        .fetch_optional(pool)
        .await
}

/// `len` bytes of a stored file from `start` (0-based), without loading the rest.
pub async fn file_slice(pool: &PgPool, plugin: &str, path: &str, start: i64, len: i64) -> Result<Option<Vec<u8>>, Error> {
    sqlx::query_scalar(
        "SELECT substring(data FROM ($3::int + 1) FOR $4::int) FROM plugin_files WHERE plugin = $1 AND path = $2",
    )
    .bind(plugin)
    .bind(path)
    .bind(start)
    .bind(len)
    .fetch_optional(pool)
    .await
}

/// Appends to a stored file in the database, without reading it back. Creates it if new.
/// Returns the new size.
pub async fn file_append(pool: &PgPool, plugin: &str, path: &str, content_type: &str, data: &[u8]) -> Result<i64, Error> {
    sqlx::query_scalar(
        "INSERT INTO plugin_files (plugin, path, content_type, data, size) VALUES ($1, $2, $3, $4, octet_length($4))
         ON CONFLICT (plugin, path) DO UPDATE SET data = plugin_files.data || EXCLUDED.data,
           size = octet_length(plugin_files.data) + octet_length(EXCLUDED.data), content_type = EXCLUDED.content_type
         RETURNING size::bigint",
    )
    .bind(plugin)
    .bind(path)
    .bind(content_type)
    .bind(data)
    .fetch_one(pool)
    .await
}

/// Whether a topic belongs to the plugin: in a forum it declared or created, or opened by its account.
pub async fn topic_owned(pool: &PgPool, plugin: &str, bot: i64, topic: i64) -> Result<bool, Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM items i LEFT JOIN nodes n ON n.id = i.node_id
                        WHERE i.id = $3 AND (n.plugin = $1
                          OR EXISTS (SELECT 1 FROM posts p WHERE p.item_id = i.id AND p.post_number = 1 AND p.author_id = $2)))",
    )
    .bind(plugin)
    .bind(bot)
    .bind(topic)
    .fetch_one(pool)
    .await
}

/// Locks or unlists an owned topic (`None` leaves a flag as it is).
pub async fn set_topic_flags(pool: &PgPool, topic: i64, locked: Option<bool>, listed: Option<bool>) -> Result<(), Error> {
    sqlx::query("UPDATE items SET locked = COALESCE($2, locked), listed = COALESCE($3, listed) WHERE id = $1")
        .bind(topic)
        .bind(locked)
        .bind(listed)
        .execute(pool)
        .await
        .map(|_| ())
}
