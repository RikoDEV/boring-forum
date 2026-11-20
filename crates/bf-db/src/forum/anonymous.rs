//! Anonymous posting through a shared account.

use sqlx::PgPool;

type Error = sqlx::Error;

/// The shared account anonymous posts appear under.
pub async fn anonymous_account(pool: &PgPool) -> Result<i64, Error> {
    const EMAIL: &str = "anonymous@system.invalid";
    // Reuses the importer's rule: if an older account took the name, this one gets a number.
    match crate::platform::importer::import_user(pool, "Anonymous", EMAIL, None, None).await {
        Ok(id) => Ok(id),
        // Two first anonymous posts at once: the loser finds the winner's account.
        Err(e) => {
            let id: Option<i64> = sqlx::query_scalar("SELECT id FROM users WHERE email_lower = $1")
                .bind(EMAIL)
                .fetch_optional(pool)
                .await?;
            id.ok_or(e)
        }
    }
}
