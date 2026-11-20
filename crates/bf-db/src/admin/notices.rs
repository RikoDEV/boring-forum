//! Notices for administrators about site problems.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{FromRow, PgPool};

type Error = sqlx::Error;

pub async fn add_notice(pool: &PgPool, kind: &str, message: &str) -> Result<(), Error> {
    // One open notice per message: a job failing all night should not flood the dashboard.
    sqlx::query(
        "WITH added AS (
           INSERT INTO admin_notices (kind, message) SELECT $1, $2
           WHERE NOT EXISTS (SELECT 1 FROM admin_notices WHERE message = $2 AND dismissed_at IS NULL) RETURNING 1)
         INSERT INTO notifications (user_id, kind, data)
         SELECT u.id, 'admin_notice', jsonb_build_object('message', $2::text, 'url', '/admin')
         FROM users u, added WHERE u.primary_group_id = 4",
    )
    .bind(kind)
    .bind(message)
    .execute(pool)
    .await
    .map(|_| ())
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Notice {
    pub id: i64,
    pub kind: String,
    pub message: String,
    pub created_at: DateTime<Utc>,
}

pub async fn notices(pool: &PgPool) -> Result<Vec<Notice>, Error> {
    sqlx::query_as(
        "SELECT id, kind, message, created_at FROM admin_notices WHERE dismissed_at IS NULL ORDER BY id DESC LIMIT 100",
    )
    .fetch_all(pool)
    .await
}

pub async fn dismiss_notice(pool: &PgPool, id: i64) -> Result<bool, Error> {
    Ok(sqlx::query("UPDATE admin_notices SET dismissed_at = now() WHERE id = $1 AND dismissed_at IS NULL")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}
