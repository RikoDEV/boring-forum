//! Topic assignment to staff.

use crate::admin::audit::{Entry, record};
use sqlx::PgPool;

type Error = sqlx::Error;

pub async fn set_assignee(pool: &PgPool, item: i64, user: Option<i64>, actor: i64) -> Result<(), Error> {
    sqlx::query("UPDATE items SET assigned_to = $2 WHERE id = $1").bind(item).bind(user).execute(pool).await?;
    record(pool, Entry::new("topic.assign").by(actor).on(item.to_string()).details(serde_json::json!({ "to": user }))).await
}

/// Topics assigned to a member, in forums the viewer can moderate: (id, slug, title).
pub async fn assigned_topics(pool: &PgPool, user: i64, nodes: &[i64]) -> Result<Vec<(i64, String, String)>, Error> {
    sqlx::query_as(
        "SELECT id, slug, title FROM items WHERE assigned_to = $1 AND state = 'visible' AND node_id = ANY($2)
         ORDER BY last_posted_at DESC LIMIT 200",
    )
    .bind(user)
    .bind(nodes)
    .fetch_all(pool)
    .await
}
