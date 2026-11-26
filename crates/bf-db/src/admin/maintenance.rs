//! Maintenance: recounts, re-cooking, retention and the error and task logs.

use crate::moderation::tools::StaffAction;
use crate::moderation::tools::audit;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{FromRow, PgPool};

type Error = sqlx::Error;

/// Nothing keeps the upload `u`: no avatar, cover, text, draft, emoji, widget or setting names it.
macro_rules! unused {
    () => {
        "AND NOT EXISTS (SELECT 1 FROM users x WHERE x.avatar_upload_id = u.id OR x.cover_upload_id = u.id)
             AND NOT EXISTS (SELECT 1 FROM upload_uses w WHERE w.upload_id = u.id)
             AND NOT EXISTS (SELECT 1 FROM posts p WHERE p.raw LIKE '%' || u.storage_key || '%')
             AND NOT EXISTS (SELECT 1 FROM post_revisions r WHERE r.raw LIKE '%' || u.storage_key || '%')
             AND NOT EXISTS (SELECT 1 FROM drafts d WHERE d.data::text LIKE '%' || u.storage_key || '%')
             AND NOT EXISTS (SELECT 1 FROM users x WHERE x.about_md LIKE '%' || u.storage_key || '%' OR x.signature_md LIKE '%' || u.storage_key || '%')
             AND NOT EXISTS (SELECT 1 FROM conversation_messages m WHERE m.raw LIKE '%' || u.storage_key || '%')
             AND NOT EXISTS (SELECT 1 FROM chat_messages m WHERE m.raw LIKE '%' || u.storage_key || '%')
             AND NOT EXISTS (SELECT 1 FROM member_statuses s WHERE s.body LIKE '%' || u.storage_key || '%')
             AND NOT EXISTS (SELECT 1 FROM member_status_replies s WHERE s.body LIKE '%' || u.storage_key || '%')
             AND NOT EXISTS (SELECT 1 FROM custom_emoji e WHERE e.url LIKE '%' || u.storage_key || '%')
             AND NOT EXISTS (SELECT 1 FROM widgets w WHERE w.config::text LIKE '%' || u.storage_key || '%')
             AND NOT EXISTS (SELECT 1 FROM announcements a WHERE a.body LIKE '%' || u.storage_key || '%')
             AND NOT EXISTS (SELECT 1 FROM nodes n WHERE n.settings::text LIKE '%' || u.storage_key || '%')"
    };
}

/// Removes rows of uploads older than a week that no post, avatar, cover or other content uses, and returns
/// their keys. Keys are content hashes, so the caller checks `upload_key_used` before deleting.
pub async fn remove_orphan_uploads(pool: &PgPool) -> Result<Vec<String>, Error> {
    let mut tx = pool.begin().await?;
    // These are full text scans, far beyond the pool's request timeout on a large forum.
    sqlx::query("SET LOCAL statement_timeout = '10min'").execute(&mut *tx).await?;
    let keys = sqlx::query_scalar(concat!(
        "WITH gone AS (
           DELETE FROM uploads u WHERE u.created_at < now() - interval '7 days' ",
        unused!(),
        "
           RETURNING storage_key)
         SELECT DISTINCT storage_key FROM gone"
    ))
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(keys)
}

/// Removes one upload row when this member uploaded it and nothing uses it, for a draft that was
/// thrown away. True when the row went, so the caller deletes the stored files.
pub async fn remove_unused_upload(pool: &PgPool, user: i64, key: &str) -> Result<bool, Error> {
    let gone: Option<String> = sqlx::query_scalar(concat!(
        "DELETE FROM uploads u WHERE u.storage_key = $1 AND u.user_id = $2 ",
        unused!(),
        " RETURNING storage_key"
    ))
    .bind(key)
    .bind(user)
    .fetch_optional(pool)
    .await?;
    Ok(gone.is_some())
}

/// Trims tables that only ever grow and are of no use once old. The first group has fixed
/// windows (90, 30, 14, 7 days and 24 hours, listed in docs/configuration.md); `operational_days`
/// and `notification_days` (0 keeps) set the windows of the rest. Kept on purpose:
/// `ap_objects` and `ap_dm_objects`, which tie remote content to the posts that still exist.
pub async fn purge_operational(pool: &PgPool, operational_days: i64, notification_days: i64) -> Result<(), Error> {
    for sql in [
        "DELETE FROM failed_logins WHERE created_at < now() - interval '90 days'",
        "DELETE FROM search_logs WHERE created_at < now() - interval '90 days'",
        // 90 days of charts, and the 90 before them to compare with.
        "DELETE FROM page_views WHERE at < now() - interval '180 days'",
        "DELETE FROM error_log WHERE created_at < now() - interval '90 days'",
        "DELETE FROM email_tokens WHERE expires_at < now() - interval '7 days'",
        "DELETE FROM pending_logins WHERE expires_at < now()",
        "DELETE FROM mail_outbox WHERE sent_at < now() - interval '30 days'",
        // Mail that kept failing still holds an address and a message.
        "DELETE FROM mail_outbox WHERE sent_at IS NULL AND attempts >= 9 AND created_at < now() - interval '30 days'",
        "DELETE FROM webhook_deliveries WHERE status <> 'pending' AND next_at < now() - interval '30 days'",
        "DELETE FROM webhook_deliveries WHERE status = 'pending' AND created_at < now() - interval '30 days'",
        // An Idempotency-Key is honoured for 24 hours.
        "DELETE FROM api_idempotency WHERE created_at < now() - interval '24 hours'",
        "DELETE FROM ap_seen WHERE seen_at < now() - interval '14 days'",
        // Expired by date: `session_user` refuses them, but the row keeps an IP and user agent.
        "DELETE FROM sessions WHERE expires_at < now()",
        "DELETE FROM oauth_codes WHERE expires_at < now() - interval '1 day'",
        "DELETE FROM api_keys WHERE expires_at < now() - interval '30 days'",
        "DELETE FROM unsubscribe_tokens WHERE created_at < now() - interval '1 year'",
    ] {
        sqlx::query(sql).execute(pool).await?;
    }
    if operational_days > 0 {
        for sql in [
            "DELETE FROM task_log WHERE created_at < now() - make_interval(days => $1::int)",
            "DELETE FROM admin_notices WHERE dismissed_at < now() - make_interval(days => $1::int)",
            "DELETE FROM link_previews WHERE fetched_at < now() - make_interval(days => $1::int)",
            "DELETE FROM profile_visits WHERE visited_at < now() - make_interval(days => $1::int)",
        ] {
            sqlx::query(sql).bind(operational_days).execute(pool).await?;
        }
    }
    if notification_days > 0 {
        sqlx::query("DELETE FROM notifications WHERE created_at < now() - make_interval(days => $1::int)")
            .bind(notification_days)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// Forgets the IP address of requests older than `days` in the tables that record one next to
/// the event: posts, sessions and failed sign-ins (audit rows have their own function in
/// `audit.rs`). Done in batches so a large forum does not hold one long lock. Returns the number
/// of rows cleared.
pub async fn clear_old_request_ips(pool: &PgPool, days: i64) -> Result<u64, Error> {
    let mut total = 0;
    for sql in [
        "UPDATE posts SET ip = NULL WHERE id IN
           (SELECT id FROM posts WHERE ip IS NOT NULL AND created_at < now() - make_interval(days => $1::int) LIMIT 5000)",
        "UPDATE sessions SET ip = NULL WHERE token_digest IN
           (SELECT token_digest FROM sessions WHERE ip IS NOT NULL AND last_used_at < now() - make_interval(days => $1::int) LIMIT 5000)",
        "UPDATE failed_logins SET ip = NULL WHERE id IN
           (SELECT id FROM failed_logins WHERE ip IS NOT NULL AND created_at < now() - make_interval(days => $1::int) LIMIT 5000)",
    ] {
        loop {
            let n = sqlx::query(sql).bind(days).execute(pool).await?.rows_affected();
            total += n;
            if n < 5000 {
                break;
            }
        }
    }
    Ok(total)
}

pub async fn log_error(
    pool: &PgPool,
    method: &str,
    path: &str,
    status: i32,
    request_id: Option<&str>,
) -> Result<(), Error> {
    sqlx::query("INSERT INTO error_log (method, path, status, request_id) VALUES ($1, $2, $3, $4)")
        .bind(method)
        .bind(path)
        .bind(status)
        .bind(request_id)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn log_task(pool: &PgPool, task: &str, detail: serde_json::Value) -> Result<(), Error> {
    sqlx::query("INSERT INTO task_log (task, detail) VALUES ($1, $2)")
        .bind(task)
        .bind(detail)
        .execute(pool)
        .await
        .map(|_| ())
}

#[derive(Debug, Serialize, FromRow)]
pub struct ErrorRow {
    pub method: String,
    pub path: String,
    pub status: i32,
    pub request_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub async fn errors(pool: &PgPool) -> Result<Vec<ErrorRow>, Error> {
    sqlx::query_as("SELECT method, path, status, request_id, created_at FROM error_log ORDER BY id DESC LIMIT 200")
        .fetch_all(pool)
        .await
}

#[derive(Debug, Serialize, FromRow)]
pub struct TaskRow {
    pub task: String,
    pub detail: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

pub async fn tasks(pool: &PgPool) -> Result<Vec<TaskRow>, Error> {
    sqlx::query_as("SELECT task, detail, created_at FROM task_log ORDER BY id DESC LIMIT 200").fetch_all(pool).await
}

/// Things an admin should look at: (failed mail, bad addresses, errors in 24 h, pending users).
pub async fn problems(pool: &PgPool) -> Result<(i64, i64, i64, i64), Error> {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM mail_outbox WHERE sent_at IS NULL AND attempts >= 9),
                (SELECT count(*) FROM users WHERE email_bad),
                (SELECT count(*) FROM error_log WHERE created_at > now() - interval '1 day'),
                (SELECT count(*) FROM users WHERE NOT approved)",
    )
    .fetch_one(pool)
    .await
}

pub async fn latest_staff_actions(pool: &PgPool) -> Result<Vec<StaffAction>, Error> {
    sqlx::query_as(
        "SELECT l.action, l.target, u.username AS actor, l.created_at FROM audit_log l
         LEFT JOIN users u ON u.id = l.actor_id ORDER BY l.id DESC LIMIT 10",
    )
    .fetch_all(pool)
    .await
}

/// Deletes revisions past the retention period, keeping each post's original and latest.
pub async fn purge_revisions(pool: &PgPool, days: i64) -> Result<u64, Error> {
    let r = sqlx::query(
        "DELETE FROM post_revisions r WHERE r.created_at < now() - make_interval(days => $1::int)
           AND r.n <> (SELECT min(n) FROM post_revisions WHERE post_id = r.post_id)
           AND r.n <> (SELECT max(n) FROM post_revisions WHERE post_id = r.post_id)",
    )
    .bind(days)
    .execute(pool)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_revision(pool: &PgPool, post: i64, n: i32, actor: i64) -> Result<bool, Error> {
    let done = sqlx::query("DELETE FROM post_revisions WHERE post_id = $1 AND n = $2")
        .bind(post)
        .bind(n)
        .execute(pool)
        .await?
        .rows_affected()
        > 0;
    if done {
        audit(pool, actor, "revision.delete", format!("post:{post}"), serde_json::json!({ "n": n })).await?;
    }
    Ok(done)
}

pub async fn upload_key_used(pool: &PgPool, key: &str) -> Result<bool, Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM uploads WHERE storage_key = $1)").bind(key).fetch_one(pool).await
}

/// Work waiting in each background queue (metrics).
pub async fn queue_depths(pool: &PgPool) -> Result<Vec<(&'static str, i64)>, Error> {
    let (mail, webhooks, jobs, federation): (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM mail_outbox WHERE sent_at IS NULL),
                (SELECT count(*) FROM webhook_deliveries WHERE status = 'pending'),
                (SELECT count(*) FROM plugin_jobs),
                (SELECT count(*) FROM ap_deliveries)",
    )
    .fetch_one(pool)
    .await?;
    Ok(vec![("mail", mail), ("webhooks", webhooks), ("plugin_jobs", jobs), ("federation", federation)])
}

#[derive(Debug, Serialize, FromRow)]
pub struct IndexStats {
    pub topics: i64,
    pub posts: i64,
    pub members: i64,
    pub newest_id: Option<i64>,
    pub newest: Option<String>,
    /// Each forum's latest listed topic as `node|item|slug|poster|epoch|title`. Part of this
    /// statement so the index stays within its query budget.
    #[serde(skip)]
    pub last_topics: Vec<String>,
}

/// Totals for the forum index, from the denormalized counters.
pub async fn index_stats(pool: &PgPool) -> Result<IndexStats, Error> {
    sqlx::query_as(
        "SELECT coalesce((SELECT sum(topic_count) FROM nodes), 0)::bigint AS topics,
                coalesce((SELECT sum(post_count) FROM nodes), 0)::bigint AS posts,
                (SELECT count(*) FROM users WHERE suspended_until IS NULL OR suspended_until < now()) AS members,
                n.id AS newest_id, n.username AS newest,
                coalesce((SELECT array_agg(concat_ws('|', f.id, x.id, x.slug, x.poster, x.at, x.title))
                          FROM nodes f CROSS JOIN LATERAL (
                            SELECT i.id, i.slug, i.title, coalesce(u.username, '') AS poster, extract(epoch FROM i.last_posted_at)::bigint AS at
                            FROM items i LEFT JOIN users u ON u.id = i.last_poster_id
                            WHERE i.node_id = f.id AND i.state = 'visible' AND i.listed
                            ORDER BY i.last_posted_at DESC LIMIT 1) x), '{}'::text[]) AS last_topics
         FROM (SELECT 1) one LEFT JOIN LATERAL (SELECT id, username FROM users WHERE approved ORDER BY joined_at DESC, id DESC LIMIT 1) n ON true",
    )
    .fetch_one(pool)
    .await
}

/// Maintenance: recomputes every topic's and forum's counters from the rows.
pub async fn recount_all(pool: &PgPool) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE items i SET reply_count = greatest(coalesce(s.n, 0) - 1, 0), last_post_id = s.last_id,
           last_posted_at = coalesce(s.last_at, i.created_at), last_poster_id = coalesce(s.last_author, i.author_id),
           highest_post_number = greatest(i.highest_post_number, coalesce(s.highest, 0)),
           like_count = coalesce(s.likes, 0)
         FROM (SELECT item_id, count(*) FILTER (WHERE state = 'visible' AND action IS NULL) AS n,
                 (array_agg(id ORDER BY post_number DESC) FILTER (WHERE state = 'visible' AND action IS NULL))[1] AS last_id,
                 max(created_at) FILTER (WHERE state = 'visible' AND action IS NULL) AS last_at,
                 (array_agg(author_id ORDER BY post_number DESC) FILTER (WHERE state = 'visible' AND action IS NULL))[1] AS last_author,
                 max(post_number) AS highest,
                 coalesce(sum(like_count) FILTER (WHERE state = 'visible' AND action IS NULL), 0)::int AS likes
               FROM posts GROUP BY item_id) s
         WHERE s.item_id = i.id",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE nodes n SET topic_count = coalesce(s.topics, 0), post_count = coalesce(s.posts, 0),
           last_item_id = s.last_item, last_activity_at = s.last_at
         FROM (SELECT nn.id, count(i.id) AS topics, sum(i.reply_count + 1) AS posts,
                 (array_agg(i.id ORDER BY i.last_posted_at DESC) FILTER (WHERE i.id IS NOT NULL))[1] AS last_item,
                 max(i.last_posted_at) AS last_at
               FROM nodes nn LEFT JOIN items i ON i.node_id = nn.id AND i.state = 'visible' GROUP BY nn.id) s
         WHERE s.id = n.id",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE users u SET post_count = (SELECT count(*) FROM posts p JOIN items i ON i.id = p.item_id JOIN nodes n ON n.id = i.node_id
                WHERE p.author_id = u.id AND p.state = 'visible' AND p.action IS NULL AND i.state = 'visible' AND n.counts_posts)",
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

/// Maintenance: marks every post for re-cooking by the background job.
pub async fn mark_all_for_recook(pool: &PgPool) -> Result<u64, Error> {
    Ok(sqlx::query("UPDATE posts SET cooked_version = 0 WHERE action IS NULL").execute(pool).await?.rows_affected())
}
