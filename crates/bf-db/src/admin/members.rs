//! Member and group administration.

use chrono::{DateTime, Utc};
use serde::Serialize;
pub use sqlx::Error;
use crate::admin::audit::{Entry, record};
use sqlx::{FromRow, PgPool};

#[derive(Debug, Serialize, FromRow)]
pub struct AdminUser {
    pub id: i64,
    pub username: String,
    pub email: String,
    pub primary_group_id: i64,
    pub secondary_groups: Vec<i64>,
    pub post_count: i32,
    pub joined_at: DateTime<Utc>,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub suspended_until: Option<DateTime<Utc>>,
    pub silenced_until: Option<DateTime<Utc>>,
    pub email_verified: bool,
    pub approved: bool,
    pub trust_level: i16,
    /// Set by hand: automatic promotion and demotion skip this member.
    pub trust_level_locked: bool,
    pub reputation: i64,
    pub title: Option<String>,
    pub topic_count: i64,
    pub warning_points: i64,
    pub totp_enabled: bool,
    /// All members this filter matches, not just this page.
    pub total: i64,
}

/// What the members list is narrowed to and sorted by.
#[derive(Debug, Default)]
pub struct UserFilter<'a> {
    pub q: &'a str,
    pub status: &'a str,
    pub sort: &'a str,
    pub id: Option<i64>,
}

pub async fn users(pool: &PgPool, f: &UserFilter<'_>, limit: i64, offset: i64) -> Result<Vec<AdminUser>, Error> {
    // Fixed strings chosen here, never text from the request.
    let order = match f.sort {
        "seen" => "u.last_seen_at DESC NULLS LAST",
        "posts" => "u.post_count DESC",
        "name" => "u.username_lower",
        "reputation" => "u.reputation DESC",
        _ => "u.id DESC",
    };
    let sql = format!(
        "SELECT u.id, u.username, u.email, u.primary_group_id,
                coalesce((SELECT array_agg(group_id ORDER BY group_id) FROM user_secondary_groups WHERE user_id = u.id), '{{}}') AS secondary_groups,
                u.post_count, u.joined_at, u.last_seen_at, u.suspended_until, u.silenced_until,
                u.email_verified_at IS NOT NULL AS email_verified, u.approved,
                u.trust_level, u.trust_level_locked, u.reputation, u.title, u.totp_secret IS NOT NULL AS totp_enabled,
                (SELECT count(*) FROM items i WHERE i.author_id = u.id AND i.state = 'visible') AS topic_count,
                (SELECT coalesce(sum(w.points), 0)::bigint FROM warnings w WHERE w.user_id = u.id AND w.expires_at > now()) AS warning_points,
                count(*) OVER () AS total
         FROM users u
         WHERE ($1 = '' OR u.username_lower LIKE '%' || lower($1) || '%' OR u.email_lower LIKE '%' || lower($1) || '%')
           AND ($5::bigint IS NULL OR u.id = $5)
           AND CASE $2
                 WHEN 'new' THEN u.joined_at > now() - interval '7 days'
                 WHEN 'staff' THEN u.primary_group_id IN (3, 4)
                 WHEN 'pending' THEN NOT u.approved
                 WHEN 'suspended' THEN u.suspended_until > now()
                 WHEN 'silenced' THEN u.silenced_until > now()
                 WHEN 'unverified' THEN u.email_verified_at IS NULL
                 ELSE true END
         ORDER BY {order} LIMIT $3 OFFSET $4"
    );
    sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(f.q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"))
        .bind(f.status)
        .bind(limit)
        .bind(offset)
        .bind(f.id)
        .fetch_all(pool)
        .await
}

/// Signs a member out everywhere.
pub async fn end_sessions(pool: &PgPool, user: i64, actor: i64) -> Result<(), Error> {
    sqlx::query("DELETE FROM sessions WHERE user_id = $1").bind(user).execute(pool).await?;
    record(pool, Entry::new("user.logout").by(actor).on(format!("user:{user}"))).await
}

#[derive(Debug, Default)]
pub struct UserChange {
    pub primary_group_id: Option<i64>,
    pub secondary_groups: Option<Vec<i64>>,
    /// `Some(None)` clears the field.
    pub suspended_until: Option<Option<DateTime<Utc>>>,
    pub silenced_until: Option<Option<DateTime<Utc>>>,
    pub approved: Option<bool>,
    pub trust_level: Option<i16>,
    /// A level set by hand locks itself; `Some(false)` hands it back to the automatic rules.
    pub trust_level_locked: Option<bool>,
    pub email_verified: Option<bool>,
    /// Staff-set title shown under the name; `Some(None)` clears it.
    pub title: Option<Option<String>>,
}

pub async fn update_user(pool: &PgPool, id: i64, c: &UserChange, actor: i64) -> Result<bool, Error> {
    let mut tx = pool.begin().await?;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id = $1)").bind(id).fetch_one(&mut *tx).await?;
    if !exists {
        return Ok(false);
    }
    // Different rights mean a new token, and these sessions can only get one by signing in.
    let before: Vec<i64> = sqlx::query_scalar(
        "SELECT primary_group_id FROM users WHERE id = $1
         UNION SELECT group_id FROM user_secondary_groups WHERE user_id = $1 ORDER BY 1",
    )
    .bind(id)
    .fetch_all(&mut *tx)
    .await?;
    if let Some(g) = c.primary_group_id {
        sqlx::query("UPDATE users SET primary_group_id = $2 WHERE id = $1").bind(id).bind(g).execute(&mut *tx).await?;
    }
    if let Some(groups) = &c.secondary_groups {
        sqlx::query("DELETE FROM user_secondary_groups WHERE user_id = $1").bind(id).execute(&mut *tx).await?;
        // An administrator editing a member: the one path that may grant a staff group.
        for group in groups {
            crate::community::groups::add_members(&mut tx, &[id], *group, crate::community::groups::Grant::Staff).await?;
        }
    }
    let after: Vec<i64> = sqlx::query_scalar(
        "SELECT primary_group_id FROM users WHERE id = $1
         UNION SELECT group_id FROM user_secondary_groups WHERE user_id = $1 ORDER BY 1",
    )
    .bind(id)
    .fetch_all(&mut *tx)
    .await?;
    if before != after {
        sqlx::query("DELETE FROM sessions WHERE user_id = $1").bind(id).execute(&mut *tx).await?;
    }
    if let Some(t) = c.suspended_until {
        sqlx::query("UPDATE users SET suspended_until = $2 WHERE id = $1").bind(id).bind(t).execute(&mut *tx).await?;
        if t.is_some() {
            // A suspension takes effect immediately.
            sqlx::query("DELETE FROM sessions WHERE user_id = $1").bind(id).execute(&mut *tx).await?;
        }
    }
    if let Some(t) = &c.title {
        sqlx::query("UPDATE users SET title = $2 WHERE id = $1").bind(id).bind(t).execute(&mut *tx).await?;
    }
    if let Some(level) = c.trust_level {
        sqlx::query("UPDATE users SET trust_level = $2, trust_level_locked = true WHERE id = $1")
            .bind(id)
            .bind(level)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(locked) = c.trust_level_locked {
        sqlx::query("UPDATE users SET trust_level_locked = $2 WHERE id = $1")
            .bind(id)
            .bind(locked)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(v) = c.email_verified {
        sqlx::query("UPDATE users SET email_verified_at = CASE WHEN $2 THEN coalesce(email_verified_at, now()) END WHERE id = $1")
            .bind(id)
            .bind(v)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(a) = c.approved {
        sqlx::query("UPDATE users SET approved = $2 WHERE id = $1").bind(id).bind(a).execute(&mut *tx).await?;
    }
    if let Some(t) = c.silenced_until {
        sqlx::query("UPDATE users SET silenced_until = $2 WHERE id = $1").bind(id).bind(t).execute(&mut *tx).await?;
    }
    record(
        &mut *tx,
        Entry::new("user.update").by(actor).on(format!("user:{id}")).details(serde_json::json!({
        "primary_group_id": c.primary_group_id,
        "secondary_groups": c.secondary_groups,
        "suspended_until": c.suspended_until,
        "silenced_until": c.silenced_until,
        "approved": c.approved,
        "trust_level": c.trust_level,
        "trust_level_locked": c.trust_level_locked,
        "email_verified": c.email_verified,
        })),
    ).await?;
    tx.commit().await?;
    Ok(true)
}

#[derive(Debug, Serialize, FromRow)]
pub struct Group {
    pub id: i64,
    pub name: String,
    pub slug: String,
    pub is_staff: bool,
    pub is_moderator: bool,
    pub is_admin: bool,
    pub members: i64,
    pub colour: Option<String>,
    pub icon: Option<String>,
    /// An uploaded badge picture, shown in place of the icon.
    pub image: Option<String>,
    pub require_2fa: bool,
    pub limits: serde_json::Value,
    /// The club this group belongs to, if any: it is managed with the club.
    pub club: Option<String>,
    /// The plugin that declared this group: it is managed there.
    pub plugin: Option<String>,
}

pub async fn groups(pool: &PgPool) -> Result<Vec<Group>, Error> {
    sqlx::query_as(
        "SELECT g.id, g.name, g.slug, g.is_staff, g.is_moderator, g.is_admin,
                (SELECT count(*) FROM users WHERE primary_group_id = g.id)
                  + (SELECT count(*) FROM user_secondary_groups WHERE group_id = g.id) AS members,
                g.colour, g.icon, g.image, g.require_2fa, g.limits,
                (SELECT c.name FROM clubs c WHERE c.group_id = g.id LIMIT 1) AS club, g.plugin
         FROM groups g ORDER BY g.id",
    )
    .fetch_all(pool)
    .await
}

/// How a group is named and shown.
pub struct GroupStyle<'a> {
    pub name: &'a str,
    pub slug: &'a str,
    pub colour: Option<&'a str>,
    pub icon: Option<&'a str>,
    pub image: Option<&'a str>,
}

/// Renames and restyles a group. False when there is no such group.
pub async fn update_group(pool: &PgPool, id: i64, g: &GroupStyle<'_>, actor: i64) -> Result<bool, Error> {
    let mut tx = pool.begin().await?;
    let old: Option<(String, String)> = sqlx::query_as("SELECT name, slug FROM groups WHERE id = $1 AND plugin IS NULL FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    let Some((old_name, old_slug)) = old else { return Ok(false) };
    sqlx::query("UPDATE groups SET name = $2, slug = $3, colour = $4, icon = $5, image = $6 WHERE id = $1")
        .bind(id)
        .bind(g.name)
        .bind(g.slug)
        .bind(g.colour)
        .bind(g.icon)
        .bind(g.image)
        .execute(&mut *tx)
        .await?;
    record(
        &mut *tx,
        Entry::new("group.update").by(actor).on(format!("group:{id}")).details(serde_json::json!({
        "from": { "name": old_name, "slug": old_slug },
        "to": { "name": g.name, "slug": g.slug, "colour": g.colour, "icon": g.icon, "image": g.image },
        })),
    ).await?;
    tx.commit().await?;
    Ok(true)
}

/// Why a group cannot be deleted, if it cannot.
pub async fn group_in_use(pool: &PgPool, id: i64) -> Result<Option<&'static str>, Error> {
    let (club, plugin, poll): (bool, bool, bool) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM clubs WHERE group_id = $1),
                EXISTS (SELECT 1 FROM groups WHERE id = $1 AND plugin IS NOT NULL),
                EXISTS (SELECT 1 FROM poll_groups WHERE group_id = $1)",
    )
    .bind(id)
    .fetch_one(pool)
    .await?;
    Ok(if plugin {
        Some("This group belongs to a plugin; uninstall the plugin to remove it.")
    } else if club {
        Some("This group belongs to a club; delete the club instead.")
    } else if poll {
        // Deleting it would open the poll to everyone, since an empty list means "anyone may vote".
        Some("A poll is limited to this group; change the poll before deleting the group.")
    } else {
        None
    })
}

/// Deletes a custom group. Members whose primary group it was go back to Members; secondary
/// memberships, forum permissions and requests go with it. False when there is no such group.
pub async fn delete_group(pool: &PgPool, id: i64, actor: i64) -> Result<bool, Error> {
    let mut tx = pool.begin().await?;
    let name: Option<String> = sqlx::query_scalar("SELECT name FROM groups WHERE id = $1 AND id > 4 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    let Some(name) = name else { return Ok(false) };
    let moved = sqlx::query("UPDATE users SET primary_group_id = 2 WHERE primary_group_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    sqlx::query("DELETE FROM groups WHERE id = $1").bind(id).execute(&mut *tx).await?;
    record(&mut *tx, Entry::new("group.delete").by(actor).on(format!("group:{id}")).details(serde_json::json!({ "name": name, "primary_members_moved": moved }))).await?;
    tx.commit().await?;
    Ok(true)
}

pub async fn create_group(pool: &PgPool, name: &str, slug: &str, actor: i64) -> Result<i64, Error> {
    let mut tx = pool.begin().await?;
    let id: i64 = sqlx::query_scalar("INSERT INTO groups (name, slug) VALUES ($1, $2) RETURNING id")
        .bind(name)
        .bind(slug)
        .fetch_one(&mut *tx)
        .await?;
    record(&mut *tx, Entry::new("group.create").by(actor).on(format!("group:{id}"))).await?;
    tx.commit().await?;
    Ok(id)
}
