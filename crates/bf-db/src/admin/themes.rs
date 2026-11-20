//! Themes and widgets.

use serde::Serialize;
use sqlx::{FromRow, PgPool};

type Error = sqlx::Error;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Theme {
    pub id: i64,
    pub slug: String,
    pub name: String,
    pub version: String,
    pub kind: String,
    pub tokens: serde_json::Value,
    pub css: String,
    pub slots: serde_json::Value,
    pub enabled: bool,
    pub selectable: bool,
    pub position: i32,
    /// Schemes this theme shows in light and dark mode; None follows the site's defaults.
    pub light_scheme: Option<String>,
    pub dark_scheme: Option<String>,
    /// Groups the theme is limited to; empty means everyone.
    pub group_ids: Vec<i64>,
}

pub async fn all(pool: &PgPool) -> Result<Vec<Theme>, Error> {
    sqlx::query_as("SELECT id, slug, name, version, kind, tokens, css, slots, enabled, selectable, position, light_scheme, dark_scheme, group_ids
         FROM themes ORDER BY position, id")
        .fetch_all(pool)
        .await
}

pub struct NewTheme<'a> {
    pub slug: &'a str,
    pub name: &'a str,
    pub version: &'a str,
    pub kind: &'a str,
    pub tokens: &'a serde_json::Value,
    pub css: &'a str,
    pub slots: &'a serde_json::Value,
}

/// Installs a theme, or updates the one with this slug.
pub async fn upsert(pool: &PgPool, t: &NewTheme<'_>) -> Result<i64, Error> {
    sqlx::query_scalar(
        "INSERT INTO themes (slug, name, version, kind, tokens, css, slots) VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (slug) DO UPDATE SET name = EXCLUDED.name, version = EXCLUDED.version, kind = EXCLUDED.kind,
           tokens = EXCLUDED.tokens, css = EXCLUDED.css, slots = EXCLUDED.slots RETURNING id",
    )
    .bind(t.slug)
    .bind(t.name)
    .bind(t.version)
    .bind(t.kind)
    .bind(t.tokens)
    .bind(t.css)
    .bind(t.slots)
    .fetch_one(pool)
    .await
}

pub struct ThemeChange {
    pub tokens: Option<serde_json::Value>,
    pub css: Option<String>,
    pub slots: Option<serde_json::Value>,
    pub enabled: Option<bool>,
    pub selectable: Option<bool>,
    pub position: Option<i32>,
    /// A scheme's address name; an empty one clears it, so the theme follows the site again.
    pub light_scheme: Option<String>,
    pub dark_scheme: Option<String>,
    pub group_ids: Option<Vec<i64>>,
}

pub async fn update(pool: &PgPool, id: i64, c: &ThemeChange) -> Result<bool, Error> {
    Ok(sqlx::query(
        "UPDATE themes SET tokens = coalesce($2, tokens), css = coalesce($3, css), slots = coalesce($4, slots),
           enabled = coalesce($5, enabled), selectable = coalesce($6, selectable), position = coalesce($7, position),
           light_scheme = CASE WHEN $8::text IS NULL THEN light_scheme WHEN $8 = '' THEN NULL ELSE $8 END,
           dark_scheme = CASE WHEN $9::text IS NULL THEN dark_scheme WHEN $9 = '' THEN NULL ELSE $9 END,
           group_ids = coalesce($10, group_ids) WHERE id = $1",
    )
    .bind(id)
    .bind(&c.tokens)
    .bind(&c.css)
    .bind(&c.slots)
    .bind(c.enabled)
    .bind(c.selectable)
    .bind(c.position)
    .bind(&c.light_scheme)
    .bind(&c.dark_scheme)
    .bind(&c.group_ids)
    .execute(pool)
    .await?
    .rows_affected()
        > 0)
}

crate::sql::delete_by_id!(delete, "themes");

crate::sql::set_col!(set_user_theme, "users", "theme_id", i64, Option<i64>);

pub async fn set_node_theme(pool: &PgPool, node: i64, theme: Option<i64>) -> Result<bool, Error> {
    Ok(sqlx::query("UPDATE nodes SET theme_id = $2 WHERE id = $1")
        .bind(node)
        .bind(theme)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Widget {
    pub id: i64,
    pub area: String,
    pub kind: String,
    pub title: String,
    pub config: serde_json::Value,
    pub position: i32,
    pub cache_seconds: i32,
}

pub async fn widgets(pool: &PgPool) -> Result<Vec<Widget>, Error> {
    sqlx::query_as(
        "SELECT id, area, kind, title, config, position, cache_seconds FROM widgets ORDER BY area, position, id",
    )
    .fetch_all(pool)
    .await
}

pub async fn add_widget(pool: &PgPool, w: &Widget) -> Result<i64, Error> {
    sqlx::query_scalar(
        "INSERT INTO widgets (area, kind, title, config, position, cache_seconds) VALUES ($1, $2, $3, $4,
           coalesce((SELECT max(position) + 1 FROM widgets WHERE area = $1), 0), $5) RETURNING id",
    )
    .bind(&w.area)
    .bind(&w.kind)
    .bind(&w.title)
    .bind(&w.config)
    .bind(w.cache_seconds)
    .fetch_one(pool)
    .await
}

crate::sql::delete_by_id!(delete_widget, "widgets");

pub async fn update_widget(pool: &PgPool, w: &Widget) -> Result<bool, Error> {
    Ok(sqlx::query("UPDATE widgets SET title = $2, config = $3, cache_seconds = $4 WHERE id = $1")
        .bind(w.id)
        .bind(&w.title)
        .bind(&w.config)
        .bind(w.cache_seconds)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

/// Saves a drag-and-drop result: widget ids in order, per area.
pub async fn arrange_widgets(pool: &PgPool, area: &str, ids: &[i64]) -> Result<(), Error> {
    sqlx::query("UPDATE widgets w SET area = $1, position = o.n FROM unnest($2::bigint[]) WITH ORDINALITY AS o(id, n) WHERE w.id = o.id")
        .bind(area)
        .bind(ids)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn online_users(pool: &PgPool, minutes: i32, limit: i64) -> Result<Vec<String>, Error> {
    sqlx::query_scalar(
        "SELECT username FROM users WHERE approved AND last_seen_at > now() - make_interval(mins => $1) AND NOT hide_last_seen
           AND profile_visibility = 'everyone'
         ORDER BY last_seen_at DESC LIMIT $2",
    )
    .bind(minutes)
    .bind(limit)
    .fetch_all(pool)
    .await
}

pub async fn top_posters(pool: &PgPool, limit: i64) -> Result<Vec<(String, i32)>, Error> {
    sqlx::query_as("SELECT username, post_count FROM users WHERE approved AND post_count > 0 ORDER BY post_count DESC, id LIMIT $1")
        .bind(limit)
        .fetch_all(pool)
        .await
}

pub async fn tag_cloud(pool: &PgPool, nodes: &[i64], limit: i64) -> Result<Vec<(String, String, i64)>, Error> {
    sqlx::query_as(
        "SELECT t.name, t.slug, count(*) FROM tags t JOIN item_tags it ON it.tag_id = t.id JOIN items i ON i.id = it.item_id
         WHERE i.state = 'visible' AND i.listed AND i.node_id = ANY($1) GROUP BY t.id ORDER BY count(*) DESC, t.name LIMIT $2",
    )
    .bind(nodes)
    .bind(limit)
    .fetch_all(pool)
    .await
}

pub async fn latest_titles(pool: &PgPool, nodes: &[i64], limit: i64) -> Result<Vec<(i64, String, String)>, Error> {
    sqlx::query_as(
        "SELECT id, title, slug FROM items WHERE state = 'visible' AND listed AND node_id = ANY($1)
         ORDER BY last_posted_at DESC, id DESC LIMIT $2",
    )
    .bind(nodes)
    .bind(limit)
    .fetch_all(pool)
    .await
}

crate::sql::set_col!(set_appearance, "users", "color_scheme", i64, Option<&str>);
