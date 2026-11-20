//! Status updates on member profiles and their replies.

use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool};

type Error = sqlx::Error;

#[derive(Debug, Clone, FromRow)]
pub struct Status {
    pub id: i64,
    pub user_id: i64,
    pub body: String,
    pub locked: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct Reply {
    pub id: i64,
    pub status_id: i64,
    pub author_id: i64,
    pub author: String,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

pub async fn create(pool: &PgPool, user: i64, body: &str) -> Result<i64, Error> {
    sqlx::query_scalar("INSERT INTO member_statuses (user_id, body) VALUES ($1, $2) RETURNING id")
        .bind(user)
        .bind(body)
        .fetch_one(pool)
        .await
}

pub async fn get(pool: &PgPool, id: i64) -> Result<Option<Status>, Error> {
    sqlx::query_as("SELECT id, user_id, body, locked, created_at FROM member_statuses WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// The newest statuses of a member, each with all its replies.
pub async fn feed(pool: &PgPool, user: i64, limit: i64) -> Result<(Vec<Status>, Vec<Reply>), Error> {
    let statuses: Vec<Status> = sqlx::query_as(
        "SELECT id, user_id, body, locked, created_at FROM member_statuses WHERE user_id = $1 ORDER BY id DESC LIMIT $2",
    )
    .bind(user)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    let ids: Vec<i64> = statuses.iter().map(|s| s.id).collect();
    let replies = sqlx::query_as(
        "SELECT r.id, r.status_id, r.author_id, u.username AS author, r.body, r.created_at
         FROM member_status_replies r JOIN users u ON u.id = r.author_id
         WHERE r.status_id = ANY($1) ORDER BY r.id",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await?;
    Ok((statuses, replies))
}

/// The newest statuses of members whose profile anyone may see: (name, text, when).
pub async fn latest_public(pool: &PgPool, limit: i64) -> Result<Vec<(String, String, DateTime<Utc>)>, Error> {
    sqlx::query_as(
        "SELECT u.username, s.body, s.created_at FROM member_statuses s JOIN users u ON u.id = s.user_id
         WHERE u.profile_visibility = 'everyone' ORDER BY s.id DESC LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await
}

pub async fn delete(pool: &PgPool, id: i64) -> Result<(), Error> {
    sqlx::query("DELETE FROM member_statuses WHERE id = $1").bind(id).execute(pool).await.map(|_| ())
}

crate::sql::set_col!(set_locked, "member_statuses", "locked", i64, bool);

/// Seconds since this member last replied to any status.
pub async fn seconds_since_last_reply(pool: &PgPool, user: i64) -> Result<Option<i64>, Error> {
    sqlx::query_scalar(
        "SELECT extract(epoch FROM now() - max(created_at))::bigint FROM member_status_replies WHERE author_id = $1",
    )
    .bind(user)
    .fetch_one(pool)
    .await
}

/// Adds a reply. Telling the status owner is the caller's job, through the notification sender,
/// which honours their preferences and ignore list.
pub async fn reply(pool: &PgPool, status: &Status, author: i64, body: &str) -> Result<(), Error> {
    sqlx::query("INSERT INTO member_status_replies (status_id, author_id, body) VALUES ($1, $2, $3)")
        .bind(status.id)
        .bind(author)
        .bind(body)
        .execute(pool)
        .await
        .map(|_| ())
}

/// (reply author, status owner, status id).
pub async fn reply_owner(pool: &PgPool, id: i64) -> Result<Option<(i64, i64, i64)>, Error> {
    sqlx::query_as(
        "SELECT r.author_id, s.user_id, s.id FROM member_status_replies r JOIN member_statuses s ON s.id = r.status_id WHERE r.id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub async fn delete_reply(pool: &PgPool, id: i64) -> Result<(), Error> {
    sqlx::query("DELETE FROM member_status_replies WHERE id = $1").bind(id).execute(pool).await.map(|_| ())
}
