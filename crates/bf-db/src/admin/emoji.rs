//! Custom emoji.

pub use sqlx::Error;
use sqlx::PgPool;

pub async fn custom_emoji(pool: &PgPool) -> Result<Vec<(String, String)>, Error> {
    sqlx::query_as("SELECT name, url FROM custom_emoji ORDER BY name").fetch_all(pool).await
}

pub async fn set_custom_emoji(pool: &PgPool, name: &str, url: &str) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO custom_emoji (name, url) VALUES ($1, $2) ON CONFLICT (name) DO UPDATE SET url = EXCLUDED.url",
    )
    .bind(name)
    .bind(url)
    .execute(pool)
    .await
    .map(|_| ())
}

pub async fn delete_custom_emoji(pool: &PgPool, name: &str) -> Result<bool, Error> {
    Ok(sqlx::query("DELETE FROM custom_emoji WHERE name = $1").bind(name).execute(pool).await?.rows_affected() > 0)
}
