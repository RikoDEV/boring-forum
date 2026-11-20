//! Live topics: sessions, questions and votes.

use chrono::{DateTime, Utc};
pub use sqlx::Error;
use sqlx::{FromRow, PgPool};

#[derive(Debug, Clone, FromRow)]
pub struct Session {
    pub item_id: i64,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub hosts: Vec<i64>,
    pub moderated: bool,
    /// (id, username) of each host.
    #[sqlx(skip)]
    pub host_names: Vec<(i64, String)>,
}

pub async fn session(pool: &PgPool, item: i64) -> Result<Option<Session>, Error> {
    let s: Option<Session> =
        sqlx::query_as("SELECT item_id, starts_at, ends_at, hosts, moderated FROM live_sessions WHERE item_id = $1")
            .bind(item)
            .fetch_optional(pool)
            .await?;
    let Some(mut s) = s else { return Ok(None) };
    s.host_names = sqlx::query_as("SELECT id, username FROM users WHERE id = ANY($1) ORDER BY username")
        .bind(&s.hosts)
        .fetch_all(pool)
        .await?;
    Ok(Some(s))
}

pub async fn save(
    pool: &PgPool,
    item: i64,
    starts: DateTime<Utc>,
    ends: DateTime<Utc>,
    hosts: &[i64],
    moderated: bool,
    by: i64,
) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO live_sessions (item_id, starts_at, ends_at, hosts, moderated, created_by) VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (item_id) DO UPDATE SET starts_at = $2, ends_at = $3, hosts = $4, moderated = $5",
    )
    .bind(item)
    .bind(starts)
    .bind(ends)
    .bind(hosts)
    .bind(moderated)
    .bind(by)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Ends a running session now.
pub async fn end(pool: &PgPool, item: i64) -> Result<(), Error> {
    sqlx::query(
        "UPDATE live_sessions SET ends_at = greatest(starts_at + interval '1 second', now()) WHERE item_id = $1",
    )
    .bind(item)
    .execute(pool)
    .await
    .map(|_| ())
}

pub async fn remove(pool: &PgPool, item: i64) -> Result<(), Error> {
    sqlx::query("DELETE FROM live_sessions WHERE item_id = $1").bind(item).execute(pool).await.map(|_| ())
}

#[derive(Debug, Clone, FromRow)]
pub struct Question {
    pub id: i64,
    pub author_id: Option<i64>,
    pub author: Option<String>,
    pub body: String,
    pub status: String,
    pub votes: i32,
    pub answer_post: Option<i32>,
    pub voted: bool,
    pub created_at: DateTime<Utc>,
}

/// Questions of a session, most votes first. Pending and rejected ones only for those who
/// decide them, and pending ones for their author.
pub async fn questions(pool: &PgPool, item: i64, viewer: Option<i64>, decides: bool) -> Result<Vec<Question>, Error> {
    sqlx::query_as(
        "SELECT q.id, q.author_id, u.username AS author, q.body, q.status, q.votes, q.answer_post,
            ($2::bigint IS NOT NULL AND EXISTS (SELECT 1 FROM live_question_votes v WHERE v.question_id = q.id AND v.user_id = $2)) AS voted,
            q.created_at
         FROM live_questions q LEFT JOIN users u ON u.id = q.author_id
         WHERE q.item_id = $1
           AND (q.status IN ('approved', 'answered') OR $3 OR (q.status = 'pending' AND q.author_id = $2))
         ORDER BY (q.status = 'answered'), (q.status = 'rejected'), q.votes DESC, q.id
         LIMIT 300",
    )
    .bind(item)
    .bind(viewer)
    .bind(decides)
    .fetch_all(pool)
    .await
}

pub async fn ask(pool: &PgPool, item: i64, author: i64, body: &str, approved: bool) -> Result<i64, Error> {
    sqlx::query_scalar(
        "INSERT INTO live_questions (item_id, author_id, body, status) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(item)
    .bind(author)
    .bind(body)
    .bind(if approved { "approved" } else { "pending" })
    .fetch_one(pool)
    .await
}

/// How many questions this member sent to a session in the last ten minutes.
pub async fn recent_questions(pool: &PgPool, item: i64, author: i64) -> Result<i64, Error> {
    sqlx::query_scalar(
        "SELECT count(*) FROM live_questions WHERE item_id = $1 AND author_id = $2 AND created_at > now() - interval '10 minutes'",
    )
    .bind(item)
    .bind(author)
    .fetch_one(pool)
    .await
}

/// The session a question belongs to, and where it stands: (item, author, status).
pub async fn question_item(pool: &PgPool, id: i64) -> Result<Option<(i64, Option<i64>, String)>, Error> {
    sqlx::query_as("SELECT item_id, author_id, status FROM live_questions WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn set_status(pool: &PgPool, id: i64, status: &str, answer_post: Option<i32>) -> Result<(), Error> {
    sqlx::query("UPDATE live_questions SET status = $2, answer_post = COALESCE($3, answer_post) WHERE id = $1")
        .bind(id)
        .bind(status)
        .bind(answer_post)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Toggles a member's vote; returns whether they now vote for it.
pub async fn vote(pool: &PgPool, id: i64, user: i64) -> Result<bool, Error> {
    let mut tx = pool.begin().await?;
    let removed = sqlx::query("DELETE FROM live_question_votes WHERE question_id = $1 AND user_id = $2")
        .bind(id)
        .bind(user)
        .execute(&mut *tx)
        .await?
        .rows_affected()
        > 0;
    if !removed {
        sqlx::query("INSERT INTO live_question_votes (question_id, user_id) VALUES ($1, $2)")
            .bind(id)
            .bind(user)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("UPDATE live_questions SET votes = (SELECT count(*) FROM live_question_votes WHERE question_id = $1) WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(!removed)
}

/// The newest reply by one of the hosts, to link an answered question to.
pub async fn latest_host_post(pool: &PgPool, item: i64, hosts: &[i64]) -> Result<Option<i32>, Error> {
    sqlx::query_scalar(
        "SELECT max(post_number) FROM posts WHERE item_id = $1 AND author_id = ANY($2) AND post_number > 1",
    )
    .bind(item)
    .bind(hosts)
    .fetch_one(pool)
    .await
}

/// Sessions running now or starting within a month, for /live: (item, title, slug, starts, ends).
pub async fn upcoming(
    pool: &PgPool,
    nodes: &[i64],
) -> Result<Vec<(i64, String, String, DateTime<Utc>, DateTime<Utc>)>, Error> {
    sqlx::query_as(
        "SELECT i.id, i.title, i.slug, s.starts_at, s.ends_at FROM live_sessions s JOIN items i ON i.id = s.item_id
         WHERE s.ends_at > now() - interval '7 days' AND s.starts_at < now() + interval '31 days'
           AND i.state = 'visible' AND i.node_id = ANY($1)
         ORDER BY (s.ends_at < now()), s.starts_at LIMIT 100",
    )
    .bind(nodes)
    .fetch_all(pool)
    .await
}
