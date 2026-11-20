//! Public member lists and the staff directory.

use crate::community::promotion::REAL;
use sqlx::PgPool;

type Error = sqlx::Error;

/// One directory row: (username, posts, reputation).
pub type MemberRow = (String, i32, i64);

/// Public directory. `by_reputation` makes it the leaderboard.
pub async fn members(pool: &PgPool, by_reputation: bool, limit: i64) -> Result<Vec<MemberRow>, Error> {
    let order =
        if by_reputation { "u.reputation DESC, u.post_count DESC" } else { "u.post_count DESC, u.reputation DESC" };
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT u.username, u.post_count, u.reputation FROM users u
         WHERE {REAL} AND u.approved AND u.profile_visibility = 'everyone' AND (u.suspended_until IS NULL OR u.suspended_until < now())
         ORDER BY {order}, u.id LIMIT $1"
    )))
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// Staff directory: (username, group name). Primary groups 3 (moderators) and 4 (admins).
pub async fn staff(pool: &PgPool) -> Result<Vec<(String, String)>, Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT u.username, g.name FROM users u JOIN groups g ON g.id = u.primary_group_id
         WHERE {REAL} AND u.primary_group_id IN (3, 4) ORDER BY u.primary_group_id DESC, u.username LIMIT 200"
    )))
    .fetch_all(pool)
    .await
}
