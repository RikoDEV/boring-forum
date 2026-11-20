//! Referrals.

use chrono::{DateTime, Utc};
pub use sqlx::Error;
use sqlx::PgPool;

/// A member by name who may refer others: an existing, approved account.
pub async fn referrer(pool: &PgPool, username: &str) -> Result<Option<i64>, Error> {
    sqlx::query_scalar("SELECT id FROM users WHERE username_lower = lower($1) AND approved AND delete_after IS NULL")
        .bind(username)
        .fetch_optional(pool)
        .await
}

/// Records who referred a new member; never themselves, and only once.
pub async fn set_referrer(pool: &PgPool, user: i64, referrer: i64) -> Result<bool, Error> {
    Ok(sqlx::query("UPDATE users SET referred_by = $2 WHERE id = $1 AND referred_by IS NULL AND id <> $2")
        .bind(user)
        .bind(referrer)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

/// A member's referrals: (id, username, joined, qualified).
pub async fn referrals(pool: &PgPool, user: i64) -> Result<Vec<(i64, String, DateTime<Utc>, bool)>, Error> {
    sqlx::query_as(
        "SELECT r.id, r.username, r.joined_at, r.trust_level >= 1 AND r.delete_after IS NULL AND (r.suspended_until IS NULL OR r.suspended_until < now())
         FROM users r WHERE r.referred_by = $1 ORDER BY r.joined_at DESC LIMIT 500",
    )
    .bind(user)
    .fetch_all(pool)
    .await
}

/// Members with at least `count` qualified referrals who have not been rewarded with `group`
/// yet; records the reward, adds them to the group and returns who was added. A reward is given
/// once: a group an administrator later removes is not handed back. Suspended and deleted
/// accounts do not qualify.
pub async fn reward(pool: &PgPool, count: i64, group: i64) -> Result<Vec<i64>, Error> {
    sqlx::query_scalar(
        "WITH granted AS (
           INSERT INTO referral_rewards (user_id, group_id)
           SELECT r.referred_by, $2 FROM users r
            WHERE r.referred_by IS NOT NULL AND r.trust_level >= 1 AND r.delete_after IS NULL
              AND (r.suspended_until IS NULL OR r.suspended_until < now())
            GROUP BY r.referred_by HAVING count(*) >= $1
           ON CONFLICT DO NOTHING RETURNING user_id)
         INSERT INTO user_secondary_groups (user_id, group_id) SELECT user_id, $2 FROM granted
         ON CONFLICT DO NOTHING RETURNING user_id",
    )
    .bind(count)
    .bind(group)
    .fetch_all(pool)
    .await
}

/// Top referrers, for the admin overview: (username, qualified, total).
pub async fn top(pool: &PgPool, limit: i64) -> Result<Vec<(String, i64, i64)>, Error> {
    sqlx::query_as(
        "SELECT u.username, count(*) FILTER (WHERE r.trust_level >= 1 AND r.delete_after IS NULL AND (r.suspended_until IS NULL OR r.suspended_until < now())), count(*) FROM users r
         JOIN users u ON u.id = r.referred_by GROUP BY u.username ORDER BY 2 DESC, 3 DESC LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await
}
