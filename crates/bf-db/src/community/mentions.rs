//! Group mentions.

pub use sqlx::Error;
use sqlx::PgPool;

/// Group slugs beginning with `prefix`, if `actor_groups` may mention groups (staff only).
pub async fn mentionable_groups(
    pool: &PgPool,
    actor_groups: &[i64],
    prefix: &str,
    limit: i64,
) -> Result<Vec<String>, Error> {
    let escaped = prefix.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    sqlx::query_scalar(
        "SELECT slug FROM groups WHERE id <> 1 AND slug ILIKE $2 || '%'
           AND EXISTS (SELECT 1 FROM groups a WHERE a.id = ANY($1) AND (a.is_admin OR a.is_moderator OR a.is_staff))
         ORDER BY slug LIMIT $3",
    )
    .bind(actor_groups)
    .bind(escaped)
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// Members of the mentioned groups, if `actor_groups` may mention groups.
pub async fn group_mention_recipients(
    pool: &PgPool,
    actor_groups: &[i64],
    slugs: &[String],
    limit: i64,
) -> Result<Vec<i64>, Error> {
    sqlx::query_scalar(
        "SELECT DISTINCT u.id FROM users u JOIN groups g
           ON g.id = u.primary_group_id OR g.id IN (SELECT group_id FROM user_secondary_groups WHERE user_id = u.id)
         WHERE g.id <> 1 AND lower(g.slug) = ANY($2)
           AND EXISTS (SELECT 1 FROM groups a WHERE a.id = ANY($1) AND (a.is_admin OR a.is_moderator OR a.is_staff))
         LIMIT $3",
    )
    .bind(actor_groups)
    .bind(slugs)
    .bind(limit)
    .fetch_all(pool)
    .await
}
