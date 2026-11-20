//! The source of a post and the author it answers, for replies and quotes.

pub use sqlx::Error;
use sqlx::PgPool;

/// A post's source and the author of the post it answers, if it is someone else's.
pub async fn post_context(pool: &PgPool, item: i64, number: i32) -> Result<(String, Option<i64>), Error> {
    sqlx::query_as(
        "SELECT p.raw, r.author_id FROM posts p LEFT JOIN posts r ON r.id = p.reply_to_post_id
         WHERE p.item_id = $1 AND p.post_number = $2",
    )
    .bind(item)
    .bind(number)
    .fetch_one(pool)
    .await
}
