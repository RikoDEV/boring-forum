//! EU Digital Services Act records: Art. 16 notices, Art. 17 statements of reasons,
//! Art. 20 appeals, and the counts for the Art. 15/24 transparency report.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{FromRow, PgPool};

type Error = sqlx::Error;

pub struct NewNotice<'a> {
    pub url: &'a str,
    pub post_id: Option<i64>,
    pub category: &'a str,
    pub explanation: &'a str,
    pub legal_ground: &'a str,
    pub notifier_name: &'a str,
    pub notifier_email: &'a str,
    pub reporter_id: Option<i64>,
    pub good_faith: bool,
}

/// How many notices an email address sent in the last day.
pub async fn notices_from_email_today(pool: &PgPool, email: &str) -> Result<i64, Error> {
    sqlx::query_scalar(
        "SELECT count(*) FROM legal_notices WHERE lower(notifier_email) = lower($1) AND created_at > now() - interval '1 day'",
    )
    .bind(email)
    .fetch_one(pool)
    .await
}

pub async fn insert_notice(pool: &PgPool, n: &NewNotice<'_>) -> Result<i64, Error> {
    sqlx::query_scalar(
        "INSERT INTO legal_notices (url, post_id, category, explanation, legal_ground, notifier_name, notifier_email,
                                    reporter_id, good_faith)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING id",
    )
    .bind(n.url)
    .bind(n.post_id)
    .bind(n.category)
    .bind(n.explanation)
    .bind(n.legal_ground)
    .bind(n.notifier_name)
    .bind(n.notifier_email)
    .bind(n.reporter_id)
    .bind(n.good_faith)
    .fetch_one(pool)
    .await
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Notice {
    pub id: i64,
    pub url: String,
    pub post_id: Option<i64>,
    pub category: String,
    pub explanation: String,
    pub legal_ground: String,
    pub notifier_name: String,
    pub notifier_email: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

macro_rules! notice_cols {
    () => {
        "id, url, post_id, category, explanation, legal_ground, notifier_name, notifier_email, status, created_at"
    };
}

pub async fn open_notices(pool: &PgPool, limit: i64) -> Result<Vec<Notice>, Error> {
    // Only a fixed column list or select prefix (a constant) is interpolated; every value is bound.
    sqlx::query_as(concat!(
        "SELECT ",
        notice_cols!(),
        " FROM legal_notices WHERE status = 'open' ORDER BY created_at LIMIT $1"
    ))
    .bind(limit)
    .fetch_all(pool)
    .await
}

pub async fn notice(pool: &PgPool, id: i64) -> Result<Option<Notice>, Error> {
    // Only a fixed column list or select prefix (a constant) is interpolated; every value is bound.
    sqlx::query_as(concat!("SELECT ", notice_cols!(), " FROM legal_notices WHERE id = $1"))
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// Closes an open notice. False when it was already decided.
pub async fn decide_notice(pool: &PgPool, id: i64, status: &str, decision: &str, by: i64) -> Result<bool, Error> {
    let n = sqlx::query(
        "UPDATE legal_notices SET status = $2, decision = $3, decided_by = $4, decided_at = now()
         WHERE id = $1 AND status = 'open'",
    )
    .bind(id)
    .bind(status)
    .bind(decision)
    .bind(by)
    .execute(pool)
    .await?;
    Ok(n.rows_affected() == 1)
}

pub struct NewStatement<'a> {
    pub user_id: i64,
    pub target_type: &'a str,
    pub target_id: i64,
    pub action: &'a str,
    pub ground: &'a str,
    pub category: &'a str,
    pub facts: &'a str,
    pub automated_detection: bool,
    pub automated_decision: bool,
    pub notice_id: Option<i64>,
    pub moderator_id: Option<i64>,
}

pub async fn insert_statement(pool: &PgPool, s: &NewStatement<'_>) -> Result<i64, Error> {
    sqlx::query_scalar(
        "INSERT INTO moderation_statements (user_id, target_type, target_id, action, ground, category, facts,
            automated_detection, automated_decision, notice_id, moderator_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) RETURNING id",
    )
    .bind(s.user_id)
    .bind(s.target_type)
    .bind(s.target_id)
    .bind(s.action)
    .bind(s.ground)
    .bind(s.category)
    .bind(s.facts)
    .bind(s.automated_detection)
    .bind(s.automated_decision)
    .bind(s.notice_id)
    .bind(s.moderator_id)
    .fetch_one(pool)
    .await
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Statement {
    pub id: i64,
    pub user_id: Option<i64>,
    pub username: Option<String>,
    pub target_type: String,
    pub target_id: i64,
    pub action: String,
    pub ground: String,
    pub category: String,
    pub facts: String,
    pub automated_detection: bool,
    pub automated_decision: bool,
    pub notice_id: Option<i64>,
    pub moderator_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub appeal_text: Option<String>,
    pub appealed_at: Option<DateTime<Utc>>,
    pub appeal_outcome: Option<String>,
    pub appeal_decided_at: Option<DateTime<Utc>>,
}

macro_rules! statement_cols {
    () => {
        "s.id, s.user_id, u.username, s.target_type, s.target_id, s.action, s.ground, s.category,
    s.facts, s.automated_detection, s.automated_decision, s.notice_id, s.moderator_id, s.created_at, s.appeal_text,
    s.appealed_at, s.appeal_outcome, s.appeal_decided_at"
    };
}

pub async fn statement(pool: &PgPool, id: i64) -> Result<Option<Statement>, Error> {
    // Only a fixed column list or select prefix (a constant) is interpolated; every value is bound.
    sqlx::query_as(concat!(
        "SELECT ",
        statement_cols!(),
        " FROM moderation_statements s LEFT JOIN users u ON u.id = s.user_id WHERE s.id = $1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await
}

/// Files an appeal: only by the affected member, once, within six months (Art. 20(1)).
pub async fn appeal(pool: &PgPool, id: i64, user_id: i64, text: &str) -> Result<bool, Error> {
    let n = sqlx::query(
        "UPDATE moderation_statements SET appeal_text = $3, appealed_at = now()
         WHERE id = $1 AND user_id = $2 AND appealed_at IS NULL AND created_at > now() - interval '6 months'",
    )
    .bind(id)
    .bind(user_id)
    .bind(text)
    .execute(pool)
    .await?;
    Ok(n.rows_affected() == 1)
}

pub async fn pending_appeals(pool: &PgPool, limit: i64) -> Result<Vec<Statement>, Error> {
    // Only a fixed column list or select prefix (a constant) is interpolated; every value is bound.
    sqlx::query_as(concat!(
        "SELECT ",
        statement_cols!(),
        " FROM moderation_statements s LEFT JOIN users u ON u.id = s.user_id
         WHERE s.appealed_at IS NOT NULL AND s.appeal_outcome IS NULL ORDER BY s.appealed_at LIMIT $1"
    ))
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// Decides an appeal. The moderator who made the original decision may not decide it
/// (Art. 20(6): not by the same person alone). False when not allowed or already decided.
pub async fn decide_appeal(pool: &PgPool, id: i64, outcome: &str, by: i64) -> Result<bool, Error> {
    let n = sqlx::query(
        "UPDATE moderation_statements SET appeal_outcome = $2, appeal_decided_by = $3, appeal_decided_at = now()
         WHERE id = $1 AND appealed_at IS NOT NULL AND appeal_outcome IS NULL AND moderator_id IS DISTINCT FROM $3",
    )
    .bind(id)
    .bind(outcome)
    .bind(by)
    .execute(pool)
    .await?;
    Ok(n.rows_affected() == 1)
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Transparency {
    pub notices: i64,
    pub notices_by_category: Vec<(String, i64)>,
    pub notices_actioned: i64,
    pub notices_rejected: i64,
    pub notice_median_hours: Option<f64>,
    pub statements: i64,
    pub by_action: Vec<(String, i64)>,
    pub by_ground: Vec<(String, i64)>,
    pub by_category: Vec<(String, i64)>,
    pub automated_detection: i64,
    pub automated_decision: i64,
    pub appeals: i64,
    pub appeals_reversed: i64,
    pub appeals_upheld: i64,
    pub appeal_median_hours: Option<f64>,
}

async fn grouped(
    pool: &PgPool,
    sql: &'static str,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<Vec<(String, i64)>, Error> {
    sqlx::query_as(sql).bind(from).bind(to).fetch_all(pool).await
}

pub async fn transparency(pool: &PgPool, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<Transparency, Error> {
    let (notices, actioned, rejected, notice_median): (i64, i64, i64, Option<f64>) = sqlx::query_as(
        "SELECT count(*), count(*) FILTER (WHERE status = 'actioned'), count(*) FILTER (WHERE status = 'rejected'),
                percentile_cont(0.5) WITHIN GROUP (ORDER BY extract(epoch FROM decided_at - created_at) / 3600)
                  FILTER (WHERE decided_at IS NOT NULL)
         FROM legal_notices WHERE created_at >= $1 AND created_at < $2",
    )
    .bind(from)
    .bind(to)
    .fetch_one(pool)
    .await?;
    let (statements, det, dec, appeals, reversed, upheld, appeal_median): (i64, i64, i64, i64, i64, i64, Option<f64>) =
        sqlx::query_as(
            "SELECT count(*), count(*) FILTER (WHERE automated_detection), count(*) FILTER (WHERE automated_decision),
                    count(*) FILTER (WHERE appealed_at IS NOT NULL),
                    count(*) FILTER (WHERE appeal_outcome = 'reversed'), count(*) FILTER (WHERE appeal_outcome = 'upheld'),
                    percentile_cont(0.5) WITHIN GROUP (ORDER BY extract(epoch FROM appeal_decided_at - appealed_at) / 3600)
                      FILTER (WHERE appeal_decided_at IS NOT NULL)
             FROM moderation_statements WHERE created_at >= $1 AND created_at < $2",
        )
        .bind(from)
        .bind(to)
        .fetch_one(pool)
        .await?;
    Ok(Transparency {
        notices,
        notices_by_category: grouped(
            pool,
            "SELECT category, count(*) FROM legal_notices WHERE created_at >= $1 AND created_at < $2 GROUP BY 1 ORDER BY 2 DESC",
            from,
            to,
        )
        .await?,
        notices_actioned: actioned,
        notices_rejected: rejected,
        notice_median_hours: notice_median,
        statements,
        by_action: grouped(
            pool,
            "SELECT action, count(*) FROM moderation_statements WHERE created_at >= $1 AND created_at < $2 GROUP BY 1 ORDER BY 2 DESC",
            from,
            to,
        )
        .await?,
        by_ground: grouped(
            pool,
            "SELECT ground, count(*) FROM moderation_statements WHERE created_at >= $1 AND created_at < $2 GROUP BY 1 ORDER BY 2 DESC",
            from,
            to,
        )
        .await?,
        by_category: grouped(
            pool,
            "SELECT category, count(*) FROM moderation_statements WHERE created_at >= $1 AND created_at < $2 GROUP BY 1 ORDER BY 2 DESC",
            from,
            to,
        )
        .await?,
        automated_detection: det,
        automated_decision: dec,
        appeals,
        appeals_reversed: reversed,
        appeals_upheld: upheld,
        appeal_median_hours: appeal_median,
    })
}

/// The post a forum URL points at: `/t/<slug>/<id>#post-<n>` or `/p/<id>`.
pub async fn post_for_topic(pool: &PgPool, topic: i64, number: i32) -> Result<Option<i64>, Error> {
    sqlx::query_scalar("SELECT id FROM posts WHERE item_id = $1 AND post_number = $2")
        .bind(topic)
        .bind(number)
        .fetch_optional(pool)
        .await
}
