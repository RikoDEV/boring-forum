//! One-time registration challenges (ALTCHA), spent once.

use sqlx::PgPool;

type Error = sqlx::Error;

/// Marks a solved challenge as used; false when it already was. Clears out expired ones too.
pub async fn spend_captcha(pool: &PgPool, signature: &str, expires_unix: i64) -> Result<bool, Error> {
    sqlx::query("DELETE FROM captcha_spent WHERE expires_at < now()").execute(pool).await?;
    let done = sqlx::query(
        "INSERT INTO captcha_spent (signature, expires_at) VALUES ($1, to_timestamp($2)) ON CONFLICT DO NOTHING",
    )
    .bind(signature)
    .bind(expires_unix as f64)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}
