//! The review queue: reports, pending posts and pending accounts.

use crate::forum::posts::PostRef;
use crate::forum::posts::set_post_state;
use chrono::{DateTime, Utc};
pub use sqlx::Error;
use crate::admin::audit::{Entry, record};
use sqlx::{FromRow, PgPool};

#[derive(Debug, Clone, FromRow)]
pub struct ReviewRow {
    pub id: i64,
    pub kind: String,
    pub score: f32,
    pub created_at: DateTime<Utc>,
    pub post_id: i64,
    pub post_number: i32,
    pub cooked: String,
    /// The Markdown source, for a moderator who edits it before approving.
    pub raw: String,
    pub author: Option<String>,
    pub item_id: i64,
    pub title: String,
    pub slug: String,
    pub node_id: i64,
    pub reasons: Option<Vec<String>>,
    pub claimed_by: Option<String>,
}

/// Pending review items in the given forums (the moderator's scope), filtered and ordered.
pub async fn review_queue(
    pool: &PgPool,
    nodes: &[i64],
    kind: Option<&str>,
    oldest_first: bool,
) -> Result<Vec<ReviewRow>, Error> {
    sqlx::query_as(
        "SELECT r.id, r.kind, r.score, r.created_at, p.id AS post_id, p.post_number, p.cooked, p.raw, u.username AS author,
                i.id AS item_id, i.title, i.slug, i.node_id,
                (SELECT array_agg(DISTINCT reason) FROM reports
                  WHERE target_type = 'post' AND target_id = p.id AND status = 'open') AS reasons,
                (SELECT username FROM users cu WHERE cu.id = r.claimed_by) AS claimed_by
         FROM reviewables r JOIN posts p ON r.target_type = 'post' AND p.id = r.target_id
         JOIN items i ON i.id = p.item_id LEFT JOIN users u ON u.id = p.author_id
         WHERE r.status = 'pending' AND r.node_id = ANY($1) AND ($2::text IS NULL OR r.kind = $2)
         ORDER BY CASE WHEN $3 THEN 0 ELSE r.score END DESC, r.created_at LIMIT 100",
    )
    .bind(nodes)
    .bind(kind)
    .bind(oldest_first)
    .fetch_all(pool)
    .await
}

/// The forum a pending review item belongs to, for the permission check.
pub async fn reviewable_node(pool: &PgPool, id: i64) -> Result<Option<i64>, Error> {
    sqlx::query_scalar("SELECT node_id FROM reviewables WHERE id = $1 AND status = 'pending'")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map(Option::flatten)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Pending post: publish. Report: dismiss and keep the post.
    Keep,
    /// Pending post: reject. Report: remove the post.
    Remove,
}

/// Resolves a review item: updates the post, closes its reports and logs the action.
pub async fn resolve_review(pool: &PgPool, id: i64, decision: Decision, moderator: i64) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    let (kind, post_id): (String, i64) =
        sqlx::query_as("SELECT kind, target_id FROM reviewables WHERE id = $1 AND status = 'pending' FOR UPDATE")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    match decision {
        Decision::Keep if kind == "pending_post" => set_post_state(&mut tx, post_id, "visible", None).await?,
        Decision::Keep => {
            sqlx::query("UPDATE reviewables SET score = 0 WHERE id = $1").bind(id).execute(&mut *tx).await?;
            let hidden: bool =
                sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM posts WHERE id = $1 AND state = 'hidden')")
                    .bind(post_id)
                    .fetch_one(&mut *tx)
                    .await?;
            if hidden {
                set_post_state(&mut tx, post_id, "visible", None).await?;
            }
        }
        Decision::Remove => set_post_state(&mut tx, post_id, "deleted", Some(moderator)).await?,
    }
    let status = if decision == Decision::Keep { "approved" } else { "rejected" };
    sqlx::query("UPDATE reviewables SET status = $2, resolved_by = $3, resolved_at = now() WHERE id = $1")
        .bind(id)
        .bind(status)
        .bind(moderator)
        .execute(&mut *tx)
        .await?;
    let report_status = if decision == Decision::Keep { "dismissed" } else { "resolved" };
    sqlx::query("UPDATE reports SET status = $2 WHERE target_type = 'post' AND target_id = $1 AND status = 'open'")
        .bind(post_id)
        .bind(report_status)
        .execute(&mut *tx)
        .await?;
    record(&mut *tx, Entry::new(format!("review.{status}")).by(moderator).on(format!("post:{post_id}")).details(serde_json::json!({ "kind": kind }))).await?;
    tx.commit().await
}

/// Files a report. A member's repeat report on the same post updates its reason without
/// raising the score. Returns the post's report score afterwards.
pub async fn report(
    pool: &PgPool,
    reporter: i64,
    post: &PostRef,
    reason: &str,
    note: Option<&str>,
    reason_weight: f32,
) -> Result<f32, Error> {
    let mut tx = pool.begin().await?;
    let new: bool = sqlx::query_scalar(
        "INSERT INTO reports (target_type, target_id, reporter_id, reason, note) VALUES ('post', $1, $2, $3, $4)
         ON CONFLICT (target_type, target_id, reporter_id) WHERE status = 'open'
         DO UPDATE SET reason = EXCLUDED.reason, note = EXCLUDED.note
         RETURNING xmax = 0",
    )
    .bind(post.id)
    .bind(reporter)
    .bind(reason)
    .bind(note)
    .fetch_one(&mut *tx)
    .await?;
    // A flagger's accuracy weighs their report: upheld vs dismissed, from 0.25 to 2.
    let weight: f32 = sqlx::query_scalar(
        "SELECT least(2, greatest(0.25, (count(*) FILTER (WHERE status = 'resolved') + 1)::real
                                       / (count(*) FILTER (WHERE status = 'dismissed') + 1)::real))::real
         FROM reports WHERE reporter_id = $1",
    )
    .bind(reporter)
    .fetch_one(&mut *tx)
    .await?;
    let score: f32 = sqlx::query_scalar(
        "INSERT INTO reviewables (kind, target_type, target_id, node_id, score) VALUES ('report', 'post', $1, $2, $4::real)
         ON CONFLICT (target_type, target_id, kind)
         DO UPDATE SET score = reviewables.score + CASE WHEN $3 THEN $4::real ELSE 0 END, status = 'pending'
         RETURNING score",
    )
    .bind(post.id)
    .bind(post.node_id)
    .bind(new)
    // The reporter's track record times how serious the chosen reason is.
    .bind(weight * reason_weight)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(score)
}

pub async fn claim_review(pool: &PgPool, id: i64, user: Option<i64>) -> Result<bool, Error> {
    Ok(sqlx::query("UPDATE reviewables SET claimed_by = $2 WHERE id = $1 AND status = 'pending'")
        .bind(id)
        .bind(user)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

/// Accounts waiting for staff approval (pending users in the review queue).
pub async fn pending_users(pool: &PgPool) -> Result<Vec<(i64, String, String, DateTime<Utc>)>, Error> {
    sqlx::query_as("SELECT id, username, email, joined_at FROM users WHERE NOT approved ORDER BY joined_at LIMIT 100")
        .fetch_all(pool)
        .await
}

/// Approves a pending account. False when no pending account matched.
pub async fn approve_user(pool: &PgPool, user_id: i64, actor: i64) -> Result<bool, Error> {
    let mut tx = pool.begin().await?;
    let n = sqlx::query("UPDATE users SET approved = true WHERE id = $1 AND NOT approved")
        .bind(user_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if n > 0 {
        record(&mut *tx, Entry::new("user.approve").by(actor).on(format!("user:{user_id}"))).await?;
    }
    tx.commit().await?;
    Ok(n > 0)
}

/// Rejects a pending account: deletes it with anything it wrote and, when `ban_email`, bans its
/// address so it cannot sign up again. Staff accounts are never rejected here. False when no
/// pending account matched.
pub async fn reject_user(pool: &PgPool, user_id: i64, ban_email: bool, actor: i64) -> Result<bool, Error> {
    let email: Option<String> = sqlx::query_scalar(
        "SELECT email FROM users WHERE id = $1 AND NOT approved AND primary_group_id NOT IN (3, 4)
           AND NOT EXISTS (SELECT 1 FROM user_secondary_groups WHERE user_id = $1 AND group_id IN (3, 4))",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    let Some(email) = email else { return Ok(false) };
    if ban_email && !email.trim().is_empty() {
        crate::moderation::bans::add(pool, "email", email.trim(), Some("Rejected at sign-up"), Some(actor)).await?;
    }
    record(pool, Entry::new("user.reject").by(actor).on(format!("user:{user_id}")).details(serde_json::json!({ "ban_email": ban_email }))).await?;
    Ok(crate::moderation::tools::delete_user(pool, user_id, true, actor).await? == Some(true))
}

/// Reports a member (their profile, bio, signature or statuses). A repeat report by the same
/// member updates the reason. False when the reporter reports themselves.
pub async fn report_member(
    pool: &PgPool,
    user_id: i64,
    reporter: i64,
    reason: &str,
    status: Option<i64>,
) -> Result<bool, Error> {
    if user_id == reporter {
        return Ok(false);
    }
    sqlx::query(
        "INSERT INTO member_reports (user_id, reporter_id, reason, status_id) VALUES ($1, $2, $3, $4)
         ON CONFLICT (user_id, reporter_id) WHERE resolved_at IS NULL
         DO UPDATE SET reason = EXCLUDED.reason, status_id = EXCLUDED.status_id",
    )
    .bind(user_id)
    .bind(reporter)
    .bind(reason)
    .bind(status)
    .execute(pool)
    .await?;
    Ok(true)
}

/// Open member reports: (report id, member id, member, reporter, reason, filed, status).
#[allow(clippy::type_complexity)]
pub async fn open_member_reports(
    pool: &PgPool,
) -> Result<Vec<(i64, i64, String, Option<String>, String, DateTime<Utc>, Option<i64>)>, Error> {
    sqlx::query_as(
        "SELECT r.id, r.user_id, u.username, x.username, r.reason, r.created_at, r.status_id FROM member_reports r
         JOIN users u ON u.id = r.user_id LEFT JOIN users x ON x.id = r.reporter_id
         WHERE r.resolved_at IS NULL ORDER BY r.id LIMIT 200",
    )
    .fetch_all(pool)
    .await
}

/// Closes a member report. False when it was not open.
pub async fn close_member_report(pool: &PgPool, id: i64, actor: i64) -> Result<bool, Error> {
    let n = sqlx::query(
        "UPDATE member_reports SET resolved_at = now(), resolved_by = $2 WHERE id = $1 AND resolved_at IS NULL",
    )
    .bind(id)
    .bind(actor)
    .execute(pool)
    .await?
    .rows_affected();
    if n > 0 {
        record(pool, Entry::new("member_report.close").by(actor).on(format!("member_report:{id}"))).await?;
    }
    Ok(n > 0)
}

/// Items only global staff handle: pending accounts, member reports and reported conversations.
pub async fn staff_queue_count(pool: &PgPool) -> Result<i64, Error> {
    sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM users WHERE NOT approved)
              + (SELECT count(*) FROM member_reports WHERE resolved_at IS NULL)
              + (SELECT count(*) FROM conversation_reports WHERE resolved_at IS NULL)",
    )
    .fetch_one(pool)
    .await
}

/// Members who reported a post, to tell them the report was handled.
pub async fn reporters(pool: &PgPool, post_id: i64) -> Result<Vec<i64>, Error> {
    sqlx::query_scalar("SELECT DISTINCT reporter_id FROM reports WHERE target_type = 'post' AND target_id = $1 AND reporter_id IS NOT NULL")
        .bind(post_id)
        .fetch_all(pool)
        .await
}
