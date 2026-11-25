//! Overrides of interface text.

use sqlx::PgPool;

type Error = sqlx::Error;

pub async fn text_overrides(pool: &PgPool) -> Result<Vec<(String, String, String)>, Error> {
    sqlx::query_as("SELECT locale, key, value FROM translation_overrides ORDER BY locale, key").fetch_all(pool).await
}

/// Texts kept in the database that readers see and an admin may translate: every badge's name,
/// description and category.
pub async fn site_texts(pool: &PgPool) -> Result<Vec<String>, Error> {
    let badges: Vec<(String, String, String)> =
        sqlx::query_as("SELECT name, description, category FROM badges").fetch_all(pool).await?;
    // Forums and categories: their name and the Markdown of their description.
    let nodes: Vec<(String, String)> =
        sqlx::query_as("SELECT title, coalesce(description_md, '') FROM nodes WHERE kind IN ('category', 'forum')")
            .fetch_all(pool)
            .await?;
    // What an administrator wrote for readers elsewhere: announcements, reactions, report reasons,
    // widget titles.
    let announcements: Vec<(String, String)> =
        sqlx::query_as("SELECT title, body FROM announcements WHERE enabled").fetch_all(pool).await?;
    let singles: Vec<(String,)> = sqlx::query_as(
        "SELECT name FROM reactions WHERE enabled
         UNION ALL SELECT label FROM report_reasons WHERE enabled
         UNION ALL SELECT title FROM widgets WHERE title <> ''",
    )
    .fetch_all(pool)
    .await?;
    Ok(badges
        .into_iter()
        .flat_map(|(n, d, c)| [n, d, c])
        .chain(nodes.into_iter().flat_map(|(t, d)| [t, d]))
        .chain(announcements.into_iter().flat_map(|(t, b)| [t, b]))
        .chain(singles.into_iter().map(|(t,)| t))
        .collect())
}

/// The header link list as stored (JSON): their labels are texts to translate.
pub async fn navigation_sources(pool: &PgPool) -> Result<Vec<String>, Error> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT value #>> '{}' FROM site_settings WHERE key = 'nav_links' AND jsonb_typeof(value) = 'string'",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(v,)| v).collect())
}

/// Adds an override unless the language already has one for this key; true when it was added.
pub async fn add_text_override(pool: &PgPool, locale: &str, key: &str, value: &str) -> Result<bool, Error> {
    Ok(sqlx::query("INSERT INTO translation_overrides (locale, key, value) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING")
        .bind(locale)
        .bind(key)
        .bind(value)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

pub async fn set_text_override(pool: &PgPool, locale: &str, key: &str, value: &str) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO translation_overrides (locale, key, value) VALUES ($1, $2, $3)
         ON CONFLICT (locale, key) DO UPDATE SET value = EXCLUDED.value",
    )
    .bind(locale)
    .bind(key)
    .bind(value)
    .execute(pool)
    .await
    .map(|_| ())
}

pub async fn delete_text_override(pool: &PgPool, locale: &str, key: &str) -> Result<bool, Error> {
    Ok(sqlx::query("DELETE FROM translation_overrides WHERE locale = $1 AND key = $2")
        .bind(locale)
        .bind(key)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}
