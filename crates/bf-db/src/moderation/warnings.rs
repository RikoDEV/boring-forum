//! Warnings and warning points.

use crate::community::promotion::Thresholds;
use chrono::{DateTime, Utc};
use crate::admin::audit::{Entry, record};
use sqlx::{FromRow, PgPool};

type Error = sqlx::Error;

#[derive(Debug, Clone, FromRow)]
pub struct Warning {
    pub moderator: Option<String>,
    pub points: i32,
    pub reason: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// Records a warning, then applies automatic actions from the points still active. Returns the
/// active total.
pub async fn warn(
    pool: &PgPool,
    user: i64,
    moderator: i64,
    points: i32,
    reason: &str,
    expire_days: i32,
    t: &Thresholds,
) -> Result<i64, Error> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO warnings (user_id, moderator_id, points, reason, expires_at)
         VALUES ($1, $2, $3, $4, now() + make_interval(days => $5))",
    )
    .bind(user)
    .bind(moderator)
    .bind(points)
    .bind(reason)
    .bind(expire_days)
    .execute(&mut *tx)
    .await?;
    let total: i64 = sqlx::query_scalar(
        "SELECT coalesce(sum(points), 0)::bigint FROM warnings WHERE user_id = $1 AND expires_at > now()",
    )
    .bind(user)
    .fetch_one(&mut *tx)
    .await?;
    // 0 turns a threshold off.
    for (limit, column) in [(t.silence_points, "silenced_until"), (t.suspend_points, "suspended_until")] {
        if limit > 0 && total >= limit {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE users SET {column} = greatest({column}, now() + make_interval(days => $2)) WHERE id = $1"
            )))
            .bind(user)
            .bind(t.days)
            .execute(&mut *tx)
            .await?;
        }
    }
    record(&mut *tx, Entry::new("user.warn").by(moderator).on(format!("user:{user}")).details(serde_json::json!({ "points": points, "total": total }))).await?;
    tx.commit().await?;
    Ok(total)
}

/// (points still active, currently silenced, currently suspended).
pub async fn warning_state(pool: &PgPool, user: i64) -> Result<(i64, bool, bool), Error> {
    sqlx::query_as(
        "SELECT (SELECT coalesce(sum(points), 0)::bigint FROM warnings WHERE user_id = u.id AND expires_at > now()),
                coalesce(u.silenced_until > now(), false), coalesce(u.suspended_until > now(), false)
         FROM users u WHERE u.id = $1",
    )
    .bind(user)
    .fetch_one(pool)
    .await
}

pub async fn warnings_of(pool: &PgPool, user: i64) -> Result<Vec<Warning>, Error> {
    sqlx::query_as(
        "SELECT m.username AS moderator, w.points, w.reason, w.created_at, w.expires_at
         FROM warnings w LEFT JOIN users m ON m.id = w.moderator_id
         WHERE w.user_id = $1 ORDER BY w.id DESC LIMIT 200",
    )
    .bind(user)
    .fetch_all(pool)
    .await
}
