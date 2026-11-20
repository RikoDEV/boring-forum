//! Topic timers, slow mode, wiki posts, saved replies and inbound mail keys.

use serde::Serialize;
use sqlx::{FromRow, PgPool};

type Error = sqlx::Error;

pub async fn set_timer(pool: &PgPool, item: i64, action: &str, minutes: i32, by: i64) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO item_timers (item_id, action, run_at, created_by) VALUES ($1, $2, now() + make_interval(mins => $3), $4)
         ON CONFLICT (item_id) DO UPDATE SET action = EXCLUDED.action, run_at = EXCLUDED.run_at, created_by = EXCLUDED.created_by",
    )
    .bind(item)
    .bind(action)
    .bind(minutes)
    .bind(by)
    .execute(pool)
    .await
    .map(|_| ())
}

pub async fn clear_timer(pool: &PgPool, item: i64) -> Result<(), Error> {
    sqlx::query("DELETE FROM item_timers WHERE item_id = $1").bind(item).execute(pool).await.map(|_| ())
}

/// Applies due timers once each, returning how many ran and which topics were published.
/// Closes topics in forums with `auto_close_days` that have had no reply for that long, and,
/// when `max_replies` is above 0, topics that have reached that many replies. Pinned topics stay
/// open. A topic a moderator reopened (`auto_close_exempt`, set by a trigger on unlocking) is not
/// closed again. Returns the topics closed.
pub async fn close_inactive(pool: &PgPool, max_replies: i64) -> Result<Vec<i64>, Error> {
    sqlx::query_scalar(
        "UPDATE items i SET locked = true FROM nodes n
         WHERE n.id = i.node_id AND NOT i.locked AND NOT i.pinned AND i.state = 'visible'
           AND NOT i.auto_close_exempt
           AND ((n.auto_close_days IS NOT NULL AND i.last_posted_at < now() - make_interval(days => n.auto_close_days))
             OR ($1 > 0 AND i.reply_count >= $1))
         RETURNING i.id",
    )
    .bind(max_replies)
    .fetch_all(pool)
    .await
}

pub async fn run_due_timers(pool: &PgPool) -> Result<(u64, Vec<i64>), Error> {
    // The due rows are locked for this transaction (SKIP LOCKED), so a second node that ticks at the
    // same time takes none of them and no timer fires twice.
    let mut tx = pool.begin().await?;
    let due: Vec<(i64, String, Option<i64>)> = sqlx::query_as(
        "SELECT item_id, action, created_by FROM item_timers WHERE run_at <= now() FOR UPDATE SKIP LOCKED",
    )
    .fetch_all(&mut *tx)
    .await?;
    let mut ran = 0;
    let mut published = Vec::new();
    for (item, action, by) in &due {
        // A timer stays until it has been applied, so a failure retries on the next tick.
        let did = match apply_timer(pool, *item, action, *by).await {
            Ok(did) => did,
            Err(e) => {
                tracing::warn!(item, action, error = %e, "topic timer failed; will retry");
                continue;
            }
        };
        sqlx::query("DELETE FROM item_timers WHERE item_id = $1 AND action = $2 AND run_at <= now()")
            .bind(item)
            .bind(action)
            .execute(&mut *tx)
            .await?;
        ran += 1;
        if action == "publish" && did {
            published.push(*item);
        }
    }
    tx.commit().await?;
    Ok((ran, published))
}

/// Applies one timer; true when it changed something that callers announce (a topic went public).
async fn apply_timer(pool: &PgPool, item: i64, action: &str, by: Option<i64>) -> Result<bool, Error> {
    let item = &item;
    match action {
        "close" => {
            sqlx::query("UPDATE items SET locked = true WHERE id = $1").bind(item).execute(pool).await?;
        }
        "open" => {
            // Reopened on purpose: the auto-close job leaves it alone (also set by the unlock trigger).
            sqlx::query("UPDATE items SET locked = false, auto_close_exempt = true WHERE id = $1")
                .bind(item)
                .execute(pool)
                .await?;
        }
        "publish" => {
            let held: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM posts p JOIN items i ON i.id = p.item_id
                 WHERE p.item_id = $1 AND p.post_number = 1 AND p.state = 'pending' AND i.state <> 'deleted')",
            )
            .bind(item)
            .fetch_one(pool)
            .await?;
            crate::forum::topics::publish_item(pool, *item).await?;
            return Ok(held);
        }
        "delete" => {
            // An already deleted first post is left as it is: no new deletion time or moderator.
            let first: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM posts WHERE item_id = $1 AND post_number = 1 AND state <> 'deleted'",
            )
            .bind(item)
            .fetch_optional(pool)
            .await?;
            if let (Some(post), Some(by)) = (first, by) {
                crate::forum::posts::soft_delete_post(pool, post, by).await?;
            }
        }
        _ => {
            sqlx::query("UPDATE items SET last_posted_at = now() WHERE id = $1").bind(item).execute(pool).await?;
        }
    }
    Ok(true)
}

/// A topic that waits for its publish time has no review item: nobody is asked to approve it
/// early, and reviewers are not told about it.
pub async fn drop_scheduled_review(pool: &PgPool, item: i64) -> Result<(), Error> {
    sqlx::query(
        "DELETE FROM reviewables WHERE kind = 'pending_post' AND status = 'pending' AND target_type = 'post'
         AND target_id IN (SELECT id FROM posts WHERE item_id = $1 AND post_number = 1 AND state = 'pending')",
    )
    .bind(item)
    .execute(pool)
    .await
    .map(|_| ())
}

crate::sql::set_col!(set_slow_mode, "items", "slow_mode_seconds", i64, i32);

/// Seconds this member must still wait in this topic, if it is in slow mode. Their anonymous
/// replies count as theirs.
pub async fn slow_mode_wait(pool: &PgPool, item: i64, user: i64) -> Result<Option<i64>, Error> {
    sqlx::query_scalar(
        "SELECT ceil(i.slow_mode_seconds - extract(epoch FROM now() - max(p.created_at)))::bigint
         FROM items i JOIN posts p ON p.item_id = i.id AND (p.author_id = $2 OR p.real_author_id = $2)
         WHERE i.id = $1 AND i.slow_mode_seconds > 0 GROUP BY i.slow_mode_seconds",
    )
    .bind(item)
    .bind(user)
    .fetch_optional(pool)
    .await
}

pub async fn toggle_wiki(pool: &PgPool, post: i64) -> Result<(), Error> {
    sqlx::query("UPDATE posts SET wiki = NOT wiki WHERE id = $1").bind(post).execute(pool).await.map(|_| ())
}

pub async fn is_wiki(pool: &PgPool, post: i64) -> Result<bool, Error> {
    sqlx::query_scalar("SELECT wiki FROM posts WHERE id = $1").bind(post).fetch_one(pool).await
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct SavedReply {
    pub id: i64,
    pub title: String,
    pub body: String,
}

pub async fn saved_replies(pool: &PgPool) -> Result<Vec<SavedReply>, Error> {
    sqlx::query_as("SELECT id, title, body FROM saved_replies ORDER BY title").fetch_all(pool).await
}

pub async fn create_saved_reply(pool: &PgPool, title: &str, body: &str) -> Result<i64, Error> {
    sqlx::query_scalar("INSERT INTO saved_replies (title, body) VALUES ($1, $2) RETURNING id")
        .bind(title)
        .bind(body)
        .fetch_one(pool)
        .await
}

pub async fn update_saved_reply(pool: &PgPool, id: i64, title: &str, body: &str) -> Result<bool, Error> {
    Ok(sqlx::query("UPDATE saved_replies SET title = $2, body = $3 WHERE id = $1")
        .bind(id)
        .bind(title)
        .bind(body)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

crate::sql::delete_by_id!(delete_saved_reply, "saved_replies");

/// Who and which topic a reply address stands for.
pub async fn reply_target(pool: &PgPool, key: &str) -> Result<Option<(i64, i64)>, Error> {
    sqlx::query_as("SELECT user_id, item_id FROM unsubscribe_tokens WHERE reply_key = $1 AND item_id IS NOT NULL AND reply_key_expires > now()")
        .bind(key)
        .fetch_optional(pool)
        .await
}

/// Who and which private conversation a reply address stands for.
pub async fn conversation_target(pool: &PgPool, key: &str) -> Result<Option<(i64, i64)>, Error> {
    sqlx::query_as("SELECT user_id, conversation_id FROM unsubscribe_tokens WHERE reply_key = $1 AND conversation_id IS NOT NULL AND reply_key_expires > now()")
        .bind(key)
        .fetch_optional(pool)
        .await
}

/// A member's key for posting by email; a new one replaces the old.
pub async fn new_mail_key(pool: &PgPool, user: i64) -> Result<String, Error> {
    sqlx::query_scalar("UPDATE users SET mail_post_key = gen_random_uuid()::text WHERE id = $1 RETURNING mail_post_key")
        .bind(user)
        .fetch_one(pool)
        .await
}

pub async fn mail_key_owner(pool: &PgPool, key: &str) -> Result<Option<i64>, Error> {
    sqlx::query_scalar("SELECT id FROM users WHERE mail_post_key = $1").bind(key).fetch_optional(pool).await
}
