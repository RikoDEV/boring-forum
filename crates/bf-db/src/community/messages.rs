//! Private messages. Every read goes through `participant_of` or filters on the user, so
//! access follows membership and nothing else: staff have no way in here.

use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool};

type Error = sqlx::Error;

#[derive(Debug, Clone, FromRow)]
pub struct InboxRow {
    /// `id|username` of each other participant.
    pub people: Vec<String>,
    pub preview: Option<String>,
    pub message_count: i64,
    pub id: i64,
    pub subject: String,
    pub last_message_at: DateTime<Utc>,
    pub unread: bool,
    pub others: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct Head {
    pub id: i64,
    pub subject: String,
    pub starter_id: Option<i64>,
    pub archived: bool,
    pub last_read_id: i64,
}

#[derive(Debug, Clone, FromRow)]
pub struct Message {
    pub id: i64,
    pub author: Option<String>,
    pub cooked: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct Participant {
    pub user_id: i64,
    pub username: String,
    pub last_read_id: i64,
}

pub async fn inbox(
    pool: &PgPool,
    user: i64,
    archived: bool,
    query: &str,
    folder: &str,
) -> Result<Vec<InboxRow>, Error> {
    sqlx::query_as(
        "SELECT c.id, c.subject, c.last_message_at, c.last_message_id > me.last_read_id AS unread,
                coalesce((SELECT string_agg(u.username, ', ' ORDER BY u.username)
                          FROM conversation_participants o JOIN users u ON u.id = o.user_id
                          WHERE o.conversation_id = c.id AND o.user_id <> $1 AND o.left_at IS NULL), '') AS others,
                coalesce((SELECT array_agg(o.user_id || '|' || u.username ORDER BY u.username)
                          FROM conversation_participants o JOIN users u ON u.id = o.user_id
                          WHERE o.conversation_id = c.id AND o.user_id <> $1 AND o.left_at IS NULL), '{}'::text[]) AS people,
                (SELECT m.cooked FROM conversation_messages m WHERE m.conversation_id = c.id ORDER BY m.id DESC LIMIT 1) AS preview,
                (SELECT count(*) FROM conversation_messages m WHERE m.conversation_id = c.id) AS message_count
         FROM conversation_participants me JOIN conversations c ON c.id = me.conversation_id
         WHERE me.user_id = $1 AND me.left_at IS NULL AND me.archived = $2 AND ($4 = '' OR me.folder = $4)
           AND ($3 = '' OR position(lower($3) IN lower(c.subject)) > 0
                OR EXISTS (SELECT 1 FROM conversation_messages m
                           WHERE m.conversation_id = c.id AND position(lower($3) IN lower(m.raw)) > 0))
         ORDER BY c.last_message_at DESC LIMIT 100",
    )
    .bind(user)
    .bind(archived)
    .bind(query)
    .bind(folder)
    .fetch_all(pool)
    .await
}

/// `None` unless this user is a current participant.
pub async fn participant_of(pool: &PgPool, id: i64, user: i64) -> Result<Option<Head>, Error> {
    sqlx::query_as(
        "SELECT c.id, c.subject, c.starter_id, me.archived, me.last_read_id
         FROM conversations c JOIN conversation_participants me ON me.conversation_id = c.id
         WHERE c.id = $1 AND me.user_id = $2 AND me.left_at IS NULL",
    )
    .bind(id)
    .bind(user)
    .fetch_optional(pool)
    .await
}

pub async fn messages(pool: &PgPool, id: i64) -> Result<Vec<Message>, Error> {
    sqlx::query_as(
        "SELECT * FROM (
           SELECT m.id, u.username AS author, m.cooked, m.created_at
           FROM conversation_messages m LEFT JOIN users u ON u.id = m.author_id
           WHERE m.conversation_id = $1 ORDER BY m.id DESC LIMIT 1000
         ) newest ORDER BY id",
    )
    .bind(id)
    .fetch_all(pool)
    .await
}

pub async fn participants(pool: &PgPool, id: i64) -> Result<Vec<Participant>, Error> {
    sqlx::query_as(
        "SELECT p.user_id, u.username, p.last_read_id
         FROM conversation_participants p JOIN users u ON u.id = p.user_id
         WHERE p.conversation_id = $1 AND p.left_at IS NULL ORDER BY u.username",
    )
    .bind(id)
    .fetch_all(pool)
    .await
}

/// Those among `candidates` who have chosen to ignore `sender`.
pub async fn ignoring(pool: &PgPool, sender: i64, candidates: &[i64]) -> Result<Vec<i64>, Error> {
    sqlx::query_scalar("SELECT user_id FROM ignored_users WHERE ignored_user_id = $1 AND user_id = ANY($2)")
        .bind(sender)
        .bind(candidates)
        .fetch_all(pool)
        .await
}

/// Those among `candidates` whose "who may message me" setting keeps `sender` out: nobody, or
/// only members they follow. Staff are never kept out.
pub async fn restricting(pool: &PgPool, sender: i64, candidates: &[i64]) -> Result<Vec<i64>, Error> {
    sqlx::query_scalar(
        "SELECT u.id FROM users u
         WHERE u.id = ANY($2) AND (u.allow_pm_from = 'nobody'
               OR (u.allow_pm_from = 'following'
                   AND NOT EXISTS (SELECT 1 FROM user_follows f WHERE f.user_id = u.id AND f.target_id = $1)))
           AND NOT EXISTS (SELECT 1 FROM users s WHERE s.id = $1 AND (s.primary_group_id IN (3, 4)
                   OR EXISTS (SELECT 1 FROM user_secondary_groups g WHERE g.user_id = s.id AND g.group_id IN (3, 4))))",
    )
    .bind(sender)
    .bind(candidates)
    .fetch_all(pool)
    .await
}

/// Conversations this user started in the last hour.
/// Seconds since this member last wrote in any conversation.
pub async fn seconds_since_last_message(pool: &PgPool, user: i64) -> Result<Option<i64>, Error> {
    sqlx::query_scalar(
        "SELECT extract(epoch FROM now() - max(created_at))::bigint FROM conversation_messages WHERE author_id = $1",
    )
    .bind(user)
    .fetch_one(pool)
    .await
}

pub async fn started_last_hour(pool: &PgPool, user: i64) -> Result<i64, Error> {
    sqlx::query_scalar(
        "SELECT count(*) FROM conversations WHERE starter_id = $1 AND created_at > now() - interval '1 hour'",
    )
    .bind(user)
    .fetch_one(pool)
    .await
}

pub async fn create(
    pool: &PgPool,
    starter: i64,
    subject: &str,
    raw: &str,
    cooked: &str,
    recipients: &[i64],
) -> Result<i64, Error> {
    Ok(create_capped(pool, starter, subject, raw, cooked, recipients, None).await?.unwrap_or(0))
}

/// [`create`] under the hourly limit: `cap` is `(per hour, already used by other means)`. When the
/// starter has begun that many conversations in the last hour it returns `None`. The count and the
/// insert run under one per-member lock, so two requests at once cannot both pass.
pub async fn create_capped(
    pool: &PgPool,
    starter: i64,
    subject: &str,
    raw: &str,
    cooked: &str,
    recipients: &[i64],
    cap: Option<(i64, i64)>,
) -> Result<Option<i64>, Error> {
    let mut tx = pool.begin().await?;
    if let Some((per_hour, extra)) = cap {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext('pm_per_hour:' || $1::text))")
            .bind(starter)
            .execute(&mut *tx)
            .await?;
        let started: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM conversations WHERE starter_id = $1 AND created_at > now() - interval '1 hour'",
        )
        .bind(starter)
        .fetch_one(&mut *tx)
        .await?;
        if started + extra >= per_hour {
            return Ok(None);
        }
    }
    let id: i64 = sqlx::query_scalar("INSERT INTO conversations (subject, starter_id) VALUES ($1, $2) RETURNING id")
        .bind(subject)
        .bind(starter)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO conversation_participants (conversation_id, user_id)
         SELECT $1, u FROM unnest($2::bigint[]) AS u ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(recipients)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO conversation_participants (conversation_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(starter)
    .execute(&mut *tx)
    .await?;
    insert_message(&mut tx, id, starter, raw, cooked).await?;
    tx.commit().await?;
    Ok(Some(id))
}

async fn insert_message(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: i64,
    author: i64,
    raw: &str,
    cooked: &str,
) -> Result<(), Error> {
    let message: i64 = sqlx::query_scalar(
        "INSERT INTO conversation_messages (conversation_id, author_id, raw, cooked) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(id)
    .bind(author)
    .bind(raw)
    .bind(cooked)
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query("UPDATE conversations SET last_message_id = $2, last_message_at = now() WHERE id = $1")
        .bind(id)
        .bind(message)
        .execute(&mut **tx)
        .await?;
    // A new message brings an archived conversation back, and the author has read their own.
    sqlx::query(
        "UPDATE conversation_participants SET archived = false,
           last_read_id = CASE WHEN user_id = $2 THEN $3 ELSE last_read_id END
         WHERE conversation_id = $1 AND left_at IS NULL",
    )
    .bind(id)
    .bind(author)
    .bind(message)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub async fn reply(pool: &PgPool, id: i64, author: i64, raw: &str, cooked: &str) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    insert_message(&mut tx, id, author, raw, cooked).await?;
    tx.commit().await
}

pub async fn mark_read(pool: &PgPool, id: i64, user: i64) -> Result<(), Error> {
    sqlx::query(
        "UPDATE conversation_participants p SET last_read_id = c.last_message_id FROM conversations c
         WHERE c.id = p.conversation_id AND p.conversation_id = $1 AND p.user_id = $2
           AND p.last_read_id <> c.last_message_id",
    )
    .bind(id)
    .bind(user)
    .execute(pool)
    .await
    .map(|_| ())
}

pub async fn mark_unread(pool: &PgPool, id: i64, user: i64) -> Result<(), Error> {
    sqlx::query("UPDATE conversation_participants SET last_read_id = 0 WHERE conversation_id = $1 AND user_id = $2")
        .bind(id)
        .bind(user)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn set_archived(pool: &PgPool, id: i64, user: i64, archived: bool) -> Result<(), Error> {
    sqlx::query("UPDATE conversation_participants SET archived = $3 WHERE conversation_id = $1 AND user_id = $2")
        .bind(id)
        .bind(user)
        .bind(archived)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn leave(pool: &PgPool, id: i64, user: i64) -> Result<(), Error> {
    sqlx::query("UPDATE conversation_participants SET left_at = now() WHERE conversation_id = $1 AND user_id = $2")
        .bind(id)
        .bind(user)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Adds someone, or brings back someone who left. They can read the earlier messages.
pub async fn add_participant(pool: &PgPool, id: i64, user: i64) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO conversation_participants (conversation_id, user_id) VALUES ($1, $2)
         ON CONFLICT (conversation_id, user_id) DO UPDATE SET left_at = NULL",
    )
    .bind(id)
    .bind(user)
    .execute(pool)
    .await
    .map(|_| ())
}

/// The starter removing someone else.
pub async fn remove_participant(pool: &PgPool, id: i64, user: i64) -> Result<(), Error> {
    leave(pool, id, user).await
}

pub async fn set_folder(pool: &PgPool, conversation: i64, user: i64, folder: &str) -> Result<(), Error> {
    sqlx::query(
        "UPDATE conversation_participants SET folder = NULLIF($3, '') WHERE conversation_id = $1 AND user_id = $2",
    )
    .bind(conversation)
    .bind(user)
    .bind(folder)
    .execute(pool)
    .await
    .map(|_| ())
}

pub async fn folders(pool: &PgPool, user: i64) -> Result<Vec<String>, Error> {
    sqlx::query_scalar(
        "SELECT DISTINCT folder FROM conversation_participants WHERE user_id = $1 AND left_at IS NULL AND folder IS NOT NULL ORDER BY folder LIMIT 50",
    )
    .bind(user)
    .fetch_all(pool)
    .await
}

pub async fn report_conversation(pool: &PgPool, conversation: i64, reporter: i64, reason: &str) -> Result<(), Error> {
    sqlx::query("INSERT INTO conversation_reports (conversation_id, reporter_id, reason) VALUES ($1, $2, $3)")
        .bind(conversation)
        .bind(reporter)
        .bind(reason)
        .execute(pool)
        .await
        .map(|_| ())
}

#[derive(Debug, Clone, FromRow)]
pub struct ConversationReport {
    pub id: i64,
    pub conversation_id: i64,
    pub reporter: Option<String>,
    pub reason: String,
    pub created_at: DateTime<Utc>,
}

pub async fn open_conversation_report_count(pool: &PgPool) -> Result<i64, Error> {
    sqlx::query_scalar("SELECT count(*) FROM conversation_reports WHERE resolved_at IS NULL").fetch_one(pool).await
}

pub async fn open_conversation_reports(pool: &PgPool) -> Result<Vec<ConversationReport>, Error> {
    sqlx::query_as(
        "SELECT r.id, r.conversation_id, u.username AS reporter, r.reason, r.created_at FROM conversation_reports r
         LEFT JOIN users u ON u.id = r.reporter_id WHERE r.resolved_at IS NULL ORDER BY r.id LIMIT 200",
    )
    .fetch_all(pool)
    .await
}

pub async fn conversation_report(pool: &PgPool, id: i64) -> Result<Option<ConversationReport>, Error> {
    sqlx::query_as(
        "SELECT r.id, r.conversation_id, u.username AS reporter, r.reason, r.created_at FROM conversation_reports r
         LEFT JOIN users u ON u.id = r.reporter_id WHERE r.id = $1 AND r.resolved_at IS NULL",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub async fn resolve_conversation_report(pool: &PgPool, id: i64, by: i64) -> Result<bool, Error> {
    Ok(sqlx::query(
        "UPDATE conversation_reports SET resolved_at = now(), resolved_by = $2 WHERE id = $1 AND resolved_at IS NULL",
    )
    .bind(id)
    .bind(by)
    .execute(pool)
    .await?
    .rows_affected()
        > 0)
}
