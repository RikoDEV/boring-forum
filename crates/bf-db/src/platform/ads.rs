//! Advertisements.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
pub use sqlx::Error;
use sqlx::{FromRow, PgPool};

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Ad {
    #[serde(default)]
    pub id: i64,
    pub name: String,
    pub placement: String,
    pub html: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub show_groups: Vec<i64>,
    #[serde(default)]
    pub hide_groups: Vec<i64>,
    #[serde(default = "five")]
    pub every: i32,
    #[serde(default = "one")]
    pub weight: i32,
    #[serde(default)]
    pub starts_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub ends_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub impressions: i64,
    #[serde(default)]
    pub clicks: i64,
}

fn five() -> i32 {
    5
}

fn one() -> i32 {
    1
}

macro_rules! ad_sql {
    () => {
        "SELECT id, name, placement, html, url, show_groups, hide_groups, every, weight, starts_at, ends_at, active,
            impressions, clicks FROM ads"
    };
}

pub async fn all(pool: &PgPool) -> Result<Vec<Ad>, Error> {
    sqlx::query_as(concat!(ad_sql!(), " ORDER BY placement, name")).fetch_all(pool).await
}

/// Ads that may show now.
pub async fn live(pool: &PgPool) -> Result<Vec<Ad>, Error> {
    sqlx::query_as(concat!(
        ad_sql!(),
        " WHERE active AND (starts_at IS NULL OR starts_at <= now()) AND (ends_at IS NULL OR ends_at > now())"
    ))
    .fetch_all(pool)
    .await
}

pub async fn save(pool: &PgPool, id: Option<i64>, a: &Ad) -> Result<Option<i64>, Error> {
    match id {
        None => sqlx::query_scalar(
            "INSERT INTO ads (name, placement, html, url, show_groups, hide_groups, every, weight, starts_at, ends_at, active)
             SELECT $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11 WHERE $12::bigint IS NULL RETURNING id",
        ),
        Some(_) => sqlx::query_scalar(
            "UPDATE ads SET name = $1, placement = $2, html = $3, url = $4, show_groups = $5, hide_groups = $6, every = $7,
               weight = $8, starts_at = $9, ends_at = $10, active = $11 WHERE id = $12 RETURNING id",
        ),
    }
    .bind(&a.name)
    .bind(&a.placement)
    .bind(&a.html)
    .bind(&a.url)
    .bind(&a.show_groups)
    .bind(&a.hide_groups)
    .bind(a.every)
    .bind(a.weight)
    .bind(a.starts_at)
    .bind(a.ends_at)
    .bind(a.active)
    .bind(id)
    .fetch_optional(pool)
    .await
}

crate::sql::delete_by_id!(delete, "ads");

/// Adds counted impressions, batched by the server between ticks.
pub async fn add_impressions(pool: &PgPool, counts: &[(i64, i64)]) -> Result<(), Error> {
    let (ids, ns): (Vec<i64>, Vec<i64>) = counts.iter().copied().unzip();
    sqlx::query(
        "UPDATE ads a SET impressions = a.impressions + c.n FROM unnest($1::bigint[], $2::bigint[]) AS c(id, n) WHERE a.id = c.id",
    )
    .bind(&ids)
    .bind(&ns)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Where a live ad leads, counting a click when `count` is set. An ad that is switched off,
/// not started or past its end leads nowhere, as with the lists pages are built from.
pub async fn click(pool: &PgPool, id: i64, count: bool) -> Result<Option<String>, Error> {
    sqlx::query_scalar(
        "UPDATE ads SET clicks = clicks + $2::int
         WHERE id = $1 AND url IS NOT NULL AND active
           AND (starts_at IS NULL OR starts_at <= now()) AND (ends_at IS NULL OR ends_at > now())
         RETURNING url",
    )
    .bind(id)
    .bind(i32::from(count))
    .fetch_optional(pool)
    .await
}
