//! Custom pages made in the admin panel.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{FromRow, PgPool};

type Error = sqlx::Error;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Page {
    pub id: i64,
    pub slug: String,
    pub title: String,
    /// The page's layout, one of `TEMPLATES`.
    pub template: String,
    pub visibility: String,
    pub published: bool,
    pub updated_at: DateTime<Utc>,
    /// An address of its own, such as `/about`, instead of `/page/{slug}`.
    pub path: Option<String>,
}

macro_rules! columns {
    () => {
        "id, slug, title, template, visibility, published, updated_at, path"
    };
}

/// The layouts a page can have.
pub const TEMPLATES: [&str; 4] = ["single", "sidebar", "columns2", "columns3"];

/// The areas of a layout (`page12_{place}`): blocks go into these. Every layout has a top and a bottom.
pub fn places(template: &str) -> &'static [&'static str] {
    match template {
        "sidebar" => &["top", "main", "side", "bottom"],
        "columns2" => &["top", "col1", "col2", "bottom"],
        "columns3" => &["top", "col1", "col2", "col3", "bottom"],
        _ => &["top", "main", "bottom"],
    }
}

/// The area that takes blocks whose own area a layout does not have.
fn first_body_place(template: &str) -> &'static str {
    if template.starts_with("columns") { "col1" } else { "main" }
}

pub async fn template_of(pool: &PgPool, id: i64) -> Result<Option<String>, Error> {
    sqlx::query_scalar("SELECT template FROM custom_pages WHERE id = $1").bind(id).fetch_optional(pool).await
}

/// After a page changes layout: blocks in areas the new layout lacks move to its first body area.
pub async fn move_stray_widgets(pool: &PgPool, id: i64, template: &str) -> Result<(), Error> {
    let keep: Vec<String> = places(template).iter().map(|s| (*s).to_owned()).collect();
    sqlx::query(
        "UPDATE widgets SET area = 'page' || $1 || '_' || $3
         WHERE area LIKE 'page' || $1 || '\\_%' AND substring(area from '_([a-z0-9]+)$') <> ALL($2)",
    )
    .bind(id.to_string())
    .bind(&keep)
    .bind(first_body_place(template))
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn all(pool: &PgPool) -> Result<Vec<Page>, Error> {
    sqlx::query_as(concat!("SELECT ", columns!(), " FROM custom_pages ORDER BY title"))
        .fetch_all(pool)
        .await
}

pub async fn by_slug(pool: &PgPool, slug: &str) -> Result<Option<Page>, Error> {
    sqlx::query_as(concat!("SELECT ", columns!(), " FROM custom_pages WHERE slug = $1"))
        .bind(slug)
        .fetch_optional(pool)
        .await
}

#[derive(Debug)]
pub struct PageChange<'a> {
    pub slug: &'a str,
    pub title: &'a str,
    pub template: &'a str,
    pub visibility: &'a str,
    pub published: bool,
    pub path: Option<&'a str>,
}

pub async fn create(pool: &PgPool, p: &PageChange<'_>) -> Result<i64, Error> {
    sqlx::query_scalar(
        "INSERT INTO custom_pages (slug, title, template, visibility, published, path)
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    )
    .bind(p.slug)
    .bind(p.title)
    .bind(p.template)
    .bind(p.visibility)
    .bind(p.published)
    .bind(p.path)
    .fetch_one(pool)
    .await
}

pub async fn update(pool: &PgPool, id: i64, p: &PageChange<'_>) -> Result<bool, Error> {
    Ok(sqlx::query(
        "UPDATE custom_pages SET slug = $2, title = $3, template = $4, visibility = $5, published = $6,
           path = $7, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(p.slug)
    .bind(p.title)
    .bind(p.template)
    .bind(p.visibility)
    .bind(p.published)
    .bind(p.path)
    .execute(pool)
    .await?
    .rows_affected()
        > 0)
}

/// Every custom address: (path, slug).
pub async fn custom_paths(pool: &PgPool) -> Result<Vec<(String, String)>, Error> {
    sqlx::query_as("SELECT path, slug FROM custom_pages WHERE path IS NOT NULL").fetch_all(pool).await
}

crate::sql::delete_by_id!(delete, "custom_pages");
