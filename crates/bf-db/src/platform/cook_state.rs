//! Whether the cooker's plugin extensions changed since posts were last cooked.

use sqlx::PgPool;

/// Stores `digest` and says whether it differs from the one stored before.
pub async fn swap_digest(pool: &PgPool, digest: &str) -> Result<bool, sqlx::Error> {
    let changed: Option<(i32,)> =
        sqlx::query_as("UPDATE plugin_cook_state SET digest = $1 WHERE id = 1 AND digest <> $1 RETURNING id")
            .bind(digest)
            .fetch_optional(pool)
            .await?;
    Ok(changed.is_some())
}
