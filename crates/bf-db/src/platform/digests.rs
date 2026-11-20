//! Email digests.

use chrono::{DateTime, Utc};
pub use sqlx::Error;
use sqlx::{FromRow, PgPool};

crate::sql::set_col!(set_digest, "users", "digest", i64, &str);

pub async fn digest_setting(pool: &PgPool, user_id: i64) -> Result<String, Error> {
    sqlx::query_scalar("SELECT digest FROM users WHERE id = $1").bind(user_id).fetch_one(pool).await
}

#[derive(Debug, Clone, FromRow)]
pub struct DigestUser {
    pub id: i64,
    pub username: String,
    pub email: String,
    pub groups: Vec<i64>,
    pub since: DateTime<Utc>,
    pub is_admin: bool,
    /// `daily`, `weekly` or `monthly`.
    pub digest: String,
}

/// Claims members whose digest is due (daily after 24 h, weekly after 7 days, monthly after 30) by moving their
/// clock forward, so on several nodes each member is handed to one of them. `since` is the clock
/// as it was. Only verified addresses that are not marked bad get mail: an address someone else
/// typed at sign-up must not receive a recurring digest. `release_digest` undoes a claim.
pub async fn due_digests(pool: &PgPool, limit: i64) -> Result<Vec<DigestUser>, Error> {
    sqlx::query_as(
        "WITH due AS (
           SELECT u.id, coalesce(u.last_digest_at, u.joined_at) AS since FROM users u
           WHERE (u.suspended_until IS NULL OR u.suspended_until < now()) AND u.approved AND u.delete_after IS NULL
             AND u.email_verified_at IS NOT NULL AND NOT u.email_bad AND u.email_lower NOT LIKE '%.invalid'
             AND ((u.digest = 'daily' AND coalesce(u.last_digest_at, u.joined_at) < now() - interval '1 day')
               OR (u.digest = 'weekly' AND coalesce(u.last_digest_at, u.joined_at) < now() - interval '7 days')
               OR (u.digest = 'monthly' AND coalesce(u.last_digest_at, u.joined_at) < now() - interval '30 days'))
           ORDER BY u.id LIMIT $1 FOR UPDATE OF u SKIP LOCKED)
         UPDATE users u SET last_digest_at = now() FROM due
         WHERE u.id = due.id
         RETURNING u.id, u.username, u.email,
                array_prepend(u.primary_group_id, coalesce((SELECT array_agg(group_id) FROM user_secondary_groups WHERE user_id = u.id), '{}')) AS groups,
                due.since, (u.primary_group_id = 4) AS is_admin, u.digest",
    )
    .bind(limit)
    .fetch_all(pool)
    .await
}

crate::sql::set_col!(
/// Gives a claim back after a failure, so the digest is tried again on the next pass.
    release_digest, "users", "last_digest_at", i64, DateTime<Utc>);

/// New topics since `since` in the given forums, most replied first: (id, slug, title, replies).
/// Forums the member muted (and their sub-forums) and topics by members they ignore are left out.
pub async fn digest_topics(
    pool: &PgPool,
    user: i64,
    nodes: &[i64],
    since: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<(i64, String, String, i32)>, Error> {
    sqlx::query_as(
        "WITH RECURSIVE muted(id) AS (
           SELECT target_id FROM follows WHERE user_id = $4 AND level = 'mute' AND target_type = 'node'
           UNION SELECT n.id FROM nodes n JOIN muted m ON n.parent_id = m.id)
         SELECT id, slug, title, reply_count FROM items
         WHERE node_id = ANY($1) AND state = 'visible' AND listed AND created_at > $2
           AND node_id NOT IN (SELECT id FROM muted)
           AND NOT EXISTS (SELECT 1 FROM follows f WHERE f.user_id = $4 AND f.level = 'mute'
                           AND f.target_type = 'item' AND f.target_id = items.id)
           AND NOT EXISTS (SELECT 1 FROM ignored_users g WHERE g.user_id = $4 AND g.ignored_user_id = items.author_id)
         ORDER BY reply_count DESC, created_at DESC LIMIT $3",
    )
    .bind(nodes)
    .bind(since)
    .bind(limit)
    .bind(user)
    .fetch_all(pool)
    .await
}

/// The summary of a longer period: topics of any age that were busy since `since`, most active
/// first (new posts, and each like on them counting twice): (id, slug, title, new replies). The
/// same muted forums, muted topics and ignored members are left out as in [`digest_topics`].
pub async fn digest_popular(
    pool: &PgPool,
    user: i64,
    nodes: &[i64],
    since: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<(i64, String, String, i32)>, Error> {
    sqlx::query_as(
        "WITH RECURSIVE muted(id) AS (
           SELECT target_id FROM follows WHERE user_id = $4 AND level = 'mute' AND target_type = 'node'
           UNION SELECT n.id FROM nodes n JOIN muted m ON n.parent_id = m.id)
         SELECT i.id, i.slug, i.title, (count(*) FILTER (WHERE p.post_number > 1))::int AS replies
         FROM items i
         JOIN posts p ON p.item_id = i.id AND p.state = 'visible' AND p.action IS NULL AND p.created_at > $2
              AND NOT EXISTS (SELECT 1 FROM ignored_users g WHERE g.user_id = $4 AND g.ignored_user_id = p.author_id)
         WHERE i.node_id = ANY($1) AND i.state = 'visible' AND i.listed
           AND i.node_id NOT IN (SELECT id FROM muted)
           AND NOT EXISTS (SELECT 1 FROM follows f WHERE f.user_id = $4 AND f.level = 'mute'
                           AND f.target_type = 'item' AND f.target_id = i.id)
         GROUP BY i.id
         ORDER BY (count(*) + 2 * coalesce(sum(p.like_count), 0)) DESC, i.last_posted_at DESC LIMIT $3",
    )
    .bind(nodes)
    .bind(since)
    .bind(limit)
    .bind(user)
    .fetch_all(pool)
    .await
}

/// Members who joined since `since` (approved accounts with a real address).
pub async fn new_members_since(pool: &PgPool, since: DateTime<Utc>) -> Result<i64, Error> {
    sqlx::query_scalar("SELECT count(*) FROM users WHERE joined_at > $1 AND approved AND NOT remote AND email_lower NOT LIKE '%.invalid'")
        .bind(since)
        .fetch_one(pool)
        .await
}

/// Topics the member watches that got replies from others since `since`, in readable and not
/// muted forums: (id, slug, title, new replies).
pub async fn digest_watched(
    pool: &PgPool,
    user: i64,
    nodes: &[i64],
    since: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<(i64, String, String, i64)>, Error> {
    sqlx::query_as(
        "WITH RECURSIVE muted(id) AS (
           SELECT target_id FROM follows WHERE user_id = $1 AND level = 'mute' AND target_type = 'node'
           UNION SELECT n.id FROM nodes n JOIN muted m ON n.parent_id = m.id)
         SELECT i.id, i.slug, i.title, count(*) AS n
         FROM follows f JOIN items i ON i.id = f.target_id
         JOIN posts p ON p.item_id = i.id AND p.state = 'visible' AND p.created_at > $2
              AND p.author_id IS DISTINCT FROM $1 AND p.action IS NULL
              AND NOT EXISTS (SELECT 1 FROM ignored_users g WHERE g.user_id = $1 AND g.ignored_user_id = p.author_id)
         WHERE f.user_id = $1 AND f.target_type = 'item' AND f.level = 'watch'
           AND i.node_id = ANY($3) AND i.state = 'visible'
           AND i.node_id NOT IN (SELECT id FROM muted)
         GROUP BY i.id ORDER BY n DESC, max(p.created_at) DESC LIMIT $4",
    )
    .bind(user)
    .bind(since)
    .bind(nodes)
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// Notifications the member has not read yet.
pub async fn unread_notifications(pool: &PgPool, user: i64) -> Result<i64, Error> {
    sqlx::query_scalar("SELECT count(*) FROM notifications WHERE user_id = $1 AND NOT read")
        .bind(user)
        .fetch_one(pool)
        .await
}

/// A fresh unsubscribe token for the digest of `user`: the link in the mail turns digests off.
pub async fn digest_token(pool: &PgPool, user: i64) -> Result<String, Error> {
    sqlx::query_scalar(
        "INSERT INTO unsubscribe_tokens (token, user_id, kind) VALUES (gen_random_uuid()::text, $1, 'digest')
         RETURNING token",
    )
    .bind(user)
    .fetch_one(pool)
    .await
}

/// Queues the digest mail. The clock was already moved by `due_digests`; a digest with nothing
/// new queues nothing, and the next one covers a fresh period.
pub async fn mark_digest_sent(
    pool: &PgPool,
    to: &str,
    subject: &str,
    body: Option<&str>,
    unsubscribe_url: &str,
) -> Result<(), Error> {
    let Some(b) = body else { return Ok(()) };
    sqlx::query("INSERT INTO mail_outbox (to_addr, subject, body, unsubscribe_url) VALUES ($1, $2, $3, $4)")
        .bind(to)
        .bind(subject)
        .bind(b)
        .bind(unsubscribe_url)
        .execute(pool)
        .await
        .map(|_| ())
}
