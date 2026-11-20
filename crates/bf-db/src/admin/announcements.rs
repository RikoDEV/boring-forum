//! Site announcements.

use crate::community::promotion::yes;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};

type Error = sqlx::Error;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Announcement {
    #[serde(default)]
    pub id: i64,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub body_html: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    pub starts_at: Option<DateTime<Utc>>,
    pub ends_at: Option<DateTime<Utc>>,
}

pub async fn announcements(pool: &PgPool) -> Result<Vec<Announcement>, Error> {
    sqlx::query_as("SELECT id, title, body, body_html, enabled, starts_at, ends_at FROM announcements ORDER BY id DESC")
        .fetch_all(pool)
        .await
}

pub async fn save_announcement(pool: &PgPool, id: i64, a: &Announcement) -> Result<Option<i64>, Error> {
    let sql = if id == 0 {
        "INSERT INTO announcements (title, body, body_html, enabled, starts_at, ends_at)
         SELECT $2, $3, $4, $5, $6, $7 WHERE $1 = 0 RETURNING id"
    } else {
        "UPDATE announcements SET title = $2, body = $3, body_html = $4, enabled = $5, starts_at = $6, ends_at = $7
         WHERE id = $1 RETURNING id"
    };
    sqlx::query_scalar(sql)
        .bind(id)
        .bind(&a.title)
        .bind(&a.body)
        .bind(&a.body_html)
        .bind(a.enabled)
        .bind(a.starts_at)
        .bind(a.ends_at)
        .fetch_optional(pool)
        .await
}

crate::sql::delete_by_id!(delete_announcement, "announcements");
