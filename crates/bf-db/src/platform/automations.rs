//! Automation rules.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
pub use sqlx::Error;
use sqlx::{FromRow, PgPool};

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Rule {
    #[serde(default)]
    pub id: i64,
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    pub trigger: String,
    #[serde(default)]
    pub conditions: serde_json::Value,
    #[serde(default)]
    pub actions: serde_json::Value,
    #[serde(default)]
    pub last_run_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub run_count: i64,
}

fn yes() -> bool {
    true
}

pub async fn rules(pool: &PgPool) -> Result<Vec<Rule>, Error> {
    sqlx::query_as(
        "SELECT id, name, enabled, trigger, conditions, actions, last_run_at, run_count FROM automations ORDER BY id",
    )
    .fetch_all(pool)
    .await
}

/// Enabled rules for one trigger.
pub async fn for_trigger(pool: &PgPool, trigger: &str) -> Result<Vec<Rule>, Error> {
    sqlx::query_as(
        "SELECT id, name, enabled, trigger, conditions, actions, last_run_at, run_count FROM automations
         WHERE enabled AND trigger = $1 ORDER BY id",
    )
    .bind(trigger)
    .fetch_all(pool)
    .await
}

/// Scheduled rules whose period has passed since they last ran, each claimed so that only one
/// server in a cluster runs it.
pub async fn claim_due(pool: &PgPool) -> Result<Vec<Rule>, Error> {
    sqlx::query_as(
        "UPDATE automations SET last_run_at = now(), run_count = run_count + 1
         WHERE enabled AND trigger IN ('daily', 'weekly', 'monthly')
           AND (last_run_at IS NULL OR last_run_at <= now() - CASE trigger WHEN 'daily' THEN interval '1 day'
                                                               WHEN 'weekly' THEN interval '7 days'
                                                               ELSE interval '1 month' END)
         RETURNING id, name, enabled, trigger, conditions, actions, last_run_at, run_count",
    )
    .fetch_all(pool)
    .await
}

pub async fn ran(pool: &PgPool, id: i64) -> Result<(), Error> {
    sqlx::query("UPDATE automations SET last_run_at = now(), run_count = run_count + 1 WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn save(pool: &PgPool, id: Option<i64>, r: &Rule) -> Result<Option<i64>, Error> {
    match id {
        None => {
            sqlx::query_scalar(
                "INSERT INTO automations (name, enabled, trigger, conditions, actions) VALUES ($1, $2, $3, $4, $5)
             RETURNING id",
            )
            .bind(&r.name)
            .bind(r.enabled)
            .bind(&r.trigger)
            .bind(&r.conditions)
            .bind(&r.actions)
            .fetch_optional(pool)
            .await
        }
        Some(id) => {
            sqlx::query_scalar(
                "UPDATE automations SET name = $2, enabled = $3, trigger = $4, conditions = $5, actions = $6
             WHERE id = $1 RETURNING id",
            )
            .bind(id)
            .bind(&r.name)
            .bind(r.enabled)
            .bind(&r.trigger)
            .bind(&r.conditions)
            .bind(&r.actions)
            .fetch_optional(pool)
            .await
        }
    }
}

crate::sql::delete_by_id!(delete, "automations");
