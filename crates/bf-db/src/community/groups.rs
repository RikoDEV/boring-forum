//! Groups: membership, club join requests and group pages.

use sqlx::{PgConnection, PgPool};

type Error = sqlx::Error;

/// Who is putting members in a group. The database refuses `Automatic` for a group that carries
/// staff rights (the `user_groups_guard_staff` trigger): promotion rules, rewards, club joins,
/// plugins and bulk tools can fill any ordinary group but never make a moderator.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Grant {
    Automatic,
    /// An administrator editing a member by hand.
    Staff,
}

/// The one place members are put in a group. Runs in the caller's transaction and returns who
/// was newly added (someone already in the group is left alone).
pub async fn add_members(tx: &mut PgConnection, users: &[i64], group: i64, grant: Grant) -> Result<Vec<i64>, Error> {
    if grant == Grant::Staff {
        sqlx::query("SELECT set_config('bf.grant_staff', 'on', true)").execute(&mut *tx).await?;
    }
    let added = sqlx::query_scalar(
        "INSERT INTO user_secondary_groups (user_id, group_id) SELECT u, $2 FROM unnest($1::bigint[]) u
         ON CONFLICT DO NOTHING RETURNING user_id",
    )
    .bind(users)
    .bind(group)
    .fetch_all(&mut *tx)
    .await;
    // Only for the insert above, not for whatever the transaction does next. (After a failed insert
    // the transaction is aborted anyway, and the real error is the one to return.)
    if grant == Grant::Staff && added.is_ok() {
        sqlx::query("SELECT set_config('bf.grant_staff', 'off', true)").execute(&mut *tx).await?;
    }
    added
}

/// Members behind `@name`: a group's members, or all staff for `@staff`.
pub async fn group_members(pool: &PgPool, slug: &str, limit: i64) -> Result<Vec<i64>, Error> {
    sqlx::query_scalar(
        "SELECT DISTINCT u.id FROM users u LEFT JOIN user_secondary_groups s ON s.user_id = u.id
         LEFT JOIN groups g ON g.id = u.primary_group_id OR g.id = s.group_id
         WHERE ($1 = 'staff' AND u.primary_group_id IN (3, 4)) OR ($1 <> 'staff' AND g.slug = $1)
         ORDER BY u.id LIMIT $2",
    )
    .bind(slug)
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// Answers a request to join a club's group (clubs.rs); approving adds the member.
pub async fn decide_group_request(pool: &PgPool, group: i64, user: i64, approve: bool) -> Result<bool, Error> {
    let mut tx = pool.begin().await?;
    let found = sqlx::query("DELETE FROM group_requests WHERE group_id = $1 AND user_id = $2")
        .bind(group)
        .bind(user)
        .execute(&mut *tx)
        .await?
        .rows_affected()
        > 0;
    if found && approve {
        add_members(&mut tx, &[user], group, Grant::Automatic).await?;
    }
    tx.commit().await?;
    Ok(found)
}

/// Members matching a filter, then one change applied to them all. Staff are never touched.
pub async fn bulk_add_group(pool: &PgPool, min_posts: i32, max_posts: Option<i32>, group: i64) -> Result<u64, Error> {
    let mut tx = pool.begin().await?;
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT u.id FROM users u WHERE u.post_count >= $1 AND ($2::int IS NULL OR u.post_count <= $2)
           AND u.primary_group_id NOT IN (3, 4) AND u.email_lower NOT LIKE '%.invalid'",
    )
    .bind(min_posts)
    .bind(max_posts)
    .fetch_all(&mut *tx)
    .await?;
    let added = add_members(&mut tx, &ids, group, Grant::Automatic).await?;
    tx.commit().await?;
    Ok(added.len() as u64)
}

/// Adds a group to the members a plugin's filter chose. Staff are left alone.
pub async fn add_group_to(pool: &PgPool, users: &[i64], group: i64) -> Result<u64, Error> {
    let mut tx = pool.begin().await?;
    let ids: Vec<i64> =
        sqlx::query_scalar("SELECT u.id FROM users u WHERE u.id = ANY($1) AND u.primary_group_id NOT IN (3, 4)")
            .bind(users)
            .fetch_all(&mut *tx)
            .await?;
    let added = add_members(&mut tx, &ids, group, Grant::Automatic).await?;
    tx.commit().await?;
    Ok(added.len() as u64)
}

/// A group's name and its members who show their profile to everyone.
/// The groups a member belongs to besides their main one, as the profile shows them: (name,
/// colour, slug). Built-in groups and club groups (managed with their club) are left out.
pub async fn secondary_groups(pool: &PgPool, user: i64) -> Result<Vec<(String, Option<String>, String)>, Error> {
    sqlx::query_as(
        "SELECT g.name, g.colour, g.slug FROM user_secondary_groups s JOIN groups g ON g.id = s.group_id
         WHERE s.user_id = $1 AND g.id > 4 AND NOT EXISTS (SELECT 1 FROM clubs c WHERE c.group_id = g.id)
         ORDER BY g.name",
    )
    .bind(user)
    .fetch_all(pool)
    .await
}

/// The group of a closed club is shown to that club's members only (`viewer`), as its forum is.
pub async fn group_page(
    pool: &PgPool,
    slug: &str,
    viewer: Option<i64>,
) -> Result<Option<(String, Vec<String>)>, Error> {
    let Some(name) = sqlx::query_scalar::<_, String>(
        "SELECT g.name FROM groups g WHERE lower(g.slug) = lower($1) AND g.id <> 1
           AND NOT EXISTS (SELECT 1 FROM clubs c WHERE c.group_id = g.id AND NOT c.open
                 AND NOT EXISTS (SELECT 1 FROM user_secondary_groups m
                                 WHERE m.group_id = g.id AND m.user_id = $2::bigint))",
    )
    .bind(slug)
    .bind(viewer)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };
    let members = sqlx::query_scalar(
        "SELECT DISTINCT u.username FROM users u JOIN groups g
           ON g.id = u.primary_group_id OR g.id IN (SELECT group_id FROM user_secondary_groups WHERE user_id = u.id)
         WHERE lower(g.slug) = lower($1) AND u.profile_visibility = 'everyone' ORDER BY u.username LIMIT 200",
    )
    .bind(slug)
    .fetch_all(pool)
    .await?;
    Ok(Some((name, members)))
}

pub async fn group_name(pool: &PgPool, group: i64) -> Result<Option<String>, Error> {
    sqlx::query_scalar("SELECT name FROM groups WHERE id = $1").bind(group).fetch_optional(pool).await
}
