//! Colour schemes: palettes an admin edits and members choose between.

use serde::Serialize;
pub use sqlx::Error;
use sqlx::types::Json;
use sqlx::{FromRow, PgPool};

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Scheme {
    pub id: i64,
    pub slug: String,
    pub name: String,
    pub light: Json<serde_json::Value>,
    pub dark: Json<serde_json::Value>,
    pub selectable: bool,
    pub position: i32,
}

macro_rules! cols {
    () => {
        "id, slug, name, light, dark, selectable, position"
    };
}

pub async fn all(pool: &PgPool) -> Result<Vec<Scheme>, Error> {
    sqlx::query_as(concat!("SELECT ", cols!(), " FROM color_schemes ORDER BY position, id"))
        .fetch_all(pool)
        .await
}

pub async fn insert(
    pool: &PgPool,
    slug: &str,
    name: &str,
    light: &serde_json::Value,
    dark: &serde_json::Value,
) -> Result<Scheme, Error> {
    sqlx::query_as(concat!("INSERT INTO color_schemes (slug, name, light, dark, position)
         VALUES ($1, $2, $3, $4, coalesce((SELECT max(position) + 1 FROM color_schemes), 0)) RETURNING ", cols!()))
    .bind(slug)
    .bind(name)
    .bind(Json(light))
    .bind(Json(dark))
    .fetch_one(pool)
    .await
}

pub async fn update(
    pool: &PgPool,
    id: i64,
    name: &str,
    light: &serde_json::Value,
    dark: &serde_json::Value,
    selectable: bool,
) -> Result<bool, Error> {
    sqlx::query("UPDATE color_schemes SET name = $2, light = $3, dark = $4, selectable = $5 WHERE id = $1")
        .bind(id)
        .bind(name)
        .bind(Json(light))
        .bind(Json(dark))
        .bind(selectable)
        .execute(pool)
        .await
        .map(|r| r.rows_affected() > 0)
}

/// Deletes a scheme and lets go of the members who had picked it (they get the site's default).
pub async fn delete(pool: &PgPool, id: i64) -> Result<Option<String>, Error> {
    let mut tx = pool.begin().await?;
    let slug: Option<String> = sqlx::query_scalar("DELETE FROM color_schemes WHERE id = $1 RETURNING slug")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    if let Some(slug) = &slug {
        sqlx::query("UPDATE users SET color_scheme = NULL WHERE color_scheme = $1").bind(slug).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(slug)
}

pub async fn slug_taken(pool: &PgPool, slug: &str) -> Result<bool, Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM color_schemes WHERE slug = $1)").bind(slug).fetch_one(pool).await
}
