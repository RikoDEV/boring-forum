//! Member writes held for approval outside forum posts.

use crate::admin::audit::{Entry, record};
use chrono::{DateTime, Utc};
pub use sqlx::Error;
use sqlx::{FromRow, PgPool};

#[derive(Debug, Clone, FromRow)]
pub struct Held {
    pub id: i64,
    pub kind: String,
    pub user_id: i64,
    pub author: String,
    pub node_id: Option<i64>,
    pub payload: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

pub async fn hold(
    pool: &PgPool,
    kind: &str,
    user: i64,
    node: Option<i64>,
    payload: &serde_json::Value,
) -> Result<i64, Error> {
    sqlx::query_scalar(
        "INSERT INTO held_content (kind, user_id, node_id, payload) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(kind)
    .bind(user)
    .bind(node)
    .bind(payload)
    .fetch_one(pool)
    .await
}

/// Pending items this reviewer may decide: everything without a forum for global staff, and
/// those in `nodes` (the forums they moderate).
pub async fn pending(pool: &PgPool, staff: bool, nodes: &[i64]) -> Result<Vec<Held>, Error> {
    sqlx::query_as(
        "SELECT h.id, h.kind, h.user_id, u.username AS author, h.node_id, h.payload, h.created_at
         FROM held_content h JOIN users u ON u.id = h.user_id
         WHERE h.status = 'pending' AND ((h.node_id IS NULL AND $1) OR h.node_id = ANY($2))
         ORDER BY h.created_at LIMIT 200",
    )
    .bind(staff)
    .bind(nodes)
    .fetch_all(pool)
    .await
}

pub async fn count(pool: &PgPool, staff: bool, nodes: &[i64]) -> Result<i64, Error> {
    sqlx::query_scalar(
        "SELECT count(*) FROM held_content
         WHERE status = 'pending' AND ((node_id IS NULL AND $1) OR node_id = ANY($2))",
    )
    .bind(staff)
    .bind(nodes)
    .fetch_one(pool)
    .await
}

pub async fn get(pool: &PgPool, id: i64) -> Result<Option<Held>, Error> {
    sqlx::query_as(
        "SELECT h.id, h.kind, h.user_id, u.username AS author, h.node_id, h.payload, h.created_at
         FROM held_content h JOIN users u ON u.id = h.user_id WHERE h.id = $1 AND h.status = 'pending'",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

/// Puts a decided item back in the queue, after its approval could not be carried out.
pub async fn reopen(pool: &PgPool, id: i64) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE held_content SET status = 'pending', resolved_by = NULL, resolved_at = NULL WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

/// Marks a pending item decided. False when someone else decided it first.
pub async fn resolve(pool: &PgPool, id: i64, approved: bool, actor: i64) -> Result<bool, Error> {
    let mut tx = pool.begin().await?;
    let n = sqlx::query(
        "UPDATE held_content SET status = $2, resolved_by = $3, resolved_at = now() WHERE id = $1 AND status = 'pending'",
    )
    .bind(id)
    .bind(if approved { "approved" } else { "rejected" })
    .bind(actor)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if n > 0 {
        record(
            &mut *tx,
            Entry::new(if approved { "held.approve" } else { "held.reject" }).by(actor).on(format!("held:{id}")),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(n > 0)
}
