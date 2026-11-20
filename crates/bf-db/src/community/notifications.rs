//! Notifications: delivery, preferences, listing and grouping.

use crate::accounts::preferences::APP;
use crate::accounts::preferences::EMAIL;
use chrono::{DateTime, Utc};
pub use sqlx::Error;
use sqlx::{FromRow, PgPool};

#[derive(Debug, Clone, FromRow)]
pub struct Row {
    pub id: i64,
    pub kind: String,
    pub data: serde_json::Value,
    pub read: bool,
    pub created_at: DateTime<Utc>,
}

/// The notifications page, optionally filtered by kinds.
pub async fn list(pool: &PgPool, user_id: i64, kinds: Option<&[&str]>, limit: i64) -> Result<Vec<Row>, Error> {
    let kinds: Option<Vec<String>> = kinds.map(|k| k.iter().map(|s| (*s).to_owned()).collect());
    // Notices about the member's own account stay even when the topic they name is gone.
    let account: Vec<&str> = bf_core::notifications::KINDS.iter().filter(|k| k.mandatory).map(|k| k.key).collect();
    // A notification whose topic was deleted or hidden since is not shown (`/t/slug/123...`).
    sqlx::query_as(
        "SELECT id, kind, data, read, created_at FROM notifications n
         WHERE user_id = $1 AND ($2::text[] IS NULL OR kind = ANY($2))
           AND (kind = ANY($4) OR data->>'url' IS NULL OR data->>'url' !~ '^/t/[^/]+/[0-9]{1,18}'
                OR EXISTS (SELECT 1 FROM items i WHERE i.state = 'visible'
                           AND i.id = substring(n.data->>'url' from '^/t/[^/]+/([0-9]{1,18})')::bigint))
         ORDER BY created_at DESC LIMIT $3",
    )
    .bind(user_id)
    .bind(kinds)
    .bind(limit)
    .bind(account)
    .fetch_all(pool)
    .await
}

/// The newest unread notifications the list would show (same filter as `list`), for the badge.
pub async fn list_unread(pool: &PgPool, user_id: i64, limit: i64) -> Result<Vec<Row>, Error> {
    let account: Vec<&str> = bf_core::notifications::KINDS.iter().filter(|k| k.mandatory).map(|k| k.key).collect();
    sqlx::query_as(
        "SELECT id, kind, data, read, created_at FROM notifications n
         WHERE user_id = $1 AND NOT read
           AND (kind = ANY($3) OR data->>'url' IS NULL OR data->>'url' !~ '^/t/[^/]+/[0-9]{1,18}'
                OR EXISTS (SELECT 1 FROM items i WHERE i.state = 'visible'
                           AND i.id = substring(n.data->>'url' from '^/t/[^/]+/([0-9]{1,18})')::bigint))
         ORDER BY created_at DESC LIMIT $2",
    )
    .bind(user_id)
    .bind(limit)
    .bind(account)
    .fetch_all(pool)
    .await
}

/// The topic a notification links to, from its `/t/slug/123#post-4` URL.
pub fn topic_in_url(url: &str) -> Option<i64> {
    let rest = url.strip_prefix("/t/")?;
    let (_, tail) = rest.split_once('/')?;
    let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// (topic, forum) of the visible topics among `ids`: one query for a page of notifications.
pub async fn topic_nodes(pool: &PgPool, ids: &[i64]) -> Result<Vec<(i64, i64)>, Error> {
    sqlx::query_as("SELECT id, node_id FROM items WHERE id = ANY($1) AND state = 'visible'")
        .bind(ids)
        .fetch_all(pool)
        .await
}

/// Held notification mail (delayed or in quiet hours): (mail id, recipient, topic, forum) for
/// topics that still exist, so the sender side can drop what the recipient may no longer read.
/// Notices about the member's own account are not included.
pub async fn held_mail_topics(pool: &PgPool, limit: i64) -> Result<Vec<(i64, i64, i64, i64)>, Error> {
    let account: Vec<&str> = bf_core::notifications::KINDS.iter().filter(|k| k.mandatory).map(|k| k.key).collect();
    sqlx::query_as(
        "SELECT o.id, o.unless_read_user, i.id, i.node_id FROM mail_outbox o
         JOIN items i ON i.id = substring(o.unless_read_url from '^/t/[^/]+/([0-9]{1,18})')::bigint
         WHERE o.sent_at IS NULL AND o.unless_read_user IS NOT NULL AND o.not_before > now()
           AND o.unless_read_url ~ '^/t/[^/]+/[0-9]{1,18}'
           AND NOT EXISTS (SELECT 1 FROM notifications n WHERE n.user_id = o.unless_read_user
                           AND n.data->>'url' = o.unless_read_url AND n.kind = ANY($2))
         ORDER BY o.id LIMIT $1",
    )
    .bind(limit)
    .bind(account)
    .fetch_all(pool)
    .await
}

/// Closes queued mail the recipient may no longer read, so it is neither sent nor retried.
pub async fn drop_mail(pool: &PgPool, ids: &[i64], reason: &str) -> Result<(), Error> {
    sqlx::query("UPDATE mail_outbox SET sent_at = now(), last_error = $2 WHERE id = ANY($1) AND sent_at IS NULL")
        .bind(ids)
        .bind(reason)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Of `ids`, the members who would get an email of this kind: the channel is on for them
/// (their choice, else the kind's default) and the address is verified and working.
pub async fn emailed_for_kind(pool: &PgPool, kind: &str, ids: &[i64]) -> Result<Vec<i64>, Error> {
    sqlx::query_scalar(
        "SELECT u.id FROM users u LEFT JOIN notification_prefs p ON p.user_id = u.id AND p.kind = $2
         WHERE u.id = ANY($1) AND u.email_verified_at IS NOT NULL AND NOT u.email_bad
           AND coalesce(p.channels, $3) & 2 <> 0",
    )
    .bind(ids)
    .bind(kind)
    .bind(default_channels(kind))
    .fetch_all(pool)
    .await
}

/// Marks one notification read and returns its stored URL.
pub async fn open(pool: &PgPool, user_id: i64, id: i64) -> Result<Option<String>, Error> {
    sqlx::query_scalar("UPDATE notifications SET read = true WHERE id = $1 AND user_id = $2 RETURNING data->>'url'")
        .bind(id)
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map(Option::flatten)
}

pub async fn mark_all_read(pool: &PgPool, user_id: i64) -> Result<(), Error> {
    sqlx::query("UPDATE notifications SET read = true WHERE user_id = $1 AND NOT read")
        .bind(user_id)
        .execute(pool)
        .await
        .map(|_| ())
}

/// A like on `post_id`: joins the author's unread like notification for that post
/// ("ann and 2 others liked …") or starts one. Returns false when the author ignores
/// the liker or likes are switched off for them.
pub async fn like(
    pool: &PgPool,
    author: i64,
    liker: &str,
    liker_id: i64,
    post_id: i64,
    title: &str,
    url: &str,
) -> Result<bool, Error> {
    let mut tx = pool.begin().await?;
    let wanted: bool = sqlx::query_scalar(
        "SELECT NOT EXISTS (SELECT 1 FROM ignored_users WHERE user_id = $1 AND ignored_user_id = $2)
            AND coalesce((SELECT channels & 1 FROM notification_prefs WHERE user_id = $1 AND kind = 'like'), 1) = 1",
    )
    .bind(author)
    .bind(liker_id)
    .fetch_one(&mut *tx)
    .await?;
    if !wanted {
        return Ok(false);
    }
    let updated = sqlx::query(
        "UPDATE notifications SET created_at = now(),
           data = jsonb_set(jsonb_set(data, '{users}', to_jsonb($3::text) || (data->'users')), '{count}', to_jsonb((data->>'count')::int + 1))
         WHERE user_id = $1 AND kind = 'like' AND NOT read AND data->>'post_id' = $2::text
           AND NOT (data->'users') ? $3",
    )
    .bind(author)
    .bind(post_id.to_string())
    .bind(liker)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    let already: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM notifications WHERE user_id = $1 AND kind = 'like' AND NOT read AND data->>'post_id' = $2::text)",
    )
    .bind(author)
    .bind(post_id.to_string())
    .fetch_one(&mut *tx)
    .await?;
    if updated == 0 && !already {
        sqlx::query("INSERT INTO notifications (user_id, kind, data) VALUES ($1, 'like', $2)")
            .bind(author)
            .bind(serde_json::json!({ "post_id": post_id.to_string(), "users": [liker], "count": 1, "title": title, "url": url }))
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(true)
}

pub async fn by_id(pool: &PgPool, id: i64) -> Result<Option<Row>, sqlx::Error> {
    sqlx::query_as("SELECT id, kind, data, read, created_at FROM notifications WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// Default channels per kind when the user has not chosen.
pub fn default_channels(kind: &str) -> i16 {
    bf_core::notifications::default_channels(kind)
}

pub async fn notification_prefs(pool: &PgPool, user_id: i64) -> Result<Vec<(String, i16)>, Error> {
    sqlx::query_as("SELECT kind, channels FROM notification_prefs WHERE user_id = $1")
        .bind(user_id)
        .fetch_all(pool)
        .await
}

pub async fn set_notification_pref(pool: &PgPool, user_id: i64, kind: &str, channels: i16) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO notification_prefs (user_id, kind, channels) VALUES ($1, $2, $3)
         ON CONFLICT (user_id, kind) DO UPDATE SET channels = EXCLUDED.channels",
    )
    .bind(user_id)
    .bind(kind)
    .bind(channels)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Delivers one notification kind to many users, honouring each user's channels:
/// in-app rows and/or queued emails, in one transaction. Returns the in-app recipients who got a new row (not those whose unread one grew).
/// Mail for a member in quiet hours or "do not disturb" is held until that ends.
#[allow(clippy::too_many_arguments)]
pub async fn deliver(
    pool: &PgPool,
    base_url: &str,
    from_user: i64,
    user_ids: &[i64],
    kind: &str,
    data: &serde_json::Value,
    email_subject: &str,
    email_body: &str,
    email_delay_minutes: i32,
) -> Result<Vec<i64>, Error> {
    if user_ids.is_empty() {
        return Ok(vec![]);
    }
    let mut tx = pool.begin().await?;
    let rows: Vec<(i64, String, i16, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT u.id, u.email,
                CASE WHEN u.email_verified_at IS NULL OR u.email_bad OR u.email_lower LIKE '%.invalid'
                     THEN (CASE WHEN $5 THEN $3 ELSE coalesce(p.channels, $3) END) & ~2::smallint
                     ELSE CASE WHEN $5 THEN $3 ELSE coalesce(p.channels, $3) END END,
                CASE WHEN $5 THEN NULL
                     ELSE greatest(CASE WHEN u.dnd_until > now() THEN u.dnd_until END,
                                   quiet_hours_end(u.quiet_start, u.quiet_end, u.timezone)) END
         FROM users u
         LEFT JOIN notification_prefs p ON p.user_id = u.id AND p.kind = $2
         WHERE u.id = ANY($1) AND ($5 OR u.suspended_until IS NULL OR u.suspended_until < now())
           AND u.approved AND u.delete_after IS NULL
           AND ($5 OR NOT EXISTS (SELECT 1 FROM ignored_users i WHERE i.user_id = u.id AND i.ignored_user_id = $4))",
    )
    .bind(user_ids)
    .bind(kind)
    .bind(default_channels(kind))
    .bind(from_user)
    .bind(bf_core::notifications::is_mandatory(kind))
    .fetch_all(&mut *tx)
    .await?;
    let app: Vec<i64> = rows.iter().filter(|r| r.2 & APP != 0).map(|r| r.0).collect();
    // A notification with a "group" joins that member's unread one instead of adding a row.
    let folded: Vec<i64> = match data.get("group").and_then(serde_json::Value::as_str) {
        Some(group) => {
            sqlx::query_scalar(
                "UPDATE notifications SET created_at = now(),
                   data = jsonb_set(data, '{count}', to_jsonb(coalesce((data->>'count')::int, 1) + 1))
                 WHERE user_id = ANY($1) AND kind = $2 AND NOT read AND data->>'group' = $3 RETURNING user_id",
            )
            .bind(&app)
            .bind(kind)
            .bind(group)
            .fetch_all(&mut *tx)
            .await?
        }
        None => vec![],
    };
    let fresh: Vec<i64> = app.iter().copied().filter(|id| !folded.contains(id)).collect();
    sqlx::query("INSERT INTO notifications (user_id, kind, data) SELECT unnest($1::bigint[]), $2, $3")
        .bind(&fresh)
        .bind(kind)
        .bind(data)
        .execute(&mut *tx)
        .await?;
    // Each email gets its own one-click unsubscribe link for this notification type.
    let (emails, held_until): (Vec<i64>, Vec<Option<DateTime<Utc>>>) =
        rows.iter().filter(|r| r.2 & EMAIL != 0).map(|r| (r.0, r.3)).unzip();
    sqlx::query(
        "WITH t AS (INSERT INTO unsubscribe_tokens (token, user_id, kind, item_id, conversation_id)
                    SELECT gen_random_uuid()::text, unnest($1::bigint[]), $2, $8::bigint, $10::bigint RETURNING token, user_id, reply_key)
         INSERT INTO mail_outbox (to_addr, subject, body, unsubscribe_url, not_before, unless_read_user, unless_read_url, reply_key)
         SELECT u.email, $3, $4 || E'\\nStop these emails: ' || $5 || '/unsubscribe/' || t.token
                || CASE WHEN $8::bigint IS NOT NULL THEN E'\\nMute this topic: ' || $5 || '/unsubscribe/' || t.token || '/topic' ELSE '' END,
                $5 || '/unsubscribe/' || t.token,
                CASE WHEN $6 > 0 OR d.held IS NOT NULL
                     THEN greatest(now() + make_interval(mins => $6), coalesce(d.held, now())) END,
                CASE WHEN $6 > 0 OR d.held IS NOT NULL THEN u.id END,
                CASE WHEN $6 > 0 OR d.held IS NOT NULL THEN $7 END,
                CASE WHEN $8::bigint IS NOT NULL OR $10::bigint IS NOT NULL THEN t.reply_key END
         FROM t JOIN users u ON u.id = t.user_id
         JOIN unnest($1::bigint[], $9::timestamptz[]) AS d(id, held) ON d.id = u.id",
    )
    .bind(&emails)
    .bind(kind)
    .bind(email_subject)
    .bind(email_body)
    .bind(base_url)
    .bind(email_delay_minutes)
    .bind(data.get("url").and_then(serde_json::Value::as_str))
    // Mail about a topic can be answered by email.
    .bind(data.get("topic_id").and_then(serde_json::Value::as_i64))
    .bind(&held_until)
    // A private message can be answered by email too.
    .bind(data.get("conversation_id").and_then(serde_json::Value::as_i64))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    // A notification folded into an unread one adds nothing to a live badge.
    Ok(fresh)
}

/// Members grouped by language, so each reads an email in their own.
pub async fn recipients_by_locale(pool: &PgPool, ids: &[i64]) -> Result<Vec<(Option<String>, Vec<i64>)>, Error> {
    sqlx::query_as("SELECT locale, array_agg(id) FROM users WHERE id = ANY($1) GROUP BY locale")
        .bind(ids)
        .fetch_all(pool)
        .await
}

/// Global staff and the moderators of one forum.
pub async fn reviewers(pool: &PgPool, node: i64) -> Result<Vec<i64>, Error> {
    sqlx::query_scalar(
        "SELECT id FROM users WHERE primary_group_id IN (3, 4)
         UNION SELECT user_id FROM node_moderators WHERE node_id = $1",
    )
    .bind(node)
    .fetch_all(pool)
    .await
}

/// Global staff: moderators (3) and administrators (4), by primary or secondary group.
pub async fn staff(pool: &PgPool) -> Result<Vec<i64>, Error> {
    sqlx::query_scalar(
        "SELECT id FROM users WHERE primary_group_id IN (3, 4)
         UNION SELECT user_id FROM user_secondary_groups WHERE group_id IN (3, 4)",
    )
    .fetch_all(pool)
    .await
}

pub async fn notify(pool: &PgPool, user_ids: &[i64], kind: &str, data: &serde_json::Value) -> Result<(), Error> {
    sqlx::query("INSERT INTO notifications (user_id, kind, data) SELECT unnest($1::bigint[]), $2, $3")
        .bind(user_ids)
        .bind(kind)
        .bind(data)
        .execute(pool)
        .await
        .map(|_| ())
}

#[derive(Debug, Clone, FromRow)]
pub struct NotificationRow {
    pub id: i64,
    pub kind: String,
    pub data: serde_json::Value,
    pub read: bool,
    pub created_at: DateTime<Utc>,
}

pub async fn notifications(pool: &PgPool, user_id: i64) -> Result<Vec<NotificationRow>, Error> {
    sqlx::query_as(
        "SELECT id, kind, data, read, created_at FROM notifications WHERE user_id = $1 ORDER BY created_at DESC LIMIT 50",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
}

/// Is this member in mailing-list mode? Stored as the email channel of the `mailing_list`
/// preference, so the unsubscribe link in each of those emails turns it off.
pub async fn mailing_list_on(pool: &PgPool, user: i64) -> Result<bool, Error> {
    Ok(sqlx::query_scalar::<_, i16>(
        "SELECT channels FROM notification_prefs WHERE user_id = $1 AND kind = 'mailing_list'",
    )
    .bind(user)
    .fetch_optional(pool)
    .await?
    .is_some_and(|c| c & EMAIL != 0))
}

pub async fn set_mailing_list(pool: &PgPool, user: i64, on: bool) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO notification_prefs (user_id, kind, channels) VALUES ($1, 'mailing_list', $2)
         ON CONFLICT (user_id, kind) DO UPDATE SET channels = EXCLUDED.channels",
    )
    .bind(user)
    .bind(if on { EMAIL } else { 0 })
    .execute(pool)
    .await
    .map(|_| ())
}

/// Members in mailing-list mode who may hear about a post in `item`: not its author, not
/// suspended, with a verified working address, not ignoring the author and not muting the topic
/// or its forum. Read permission is checked by the caller.
pub async fn mailing_list_members(pool: &PgPool, author: i64, item: i64, node: i64) -> Result<Vec<i64>, Error> {
    sqlx::query_scalar(
        "WITH RECURSIVE chain(id, parent_id) AS (
           SELECT id, parent_id FROM nodes WHERE id = $3
           UNION SELECT n.id, n.parent_id FROM nodes n JOIN chain c ON n.id = c.parent_id)
         SELECT u.id FROM notification_prefs p JOIN users u ON u.id = p.user_id
         WHERE p.kind = 'mailing_list' AND p.channels & 2 <> 0 AND u.id <> $1
           AND u.email_verified_at IS NOT NULL AND NOT u.email_bad AND u.approved
           AND (u.suspended_until IS NULL OR u.suspended_until < now())
           AND NOT EXISTS (SELECT 1 FROM ignored_users i WHERE i.user_id = u.id AND i.ignored_user_id = $1)
           AND u.email_lower NOT LIKE '%.invalid' AND u.delete_after IS NULL
           AND NOT EXISTS (SELECT 1 FROM follows f WHERE f.user_id = u.id AND f.level = 'mute'
                           AND ((f.target_type = 'item' AND f.target_id = $2)
                             OR (f.target_type = 'node' AND f.target_id IN (SELECT id FROM chain))))",
    )
    .bind(author)
    .bind(item)
    .bind(node)
    .fetch_all(pool)
    .await
}
