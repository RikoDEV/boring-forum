//! Report reasons staff can edit.

use serde::{Deserialize, Serialize};
pub use sqlx::Error;
use sqlx::{FromRow, PgPool};

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Reason {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub hint: String,
    #[serde(default = "one")]
    pub weight: f32,
    #[serde(default)]
    pub needs_note: bool,
    #[serde(default)]
    pub illegal: bool,
    /// Instead of a report, the reporter's note goes to the post's author as a private message.
    #[serde(default)]
    pub message_user: bool,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub position: i32,
    /// The plugin that declared this reason: it is managed there.
    #[serde(default, skip_deserializing)]
    pub plugin: Option<String>,
}

fn one() -> f32 {
    1.0
}

fn yes() -> bool {
    true
}

pub async fn reasons(pool: &PgPool) -> Result<Vec<Reason>, Error> {
    sqlx::query_as(
        "SELECT key, label, hint, weight, needs_note, illegal, message_user, enabled, position, plugin FROM report_reasons
         ORDER BY position, key",
    )
    .fetch_all(pool)
    .await
}

/// Adds or replaces a reason by key.
pub async fn save(pool: &PgPool, r: &Reason) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO report_reasons (key, label, hint, weight, needs_note, illegal, enabled, position, message_user)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
         ON CONFLICT (key) DO UPDATE SET label = EXCLUDED.label, hint = EXCLUDED.hint, weight = EXCLUDED.weight,
           needs_note = EXCLUDED.needs_note, illegal = EXCLUDED.illegal, enabled = EXCLUDED.enabled,
           message_user = EXCLUDED.message_user,
           position = EXCLUDED.position WHERE report_reasons.plugin IS NULL",
    )
    .bind(&r.key)
    .bind(&r.label)
    .bind(&r.hint)
    .bind(r.weight)
    .bind(r.needs_note)
    .bind(r.illegal)
    .bind(r.enabled)
    .bind(r.position)
    .bind(r.message_user)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Removes a reason. `other` stays: it is the fallback for a reason that no longer exists.
pub async fn delete(pool: &PgPool, key: &str) -> Result<bool, Error> {
    Ok(sqlx::query("DELETE FROM report_reasons WHERE key = $1 AND key <> 'other' AND plugin IS NULL")
        .bind(key)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}
