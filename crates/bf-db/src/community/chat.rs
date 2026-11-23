//! Built-in chat and the rules that send forum activity to outside chat services.

use crate::admin::audit::{Entry, record};
use chrono::{DateTime, Utc};
use serde::Serialize;
pub use sqlx::Error;
use sqlx::{FromRow, PgPool};

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Channel {
    pub id: i64,
    pub kind: String,
    pub name: String,
    pub description: String,
    /// open, groups or invite.
    pub access: String,
    pub created_by: Option<i64>,
    pub archived: bool,
}

macro_rules! channel_cols {
    () => {
        "c.id, c.kind, c.name, c.description, c.access, c.created_by, c.archived"
    };
}

/// Whether one of these groups is allowed into the channel.
pub async fn group_allowed(pool: &PgPool, channel: i64, groups: &[i64]) -> Result<bool, Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM chat_channel_groups WHERE channel_id = $1 AND group_id = ANY($2))")
        .bind(channel)
        .bind(groups)
        .fetch_one(pool)
        .await
}

/// Those among `ids` who can see this channel, for mentions.
pub async fn can_see(pool: &PgPool, channel: i64, ids: &[i64]) -> Result<Vec<i64>, Error> {
    sqlx::query_scalar(
        "SELECT u.id FROM users u JOIN chat_channels c ON c.id = $1 WHERE u.id = ANY($2) AND (
           c.access = 'open'
           OR EXISTS (SELECT 1 FROM chat_members m WHERE m.channel_id = c.id AND m.user_id = u.id)
           OR (c.access = 'groups' AND EXISTS (SELECT 1 FROM chat_channel_groups g WHERE g.channel_id = c.id
                 AND (g.group_id = u.primary_group_id OR g.group_id IN (SELECT group_id FROM user_secondary_groups s WHERE s.user_id = u.id)))))",
    )
    .bind(channel)
    .bind(ids)
    .fetch_all(pool)
    .await
}

/// A channel's people, for its creator: (id, username).
pub async fn members(pool: &PgPool, channel: i64) -> Result<Vec<(i64, String)>, Error> {
    sqlx::query_as("SELECT u.id, u.username FROM chat_members m JOIN users u ON u.id = m.user_id WHERE m.channel_id = $1 ORDER BY u.username")
        .bind(channel)
        .fetch_all(pool)
        .await
}

pub async fn channel(pool: &PgPool, id: i64) -> Result<Option<Channel>, Error> {
    // Only a fixed column list or select prefix (a constant) is interpolated; every value is bound.
    sqlx::query_as(concat!("SELECT ", channel_cols!(), " FROM chat_channels c WHERE c.id = $1"))
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn is_member(pool: &PgPool, channel: i64, user: i64) -> Result<bool, Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM chat_members WHERE channel_id = $1 AND user_id = $2)")
        .bind(channel)
        .bind(user)
        .fetch_one(pool)
        .await
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct ChannelRow {
    pub id: i64,
    pub kind: String,
    pub name: String,
    pub description: String,
    pub archived: bool,
    pub access: String,
    pub created_by: Option<i64>,
    pub joined: bool,
    pub muted: bool,
    pub unread: i64,
    /// A direct message's other people.
    pub people: Vec<String>,
}

/// Every channel this member can open: public ones they may read, and their direct messages.
pub async fn channels_for(pool: &PgPool, user: i64, groups: &[i64], admin: bool) -> Result<Vec<ChannelRow>, Error> {
    sqlx::query_as(
        "SELECT c.id, c.kind, c.name, c.description, c.archived, c.access, c.created_by, m.user_id IS NOT NULL AS joined,
                coalesce(m.muted, false) AS muted,
                CASE WHEN m.user_id IS NULL THEN 0 ELSE
                  (SELECT count(*) FROM chat_messages x WHERE x.channel_id = c.id AND x.id > m.last_read_id
                     AND x.deleted_at IS NULL AND x.user_id IS DISTINCT FROM $1) END AS unread,
                CASE WHEN c.kind = 'dm' THEN ARRAY(SELECT u.username FROM chat_members o JOIN users u ON u.id = o.user_id
                     WHERE o.channel_id = c.id AND o.user_id <> $1 ORDER BY u.username) ELSE '{}'::text[] END AS people
         FROM chat_channels c LEFT JOIN chat_members m ON m.channel_id = c.id AND m.user_id = $1
         WHERE (m.user_id IS NOT NULL AND (c.kind <> 'public' OR c.access <> 'groups'))
            OR (c.kind = 'public' AND (c.access = 'open' OR (c.access = 'groups' AND ($3 OR EXISTS (
                  SELECT 1 FROM chat_channel_groups g WHERE g.channel_id = c.id AND g.group_id = ANY($2))))))
         ORDER BY c.kind DESC, c.name, c.id",
    )
    .bind(user)
    .bind(groups)
    .bind(admin)
    .fetch_all(pool)
    .await
}

pub async fn join(pool: &PgPool, channel: i64, user: i64) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO chat_members (channel_id, user_id, last_read_id)
         VALUES ($1, $2, coalesce((SELECT max(id) FROM chat_messages WHERE channel_id = $1), 0)) ON CONFLICT DO NOTHING",
    )
    .bind(channel)
    .bind(user)
    .execute(pool)
    .await
    .map(|_| ())
}

pub async fn leave(pool: &PgPool, channel: i64, user: i64) -> Result<(), Error> {
    sqlx::query("DELETE FROM chat_members WHERE channel_id = $1 AND user_id = $2")
        .bind(channel)
        .bind(user)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn set_muted(pool: &PgPool, channel: i64, user: i64, muted: bool) -> Result<(), Error> {
    sqlx::query("UPDATE chat_members SET muted = $3 WHERE channel_id = $1 AND user_id = $2")
        .bind(channel)
        .bind(user)
        .bind(muted)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Only ever moves forward, so a stale tab cannot bring old messages back as unread.
pub async fn mark_read(pool: &PgPool, channel: i64, user: i64, id: i64) -> Result<(), Error> {
    sqlx::query(
        "UPDATE chat_members SET last_read_id = greatest(last_read_id, $3) WHERE channel_id = $1 AND user_id = $2",
    )
    .bind(channel)
    .bind(user)
    .bind(id)
    .execute(pool)
    .await
    .map(|_| ())
}

/// The one conversation between exactly these people, started if need be.
pub async fn open_dm(pool: &PgPool, people: &[i64], by: i64) -> Result<i64, Error> {
    let mut sorted = people.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let key = sorted.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
    let mut tx = pool.begin().await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO chat_channels (kind, dm_key, created_by) VALUES ('dm', $1, $2)
         ON CONFLICT (dm_key) DO UPDATE SET dm_key = EXCLUDED.dm_key RETURNING id",
    )
    .bind(&key)
    .bind(by)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO chat_members (channel_id, user_id) SELECT $1, unnest($2::bigint[]) ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(&sorted)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(id)
}

/// Those among `people` who ignore `user`, and so cannot be messaged by them.
pub async fn ignoring(pool: &PgPool, people: &[i64], user: i64) -> Result<bool, Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM ignored_users WHERE user_id = ANY($1) AND ignored_user_id = $2)")
        .bind(people)
        .bind(user)
        .fetch_one(pool)
        .await
}

/// A new channel. An invite-only one starts with its creator as the first member.
pub async fn create_channel(
    pool: &PgPool,
    name: &str,
    description: &str,
    access: &str,
    groups: &[i64],
    by: i64,
) -> Result<i64, Error> {
    let mut tx = pool.begin().await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO chat_channels (kind, name, description, access, created_by) VALUES ('public', $1, $2, $3, $4) RETURNING id",
    )
    .bind(name)
    .bind(description)
    .bind(access)
    .bind(by)
    .fetch_one(&mut *tx)
    .await?;
    set_groups(&mut tx, id, groups).await?;
    if access == "invite" {
        sqlx::query("INSERT INTO chat_members (channel_id, user_id) VALUES ($1, $2)")
            .bind(id)
            .bind(by)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(id)
}

async fn set_groups(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, channel: i64, groups: &[i64]) -> Result<(), Error> {
    sqlx::query("DELETE FROM chat_channel_groups WHERE channel_id = $1").bind(channel).execute(&mut **tx).await?;
    sqlx::query(
        "INSERT INTO chat_channel_groups (channel_id, group_id) SELECT $1, unnest($2::bigint[]) ON CONFLICT DO NOTHING",
    )
    .bind(channel)
    .bind(groups)
    .execute(&mut **tx)
    .await
    .map(|_| ())
}

pub async fn update_channel(
    pool: &PgPool,
    id: i64,
    name: &str,
    description: &str,
    access: &str,
    groups: &[i64],
    archived: bool,
) -> Result<bool, Error> {
    let mut tx = pool.begin().await?;
    let found = sqlx::query(
        "UPDATE chat_channels SET name = $2, description = $3, access = $4, archived = $5 WHERE id = $1 AND kind = 'public'",
    )
    .bind(id)
    .bind(name)
    .bind(description)
    .bind(access)
    .bind(archived)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        > 0;
    if found {
        set_groups(&mut tx, id, groups).await?;
    }
    tx.commit().await?;
    Ok(found)
}

/// Switches "everyone joins" on or off. Turning it on puts every current member in the channel,
/// reading from its newest message, so nobody starts with a pile of unread. Only an open channel
/// can have it; for any other the flag stays off. Returns whether the channel exists.
pub async fn set_auto_join(pool: &PgPool, id: i64, on: bool) -> Result<bool, Error> {
    let mut tx = pool.begin().await?;
    let Some((was, open)): Option<(bool, bool)> = sqlx::query_as(
        "SELECT auto_join, access = 'open' FROM chat_channels WHERE id = $1 AND kind = 'public' FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Ok(false);
    };
    let on = on && open;
    if on != was {
        sqlx::query("UPDATE chat_channels SET auto_join = $2 WHERE id = $1")
            .bind(id)
            .bind(on)
            .execute(&mut *tx)
            .await?;
    }
    if on && !was {
        sqlx::query(
            "INSERT INTO chat_members (channel_id, user_id, last_read_id)
             SELECT $1, u.id, coalesce((SELECT max(id) FROM chat_messages WHERE channel_id = $1), 0)
               FROM users u WHERE NOT u.remote AND u.email_lower NOT LIKE '%@plugins.invalid'
             ON CONFLICT DO NOTHING",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(true)
}

/// Puts a new account into every channel that takes everyone.
pub async fn join_auto_channels(conn: &mut sqlx::PgConnection, user: i64) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO chat_members (channel_id, user_id, last_read_id)
         SELECT c.id, $1, coalesce((SELECT max(id) FROM chat_messages WHERE channel_id = c.id), 0)
           FROM chat_channels c WHERE c.auto_join AND c.access = 'open' AND NOT c.archived
         ON CONFLICT DO NOTHING",
    )
    .bind(user)
    .execute(conn)
    .await
    .map(|_| ())
}

pub async fn delete_channel(pool: &PgPool, id: i64) -> Result<bool, Error> {
    Ok(sqlx::query("DELETE FROM chat_channels WHERE id = $1 AND kind = 'public'")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct AdminChannel {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub access: String,
    pub group_ids: Vec<i64>,
    pub archived: bool,
    pub auto_join: bool,
    pub messages: i64,
}

pub async fn admin_channels(pool: &PgPool) -> Result<Vec<AdminChannel>, Error> {
    sqlx::query_as(
        "SELECT c.id, c.name, c.description, c.access, c.archived, c.auto_join,
                ARRAY(SELECT group_id FROM chat_channel_groups g WHERE g.channel_id = c.id) AS group_ids,
                (SELECT count(*) FROM chat_messages m WHERE m.channel_id = c.id) AS messages
         FROM chat_channels c WHERE c.kind = 'public' ORDER BY c.name",
    )
    .fetch_all(pool)
    .await
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Message {
    pub id: i64,
    pub channel_id: i64,
    pub user_id: Option<i64>,
    pub username: Option<String>,
    pub raw: String,
    pub cooked: String,
    pub reply_to_id: Option<i64>,
    pub reply_author: Option<String>,
    pub reply_text: Option<String>,
    pub pinned: bool,
    pub edited: bool,
    pub deleted: bool,
    pub created_at: DateTime<Utc>,
    /// [{emoji, count, users}]
    pub reactions: serde_json::Value,
}

/// Deleted text never leaves here: a removed message comes back empty.
macro_rules! message_select {
    () => {
        "SELECT m.id, m.channel_id, m.user_id, u.username,
    CASE WHEN m.deleted_at IS NULL THEN m.raw ELSE '' END AS raw,
    CASE WHEN m.deleted_at IS NULL THEN m.cooked ELSE '' END AS cooked,
    m.reply_to_id, ru.username AS reply_author,
    CASE WHEN r.deleted_at IS NULL THEN left(r.raw, 120) END AS reply_text,
    m.pinned, m.edited_at IS NOT NULL AS edited, m.deleted_at IS NOT NULL AS deleted, m.created_at,
    coalesce((SELECT jsonb_agg(jsonb_build_object('emoji', g.emoji, 'count', g.n, 'users', g.names) ORDER BY g.emoji)
              FROM (SELECT x.emoji, count(*) AS n, array_agg(xu.username) AS names
                    FROM chat_reactions x JOIN users xu ON xu.id = x.user_id
                    WHERE x.message_id = m.id GROUP BY x.emoji) g), '[]'::jsonb) AS reactions
    FROM chat_messages m LEFT JOIN users u ON u.id = m.user_id
    LEFT JOIN chat_messages r ON r.id = m.reply_to_id LEFT JOIN users ru ON ru.id = r.user_id"
    };
}

pub async fn message(pool: &PgPool, id: i64) -> Result<Option<Message>, Error> {
    // Only a fixed column list or select prefix (a constant) is interpolated; every value is bound.
    sqlx::query_as(concat!(message_select!(), " WHERE m.id = $1")).bind(id).fetch_optional(pool).await
}

/// A page of messages, newest first; `search` and `pinned` narrow it.
pub async fn messages(
    pool: &PgPool,
    channel: i64,
    before: Option<i64>,
    search: Option<&str>,
    pinned: bool,
) -> Result<Vec<Message>, Error> {
    // Only a fixed column list or select prefix (a constant) is interpolated; every value is bound.
    sqlx::query_as(concat!(
        message_select!(),
        " WHERE m.channel_id = $1 AND ($2::bigint IS NULL OR m.id < $2)
           AND ($3::text IS NULL OR (m.deleted_at IS NULL AND m.raw ILIKE '%' || $3 || '%' ESCAPE '\\'))
           AND (NOT $4 OR (m.pinned AND m.deleted_at IS NULL))
         ORDER BY m.id DESC LIMIT 50"
    ))
    .bind(channel)
    .bind(before)
    .bind(search.map(|s| s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")))
    .bind(pinned)
    .fetch_all(pool)
    .await
}

/// A reply must point into the same channel; anything else is dropped.
pub async fn send(
    pool: &PgPool,
    channel: i64,
    user: i64,
    raw: &str,
    cooked: &str,
    reply_to: Option<i64>,
) -> Result<i64, Error> {
    let mut tx = pool.begin().await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO chat_messages (channel_id, user_id, raw, cooked, reply_to_id)
         VALUES ($1, $2, $3, $4, (SELECT id FROM chat_messages WHERE id = $5 AND channel_id = $1)) RETURNING id",
    )
    .bind(channel)
    .bind(user)
    .bind(raw)
    .bind(cooked)
    .bind(reply_to)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO chat_members (channel_id, user_id, last_read_id) VALUES ($1, $2, $3)
         ON CONFLICT (channel_id, user_id) DO UPDATE SET last_read_id = EXCLUDED.last_read_id",
    )
    .bind(channel)
    .bind(user)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(id)
}

pub async fn sent_last_minute(pool: &PgPool, user: i64) -> Result<i64, Error> {
    sqlx::query_scalar(
        "SELECT count(*) FROM chat_messages WHERE user_id = $1 AND created_at > now() - interval '1 minute'",
    )
    .bind(user)
    .fetch_one(pool)
    .await
}

pub async fn edit(pool: &PgPool, id: i64, raw: &str, cooked: &str) -> Result<(), Error> {
    sqlx::query(
        "UPDATE chat_messages SET raw = $2, cooked = $3, edited_at = now() WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(raw)
    .bind(cooked)
    .execute(pool)
    .await
    .map(|_| ())
}

pub async fn delete(pool: &PgPool, id: i64, by: i64) -> Result<(), Error> {
    sqlx::query("UPDATE chat_messages SET deleted_at = now(), deleted_by = $2 WHERE id = $1 AND deleted_at IS NULL")
        .bind(id)
        .bind(by)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn toggle_reaction(pool: &PgPool, message: i64, user: i64, emoji: &str) -> Result<(), Error> {
    let removed = sqlx::query("DELETE FROM chat_reactions WHERE message_id = $1 AND user_id = $2 AND emoji = $3")
        .bind(message)
        .bind(user)
        .bind(emoji)
        .execute(pool)
        .await?
        .rows_affected();
    if removed == 0 {
        sqlx::query(
            "INSERT INTO chat_reactions (message_id, user_id, emoji) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
        )
        .bind(message)
        .bind(user)
        .bind(emoji)
        .execute(pool)
        .await?;
    }
    Ok(())
}

pub async fn toggle_pin(pool: &PgPool, id: i64) -> Result<(), Error> {
    sqlx::query("UPDATE chat_messages SET pinned = NOT pinned WHERE id = $1").bind(id).execute(pool).await.map(|_| ())
}

/// Records a flag and returns how many different members have flagged this message.
pub async fn flag(pool: &PgPool, message: i64, user: i64, reason: &str) -> Result<i64, Error> {
    sqlx::query("INSERT INTO chat_flags (message_id, user_id, reason) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING")
        .bind(message)
        .bind(user)
        .bind(reason)
        .execute(pool)
        .await?;
    sqlx::query_scalar("SELECT count(*) FROM chat_flags WHERE message_id = $1").bind(message).fetch_one(pool).await
}

/// Takes a message out of view until staff decide.
pub async fn auto_hide(pool: &PgPool, id: i64) -> Result<(), Error> {
    sqlx::query("UPDATE chat_messages SET deleted_at = now(), auto_hidden = true WHERE id = $1 AND deleted_at IS NULL")
        .bind(id)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Messages by this member hidden by flags in the last day.
pub async fn hidden_today(pool: &PgPool, user: i64) -> Result<i64, Error> {
    sqlx::query_scalar("SELECT count(*) FROM chat_messages WHERE user_id = $1 AND auto_hidden AND deleted_at > now() - interval '1 day'")
        .bind(user)
        .fetch_one(pool)
        .await
}

/// An automatic silence never shortens one the member already has (a longer or endless one stays).
pub async fn auto_silence(pool: &PgPool, user: i64, hours: i32) -> Result<(), Error> {
    sqlx::query("UPDATE users SET silenced_until = greatest(coalesce(silenced_until, '-infinity'), now() + make_interval(hours => $2)) WHERE id = $1")
        .bind(user)
        .bind(hours)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Staff's answer to flags: keep brings a hidden message back, otherwise it stays gone. False,
/// and nothing changed, when nobody flagged the message: staff decide on reports, they do not
/// reach into channels they cannot see.
pub async fn decide_flags(pool: &PgPool, id: i64, keep: bool, by: i64) -> Result<bool, Error> {
    let mut tx = pool.begin().await?;
    let flagged: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM chat_flags WHERE message_id = $1)")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    if !flagged {
        return Ok(false);
    }
    if keep {
        sqlx::query("UPDATE chat_messages SET deleted_at = CASE WHEN auto_hidden THEN NULL ELSE deleted_at END, auto_hidden = false WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
    } else {
        sqlx::query("UPDATE chat_messages SET deleted_at = coalesce(deleted_at, now()), deleted_by = $2, auto_hidden = false WHERE id = $1")
            .bind(id)
            .bind(by)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("DELETE FROM chat_flags WHERE message_id = $1").bind(id).execute(&mut *tx).await?;
    record(
        &mut *tx,
        Entry::new(if keep { "chat.flag.keep" } else { "chat.flag.remove" }).by(by).on(format!("chat_message:{id}")),
    )
    .await?;
    tx.commit().await?;
    Ok(true)
}

/// Seconds since this member's last chat message.
pub async fn seconds_since_last(pool: &PgPool, user: i64) -> Result<Option<i64>, Error> {
    sqlx::query_scalar(
        "SELECT extract(epoch FROM now() - max(created_at))::bigint FROM chat_messages WHERE user_id = $1",
    )
    .bind(user)
    .fetch_one(pool)
    .await
}

pub async fn is_duplicate(pool: &PgPool, channel: i64, user: i64, raw: &str, seconds: i32) -> Result<bool, Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM chat_messages WHERE channel_id = $1 AND user_id = $2 AND raw = $3
           AND created_at > now() - make_interval(secs => $4))",
    )
    .bind(channel)
    .bind(user)
    .bind(raw)
    .bind(f64::from(seconds))
    .fetch_one(pool)
    .await
}

/// Someone else in this conversation ignores the sender.
pub async fn ignored_in(pool: &PgPool, channel: i64, user: i64) -> Result<bool, Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM chat_members m JOIN ignored_users i ON i.user_id = m.user_id
           WHERE m.channel_id = $1 AND i.ignored_user_id = $2)",
    )
    .bind(channel)
    .bind(user)
    .fetch_one(pool)
    .await
}

pub async fn ignored_names(pool: &PgPool, user: i64) -> Result<Vec<String>, Error> {
    sqlx::query_scalar(
        "SELECT u.username FROM ignored_users i JOIN users u ON u.id = i.ignored_user_id WHERE i.user_id = $1",
    )
    .bind(user)
    .fetch_all(pool)
    .await
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Flag {
    pub message_id: i64,
    pub channel_id: i64,
    pub author: Option<String>,
    pub flagged_by: String,
    pub reason: String,
    pub raw: String,
}

pub async fn flags(pool: &PgPool) -> Result<Vec<Flag>, Error> {
    sqlx::query_as(
        "SELECT f.message_id, m.channel_id, a.username AS author, u.username AS flagged_by, f.reason, m.raw
         FROM chat_flags f JOIN chat_messages m ON m.id = f.message_id JOIN users u ON u.id = f.user_id
         LEFT JOIN users a ON a.id = m.user_id WHERE m.deleted_at IS NULL OR m.auto_hidden ORDER BY f.created_at DESC LIMIT 200",
    )
    .fetch_all(pool)
    .await
}

/// Who should see an unread badge move: joined members who have not muted the channel.
pub async fn listeners(pool: &PgPool, channel: i64) -> Result<Vec<i64>, Error> {
    sqlx::query_scalar("SELECT user_id FROM chat_members WHERE channel_id = $1 AND NOT muted LIMIT 5000")
        .bind(channel)
        .fetch_all(pool)
        .await
}

/// Retention: 0 keeps everything. Direct messages have their own period.
pub async fn purge(pool: &PgPool, channel_days: i32, dm_days: i32) -> Result<u64, Error> {
    Ok(sqlx::query(
        "DELETE FROM chat_messages m USING chat_channels c WHERE c.id = m.channel_id AND m.created_at <
           now() - make_interval(days => CASE WHEN c.kind = 'dm' THEN nullif($2, 0) ELSE nullif($1, 0) END)",
    )
    .bind(channel_days)
    .bind(dm_days)
    .execute(pool)
    .await?
    .rows_affected())
}

#[derive(Debug, Clone, FromRow)]
pub struct Rule {
    pub id: i64,
    pub provider: String,
    pub url: String,
    pub token: String,
    pub channel: String,
    pub filter: String,
    pub node_id: Option<i64>,
    pub tag: String,
    pub enabled: bool,
}

/// All rules, with their address and token opened.
pub async fn rules(pool: &PgPool) -> Result<Vec<Rule>, Error> {
    let rows: Vec<Rule> = sqlx::query_as(
        "SELECT id, provider, url, token, channel, filter, node_id, tag, enabled FROM chat_integration_rules ORDER BY id",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|mut r| {
            r.url = bf_core::secretbox::open(&r.url).unwrap_or_default();
            r.token = bf_core::secretbox::open(&r.token).unwrap_or_default();
            r
        })
        .collect())
}

/// Saves a rule. `id` None adds one. An empty url or token on an update keeps the stored one, so
/// the admin page never has to show them.
pub async fn save_rule(pool: &PgPool, id: Option<i64>, r: &Rule) -> Result<i64, Error> {
    let seal = |s: &str| if s.is_empty() { String::new() } else { bf_core::secretbox::seal(s) };
    sqlx::query_scalar(match id {
        None => "INSERT INTO chat_integration_rules (provider, url, token, channel, filter, node_id, tag, enabled)
                 VALUES ($2, $3, $4, $5, $6, $7, $8, $9) RETURNING id",
        Some(_) => "UPDATE chat_integration_rules SET provider = $2, url = coalesce(nullif($3, ''), url),
                    token = coalesce(nullif($4, ''), token), channel = $5, filter = $6, node_id = $7, tag = $8, enabled = $9
                    WHERE id = $1 RETURNING id",
    })
    .bind(id)
    .bind(&r.provider)
    .bind(seal(&r.url))
    .bind(seal(&r.token))
    .bind(&r.channel)
    .bind(&r.filter)
    .bind(r.node_id)
    .bind(&r.tag)
    .bind(r.enabled)
    .fetch_one(pool)
    .await
}

pub async fn delete_rule(pool: &PgPool, id: i64) -> Result<bool, Error> {
    Ok(sqlx::query("DELETE FROM chat_integration_rules WHERE id = $1").bind(id).execute(pool).await?.rows_affected()
        > 0)
}

/// Slugs of a topic's tags, for rules limited to one.
pub async fn topic_tags(pool: &PgPool, item: i64) -> Result<Vec<String>, Error> {
    sqlx::query_scalar("SELECT tg.slug FROM item_tags it JOIN tags tg ON tg.id = it.tag_id WHERE it.item_id = $1")
        .bind(item)
        .fetch_all(pool)
        .await
}
