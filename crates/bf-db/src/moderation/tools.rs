//! Staff tools: IP lookups, member history, saved moderator actions, topic flags.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::admin::audit::{Entry, record};
use sqlx::{FromRow, PgPool};

type Error = sqlx::Error;

pub(crate) async fn audit(
    pool: &PgPool,
    actor: i64,
    action: &str,
    target: String,
    details: serde_json::Value,
) -> Result<(), Error> {
    record(pool, Entry::new(action).by(actor).on(target).details(details)).await
}

#[derive(Debug, Serialize, FromRow)]
pub struct IpRow {
    pub ip: String,
    pub posts: i64,
    pub last_used: DateTime<Utc>,
}

/// Addresses a member has posted or signed in from.
pub async fn user_ips(pool: &PgPool, user: i64) -> Result<Vec<IpRow>, Error> {
    sqlx::query_as(
        "SELECT host(ip) AS ip, count(*) AS posts, max(created_at) AS last_used FROM posts
         WHERE author_id = $1 AND ip IS NOT NULL GROUP BY ip ORDER BY last_used DESC LIMIT 100",
    )
    .bind(user)
    .fetch_all(pool)
    .await
}

#[derive(Debug, Serialize, FromRow)]
pub struct IpMember {
    pub id: i64,
    pub username: String,
    pub posts: i64,
    pub last_used: DateTime<Utc>,
}

/// Members who posted from an address or network.
pub async fn members_on_ip(pool: &PgPool, net: &str) -> Result<Vec<IpMember>, Error> {
    sqlx::query_as(
        "SELECT u.id, u.username, count(*) AS posts, max(p.created_at) AS last_used
         FROM posts p JOIN users u ON u.id = p.author_id
         WHERE p.ip <<= $1::inet GROUP BY u.id ORDER BY last_used DESC LIMIT 100",
    )
    .bind(net)
    .fetch_all(pool)
    .await
}

/// Turns off a member's TOTP, backup codes and passkeys, and ends their sessions.
pub async fn reset_two_factor(pool: &PgPool, user: i64, actor: i64) -> Result<bool, Error> {
    let mut tx = pool.begin().await?;
    let n = sqlx::query("UPDATE users SET totp_secret = NULL WHERE id = $1")
        .bind(user)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    sqlx::query("DELETE FROM passkeys WHERE user_id = $1").bind(user).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM backup_codes WHERE user_id = $1").bind(user).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM sessions WHERE user_id = $1").bind(user).execute(&mut *tx).await?;
    tx.commit().await?;
    audit(pool, actor, "user.reset_2fa", format!("user:{user}"), serde_json::json!({})).await?;
    Ok(n > 0)
}

/// Deletes an account. Content stays, shown as "deleted", unless `with_content`, which
/// soft-deletes it first. Refuses the last administrator.
pub async fn delete_user(pool: &PgPool, user: i64, with_content: bool, actor: i64) -> Result<Option<bool>, Error> {
    let admin_left: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM users u JOIN groups g ON g.id = u.primary_group_id OR g.id IN
             (SELECT group_id FROM user_secondary_groups WHERE user_id = u.id)
           WHERE g.is_admin AND u.id <> $1)",
    )
    .bind(user)
    .fetch_one(pool)
    .await?;
    if !admin_left {
        return Ok(None);
    }
    let mut tx = pool.begin().await?;
    if with_content {
        // Topics they took part in need their counters put right afterwards.
        let touched: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT DISTINCT i.id, i.node_id FROM posts p JOIN items i ON i.id = p.item_id WHERE p.author_id = $1",
        )
        .bind(user)
        .fetch_all(&mut *tx)
        .await?;
        sqlx::query("UPDATE posts SET state = 'deleted', deleted_at = now(), deleted_by = $2 WHERE author_id = $1 AND state <> 'deleted'")
            .bind(user)
            .bind(actor)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE items SET state = 'deleted', deleted_at = now(), deleted_by = $2 WHERE author_id = $1 AND state <> 'deleted'")
            .bind(user)
            .bind(actor)
            .execute(&mut *tx)
            .await?;
        for (item, _) in &touched {
            crate::moderation::topics::recount_item(&mut tx, *item).await?;
            crate::moderation::topics::recount_authors_of(&mut tx, *item).await?;
        }
        let mut nodes: Vec<i64> = touched.iter().map(|t| t.1).collect();
        nodes.sort_unstable();
        nodes.dedup();
        for node in nodes {
            crate::moderation::topics::recount_node(&mut tx, node).await?;
        }
    }
    let n = sqlx::query("DELETE FROM users WHERE id = $1").bind(user).execute(&mut *tx).await?.rows_affected();
    tx.commit().await?;
    audit(pool, actor, "user.delete", format!("user:{user}"), serde_json::json!({ "with_content": with_content }))
        .await?;
    Ok(Some(n > 0))
}

#[derive(Debug, Serialize, FromRow)]
pub struct StaffAction {
    pub action: String,
    pub target: Option<String>,
    pub actor: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Moderator overview counts over the forums in `nodes`: (reports and queue, hidden, deleted).
pub async fn modcp_counts(pool: &PgPool, nodes: &[i64]) -> Result<(i64, i64, i64), Error> {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM reviewables WHERE status = 'pending' AND node_id = ANY($1)),
                (SELECT count(*) FROM posts p JOIN items i ON i.id = p.item_id WHERE p.state = 'hidden' AND i.node_id = ANY($1)),
                (SELECT count(*) FROM posts p JOIN items i ON i.id = p.item_id WHERE p.state = 'deleted' AND i.node_id = ANY($1))",
    )
    .bind(nodes)
    .fetch_one(pool)
    .await
}

/// Hidden posts in the given forums: (post id, topic id, slug, title, author, reason, number).
#[allow(clippy::type_complexity)]
pub async fn hidden_posts(
    pool: &PgPool,
    nodes: &[i64],
) -> Result<Vec<(i64, i64, String, String, Option<String>, Option<String>, i32)>, Error> {
    sqlx::query_as(
        "SELECT p.id, i.id, i.slug, i.title, u.username, p.hidden_reason, p.post_number
         FROM posts p JOIN items i ON i.id = p.item_id LEFT JOIN users u ON u.id = p.author_id
         WHERE p.state = 'hidden' AND i.node_id = ANY($1) ORDER BY p.id DESC LIMIT 100",
    )
    .bind(nodes)
    .fetch_all(pool)
    .await
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub kind: String,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModAction {
    #[serde(default)]
    pub id: i64,
    pub name: String,
    pub steps: Vec<Step>,
}

fn action_from(row: (i64, String, serde_json::Value)) -> ModAction {
    ModAction { id: row.0, name: row.1, steps: serde_json::from_value(row.2).unwrap_or_default() }
}

pub async fn mod_actions(pool: &PgPool) -> Result<Vec<ModAction>, Error> {
    let rows: Vec<(i64, String, serde_json::Value)> =
        sqlx::query_as("SELECT id, name, steps FROM mod_actions ORDER BY name").fetch_all(pool).await?;
    Ok(rows.into_iter().map(action_from).collect())
}

pub async fn mod_action(pool: &PgPool, id: i64) -> Result<Option<ModAction>, Error> {
    let row: Option<(i64, String, serde_json::Value)> =
        sqlx::query_as("SELECT id, name, steps FROM mod_actions WHERE id = $1").bind(id).fetch_optional(pool).await?;
    Ok(row.map(action_from))
}

pub async fn save_mod_action(pool: &PgPool, id: i64, a: &ModAction) -> Result<Option<i64>, Error> {
    let steps = serde_json::to_value(&a.steps).unwrap_or_default();
    let sql = if id == 0 {
        "INSERT INTO mod_actions (name, steps) SELECT $2, $3 WHERE $1 = 0 RETURNING id"
    } else {
        "UPDATE mod_actions SET name = $2, steps = $3 WHERE id = $1 RETURNING id"
    };
    sqlx::query_scalar(sql).bind(id).bind(&a.name).bind(steps).fetch_optional(pool).await
}

crate::sql::delete_by_id!(delete_mod_action, "mod_actions");

/// `column` comes only from the fixed names in the handler, never from input.
pub async fn set_topic_flag(pool: &PgPool, item: i64, column: &'static str, on: bool) -> Result<(), Error> {
    debug_assert!(matches!(column, "locked" | "pinned"));
    sqlx::query(sqlx::AssertSqlSafe(format!("UPDATE items SET {column} = $2 WHERE id = $1")))
        .bind(item)
        .bind(on)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn log_action(pool: &PgPool, actor: i64, item: i64, name: &str) -> Result<(), Error> {
    record(pool, Entry::new("topic.action").by(actor).on(item.to_string()).details(serde_json::json!({ "name": name }))).await
}

/// Accounts seen on an address, for staff.
pub async fn accounts_on_ip(pool: &PgPool, ip: &str) -> Result<Vec<(i64, String, i64)>, Error> {
    sqlx::query_as(
        "SELECT u.id, u.username, count(*) AS posts FROM posts p JOIN users u ON u.id = p.author_id
         WHERE p.ip = $1::text::inet GROUP BY u.id, u.username ORDER BY posts DESC LIMIT 200",
    )
    .bind(ip)
    .fetch_all(pool)
    .await
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct HistoryRow {
    pub actor: Option<String>,
    pub action: String,
    pub details: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

pub async fn member_history(pool: &PgPool, user: i64) -> Result<Vec<HistoryRow>, Error> {
    sqlx::query_as(
        "SELECT u.username AS actor, a.action, a.details, a.created_at FROM audit_log a
         LEFT JOIN users u ON u.id = a.actor_id WHERE a.target IN ($1, 'user:' || $1) AND a.action LIKE ANY (ARRAY['user.%', 'account.%'])
         ORDER BY a.id DESC LIMIT 200",
    )
    .bind(user.to_string())
    .fetch_all(pool)
    .await
}

/// A session opened by an admin as another member: short-lived, and audited.
pub async fn mark_impersonation(pool: &PgPool, digest: &str, admin: i64, target: i64) -> Result<(), Error> {
    sqlx::query("UPDATE sessions SET expires_at = now() + interval '1 hour' WHERE token_digest = $1")
        .bind(digest)
        .execute(pool)
        .await?;
    record(pool, Entry::new("user.impersonate").by(admin).on(format!("user:{target}")).details(serde_json::json!({ "impersonated_by": admin }))).await
}

// ---- Moderator panel overview ----

/// When the oldest pending review item was raised, in the given forums.
pub async fn oldest_pending(pool: &PgPool, nodes: &[i64]) -> Result<Option<DateTime<Utc>>, Error> {
    sqlx::query_scalar("SELECT min(created_at) FROM reviewables WHERE status = 'pending' AND node_id = ANY($1)")
        .bind(nodes)
        .fetch_one(pool)
        .await
}

/// Accounts waiting for approval.
pub async fn unapproved_members(pool: &PgPool) -> Result<i64, Error> {
    sqlx::query_scalar("SELECT count(*) FROM users WHERE NOT approved").fetch_one(pool).await
}

#[derive(Debug, Clone, FromRow)]
pub struct MemberRow {
    pub id: i64,
    pub username: String,
    pub is_admin: bool,
    pub is_moderator: bool,
    pub joined_at: DateTime<Utc>,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub post_count: i32,
    pub trust_level: i16,
    pub approved: bool,
    pub suspended: bool,
    pub silenced: bool,
    /// Warning points that have not expired.
    pub warning_points: i64,
}

/// Members, newest first; with `find`, those whose name contains it. Plugin accounts are left out.
pub async fn members(pool: &PgPool, find: &str, limit: i64, offset: i64) -> Result<Vec<MemberRow>, Error> {
    sqlx::query_as(
        "SELECT u.id, u.username, u.joined_at, u.last_seen_at, u.post_count, u.trust_level, u.approved,
                EXISTS (SELECT 1 FROM groups g WHERE g.is_admin AND (g.id = u.primary_group_id
                    OR g.id IN (SELECT group_id FROM user_secondary_groups WHERE user_id = u.id))) AS is_admin,
                EXISTS (SELECT 1 FROM groups g WHERE g.is_moderator AND (g.id = u.primary_group_id
                    OR g.id IN (SELECT group_id FROM user_secondary_groups WHERE user_id = u.id))) AS is_moderator,
                coalesce(u.suspended_until > now(), false) AS suspended,
                coalesce(u.silenced_until > now(), false) AS silenced,
                (SELECT coalesce(sum(w.points), 0)::bigint FROM warnings w WHERE w.user_id = u.id AND w.expires_at > now())
                    AS warning_points
         FROM users u
         WHERE u.email_lower NOT LIKE '%@plugins.invalid' AND ($1 = '' OR strpos(u.username_lower, lower($1)) > 0)
         ORDER BY u.joined_at DESC LIMIT $2 OFFSET $3",
    )
    .bind(find)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
}

#[derive(Debug, Clone, FromRow)]
pub struct WarningRow {
    pub member: String,
    pub moderator: Option<String>,
    pub points: i32,
    pub reason: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

pub async fn recent_warnings(pool: &PgPool, limit: i64) -> Result<Vec<WarningRow>, Error> {
    sqlx::query_as(
        "SELECT u.username AS member, m.username AS moderator, w.points, w.reason, w.created_at, w.expires_at
         FROM warnings w JOIN users u ON u.id = w.user_id LEFT JOIN users m ON m.id = w.moderator_id
         ORDER BY w.created_at DESC LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await
}

#[derive(Debug, Clone, FromRow)]
pub struct ModLogEntry {
    pub action: String,
    pub target: Option<String>,
    pub actor: Option<String>,
    /// The member an action was about, when it was about one.
    pub target_name: Option<String>,
    /// The reason a moderator gave, when they gave one.
    pub note: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// What moderators did, newest first: hiding, review decisions, warnings, topic tools, bans.
pub async fn mod_log(pool: &PgPool, limit: i64, offset: i64) -> Result<Vec<ModLogEntry>, Error> {
    sqlx::query_as(
        "SELECT a.action, a.target, u.username AS actor, tu.username AS target_name, a.details ->> 'reason' AS note, a.created_at FROM audit_log a
         LEFT JOIN users u ON u.id = a.actor_id
         LEFT JOIN users tu ON (a.action LIKE 'user.%' OR a.action LIKE 'mod.%') AND tu.id::text = regexp_replace(coalesce(a.target, ''), '^user:', '')
         WHERE a.action LIKE 'post.%' OR a.action LIKE 'review.%' OR a.action LIKE 'held.%' OR (a.action LIKE 'topic.%' AND a.action <> 'topic.invite')
            OR a.action LIKE 'ban.%' OR a.action LIKE 'member_report.%' OR a.action LIKE 'dsa.%'
            OR a.action LIKE 'mod.%' OR a.action LIKE 'mod_action.%' OR a.action LIKE 'report_reason.%' OR a.action LIKE 'revision.%'
            OR a.action IN ('user.warn', 'user.approve', 'user.reject', 'user.delete_spammer', 'user.reset_2fa', 'user.delete', 'upload.known_image')
         ORDER BY a.id DESC LIMIT $1 OFFSET $2",
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
}
