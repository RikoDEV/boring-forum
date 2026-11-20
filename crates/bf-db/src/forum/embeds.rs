//! Topics embedded as comment threads on other sites.

use sqlx::PgPool;

type Error = sqlx::Error;

pub async fn embed_item(pool: &PgPool, url: &str) -> Result<Option<i64>, Error> {
    sqlx::query_scalar("SELECT item_id FROM embed_topics WHERE url = $1").bind(url).fetch_optional(pool).await
}

pub async fn add_embed(pool: &PgPool, url: &str, item: i64) -> Result<(), Error> {
    sqlx::query("INSERT INTO embed_topics (url, item_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(url)
        .bind(item)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn embeds_last_hour(pool: &PgPool) -> Result<i64, Error> {
    sqlx::query_scalar("SELECT count(*) FROM embed_topics WHERE created_at > now() - interval '1 hour'")
        .fetch_one(pool)
        .await
}
