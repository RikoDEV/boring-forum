//! Clubs: member-owned forums with their own membership.

use crate::platform::importer::CLUB_COLS;
use crate::admin::audit::{Entry, record};
use sqlx::PgPool;

type Error = sqlx::Error;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Club {
    pub id: i64,
    pub name: String,
    pub node_id: i64,
    pub slug: String,
    pub group_id: i64,
    pub owner_id: Option<i64>,
    pub open: bool,
    /// Read-only: nobody joins, asks or is invited.
    pub archived: bool,
    pub members: i64,
    pub joined: bool,
    pub owner: Option<String>,
    /// The club forum's description, which is the club's.
    pub description_html: String,
    pub description_md: String,
    pub topic_count: i32,
    pub post_count: i32,
    pub last_activity_at: Option<chrono::DateTime<chrono::Utc>>,
    /// The viewer asked to join and waits for the owner.
    pub requested: bool,
    /// The viewer has an invitation.
    pub invited: bool,
}

/// An invitation older than this has lapsed; every query that lists or accepts one applies it.
pub const INVITE_LIFETIME_SQL: &str = "interval '30 days'";

/// The club query. `CLUB_COLS` already counts only live invitations toward `invited` (the lifetime
/// is written there as well as in [`INVITE_LIFETIME_SQL`]; a test keeps the two in step).
fn club_cols() -> String {
    CLUB_COLS.to_owned()
}

#[cfg(test)]
mod invite_tests {
    #[test]
    fn the_club_query_and_the_invite_lifetime_agree() {
        assert!(super::CLUB_COLS.contains(super::INVITE_LIFETIME_SQL));
    }
}

pub async fn clubs(pool: &PgPool, viewer: Option<i64>) -> Result<Vec<Club>, Error> {
    // Only the constant club query (`CLUB_COLS`) is interpolated; the viewer id is bound.
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{} ORDER BY c.name LIMIT 200", club_cols())))
        .bind(viewer)
        .fetch_all(pool)
        .await
}

pub async fn club(pool: &PgPool, id: i64, viewer: Option<i64>) -> Result<Option<Club>, Error> {
    // Only the constant club query (`CLUB_COLS`) is interpolated; the ids are bound.
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{} WHERE c.id = $2", club_cols())))
        .bind(viewer)
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn clubs_owned(pool: &PgPool, user: i64) -> Result<i64, Error> {
    sqlx::query_scalar("SELECT count(*) FROM clubs WHERE owner_id = $1").bind(user).fetch_one(pool).await
}

/// Creates the club row. With `cap`, a member who already owns that many clubs gets `None`: the
/// count and the insert run under one per-member lock, so two requests at once cannot both pass.
pub async fn create_club(
    pool: &PgPool,
    name: &str,
    node: i64,
    group: i64,
    owner: i64,
    open: bool,
    cap: Option<i64>,
) -> Result<Option<i64>, Error> {
    let mut tx = pool.begin().await?;
    if let Some(cap) = cap {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext('clubs_per_member:' || $1::text))")
            .bind(owner)
            .execute(&mut *tx)
            .await?;
        let owned: i64 =
            sqlx::query_scalar("SELECT count(*) FROM clubs WHERE owner_id = $1").bind(owner).fetch_one(&mut *tx).await?;
        if owned >= cap {
            return Ok(None);
        }
    }
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO clubs (name, node_id, group_id, owner_id, open) VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(name)
    .bind(node)
    .bind(group)
    .bind(owner)
    .bind(open)
    .fetch_one(&mut *tx)
    .await?;
    crate::community::groups::add_members(&mut tx, &[owner], group, crate::community::groups::Grant::Automatic).await?;
    tx.commit().await?;
    Ok(Some(id))
}

pub async fn club_membership(pool: &PgPool, group: i64, user: i64, join: bool) -> Result<(), Error> {
    if join {
        let mut tx = pool.begin().await?;
        crate::community::groups::add_members(&mut tx, &[user], group, crate::community::groups::Grant::Automatic).await?;
        return tx.commit().await;
    }
    sqlx::query("DELETE FROM user_secondary_groups WHERE group_id = $1 AND user_id = $2")
        .bind(group)
        .bind(user)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn invite_to_club(pool: &PgPool, club: i64, user: i64) -> Result<(), Error> {
    sqlx::query("INSERT INTO club_invites (club_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(club)
        .bind(user)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Clubs a member has an open invitation to.
pub async fn clubs_invited(pool: &PgPool, user: i64) -> Result<Vec<i64>, Error> {
    // Only the constant `INVITE_LIFETIME_SQL` interval is interpolated; the user id is bound.
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT club_id FROM club_invites WHERE user_id = $1 AND created_at > now() - {INVITE_LIFETIME_SQL}"
    )))
    .bind(user)
    .fetch_all(pool)
    .await
}

pub async fn clear_club_invite(pool: &PgPool, club: i64, user: i64) -> Result<(), Error> {
    sqlx::query("DELETE FROM club_invites WHERE club_id = $1 AND user_id = $2")
        .bind(club)
        .bind(user)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Takes back a request to join.
pub async fn withdraw_request(pool: &PgPool, group: i64, user: i64) -> Result<(), Error> {
    sqlx::query("DELETE FROM group_requests WHERE group_id = $1 AND user_id = $2")
        .bind(group)
        .bind(user)
        .execute(pool)
        .await
        .map(|_| ())
}

/// The owner's description of their club, on its forum.
pub async fn set_description(pool: &PgPool, node: i64, md: &str, html: &str) -> Result<(), Error> {
    sqlx::query("UPDATE nodes SET description_md = $2, description_html = $3 WHERE id = $1")
        .bind(node)
        .bind(md)
        .bind(html)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn request_club(pool: &PgPool, group: i64, user: i64) -> Result<bool, Error> {
    Ok(sqlx::query("INSERT INTO group_requests (group_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(group)
        .bind(user)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

/// People waiting to join clubs this member owns: (club, user, username).
pub async fn club_requests(pool: &PgPool, owner: i64) -> Result<Vec<(i64, i64, String)>, Error> {
    sqlx::query_as(
        "SELECT c.id, r.user_id, u.username FROM clubs c JOIN group_requests r ON r.group_id = c.group_id
         JOIN users u ON u.id = r.user_id WHERE c.owner_id = $1 ORDER BY r.created_at LIMIT 200",
    )
    .bind(owner)
    .fetch_all(pool)
    .await
}

/// Members of a club, owner first, then by name: (id, username).
pub async fn club_members(pool: &PgPool, group: i64, owner: Option<i64>, limit: i64) -> Result<Vec<(i64, String)>, Error> {
    sqlx::query_as(
        "SELECT u.id, u.username FROM user_secondary_groups m JOIN users u ON u.id = m.user_id
         WHERE m.group_id = $1 ORDER BY (u.id = $2) DESC, u.username_lower LIMIT $3",
    )
    .bind(group)
    .bind(owner)
    .bind(limit)
    .fetch_all(pool)
    .await
}

pub async fn is_banned(pool: &PgPool, club: i64, user: i64) -> Result<bool, Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM club_bans WHERE club_id = $1 AND user_id = $2)")
        .bind(club)
        .bind(user)
        .fetch_one(pool)
        .await
}

/// Takes a member out of the club (and clears their invitation and request); `ban` also keeps
/// them from coming back.
pub async fn remove_member(pool: &PgPool, club: i64, group: i64, user: i64, ban: bool) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM user_secondary_groups WHERE group_id = $1 AND user_id = $2")
        .bind(group)
        .bind(user)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM club_invites WHERE club_id = $1 AND user_id = $2")
        .bind(club)
        .bind(user)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM group_requests WHERE group_id = $1 AND user_id = $2")
        .bind(group)
        .bind(user)
        .execute(&mut *tx)
        .await?;
    if ban {
        sqlx::query("INSERT INTO club_bans (club_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(club)
            .bind(user)
            .execute(&mut *tx)
            .await?;
    } else {
        sqlx::query("DELETE FROM club_bans WHERE club_id = $1 AND user_id = $2")
            .bind(club)
            .bind(user)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await
}

/// Hands the club to another of its members, who then runs its forum. False when that person
/// is not a member.
pub async fn transfer_club(pool: &PgPool, club: i64, node: i64, group: i64, to: i64) -> Result<bool, Error> {
    let mut tx = pool.begin().await?;
    let member: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM user_secondary_groups WHERE group_id = $1 AND user_id = $2)",
    )
    .bind(group)
    .bind(to)
    .fetch_one(&mut *tx)
    .await?;
    if !member {
        return Ok(false);
    }
    sqlx::query("UPDATE clubs SET owner_id = $2 WHERE id = $1").bind(club).bind(to).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM node_moderators WHERE node_id = $1").bind(node).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO node_moderators (node_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(node)
        .bind(to)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

crate::sql::set_col!(
/// An archived club keeps its forum and members but takes no new ones.
    set_archived, "clubs", "archived", i64, bool);

/// Deletes a club with its forum and group. Refused (false) while its forum holds topics: those
/// are never lost, so the club is archived instead.
pub async fn delete_club(pool: &PgPool, club: i64, actor: i64) -> Result<bool, Error> {
    let mut tx = pool.begin().await?;
    let row: Option<(i64, i64, String)> =
        sqlx::query_as("SELECT node_id, group_id, name FROM clubs WHERE id = $1 FOR UPDATE").bind(club).fetch_optional(&mut *tx).await?;
    let Some((node, group, name)) = row else { return Ok(false) };
    let has_topics: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM items WHERE node_id = $1)")
        .bind(node)
        .fetch_one(&mut *tx)
        .await?;
    if has_topics {
        return Ok(false);
    }
    sqlx::query("DELETE FROM nodes WHERE id = $1").bind(node).execute(&mut *tx).await?;
    sqlx::query("UPDATE users SET primary_group_id = 2 WHERE primary_group_id = $1").bind(group).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM groups WHERE id = $1").bind(group).execute(&mut *tx).await?;
    record(&mut *tx, Entry::new("club.delete").by(actor).on(format!("club:{club}")).details(serde_json::json!({ "name": name }))).await?;
    tx.commit().await?;
    Ok(true)
}
