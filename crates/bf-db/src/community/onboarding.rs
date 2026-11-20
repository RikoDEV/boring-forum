//! The getting-started checklist.

use sqlx::PgPool;

type Error = sqlx::Error;

/// Onboarding progress: (has avatar, has about text, has posted, email verified).
pub async fn onboarding(pool: &PgPool, user: i64) -> Result<(bool, bool, bool, bool), Error> {
    sqlx::query_as(
        "SELECT avatar_upload_id IS NOT NULL, about_md IS NOT NULL AND about_md <> '', post_count > 0, email_verified_at IS NOT NULL FROM users WHERE id = $1",
    )
    .bind(user).fetch_one(pool).await
}

/// Hides the getting-started banner for this member for good.
pub async fn dismiss_welcome(pool: &PgPool, user: i64) -> Result<(), Error> {
    sqlx::query("UPDATE users SET welcome_dismissed_at = now() WHERE id = $1 AND welcome_dismissed_at IS NULL")
        .bind(user)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Whether this member has a visible post in the topic, which opens `[hide]` blocks.
pub async fn has_posted(pool: &PgPool, item: i64, user: i64) -> Result<bool, Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM posts WHERE item_id = $1 AND author_id = $2 AND state = 'visible' AND action IS NULL)",
    )
    .bind(item)
    .bind(user)
    .fetch_one(pool)
    .await
}
