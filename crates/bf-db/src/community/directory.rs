//! Public member directory, groups and badges.

use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool};

type Error = sqlx::Error;

/// Members anyone may look up: real people, with a public profile, not suspended.
const PUBLIC: &str = "u.approved AND u.email_lower NOT LIKE '%.invalid' AND u.profile_visibility = 'everyone' \
                      AND (u.suspended_until IS NULL OR u.suspended_until < now())";

#[derive(Debug, Clone, FromRow)]
pub struct Member {
    pub username: String,
    pub post_count: i32,
    pub reputation: i64,
    pub created_at: DateTime<Utc>,
    pub group_name: String,
    pub trust_level: i16,
}

/// A page of the directory, optionally narrowed by part of a name.
pub async fn members(pool: &PgPool, sort: &str, q: &str, limit: i64, offset: i64) -> Result<Vec<Member>, Error> {
    let order = match sort {
        "reputation" => "u.reputation DESC, u.post_count DESC",
        "joined" => "u.joined_at DESC",
        "name" => "lower(u.username)",
        _ => "u.post_count DESC, u.reputation DESC",
    };
    let needle = format!("%{}%", q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT u.username, u.post_count, u.reputation, u.joined_at AS created_at, g.name AS group_name, u.trust_level
         FROM users u JOIN groups g ON g.id = u.primary_group_id
         WHERE {PUBLIC} AND u.username ILIKE $1
         ORDER BY {order}, u.id LIMIT $2 OFFSET $3"
    )))
    .bind(needle)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
}

/// (name, reputation, posts) of the members with the most reputation.
pub async fn leaderboard(pool: &PgPool, limit: i64) -> Result<Vec<(String, i64, i32)>, Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT u.username, u.reputation, u.post_count FROM users u
         WHERE {PUBLIC} AND u.reputation > 0 ORDER BY u.reputation DESC, u.post_count DESC, u.id LIMIT $1"
    )))
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// (name, slug, members) of the groups worth listing: not guests, and not the private group of a club.
pub async fn groups(pool: &PgPool) -> Result<Vec<(String, String, i64)>, Error> {
    sqlx::query_as(
        "SELECT g.name, g.slug,
                (SELECT count(*) FROM users WHERE primary_group_id = g.id)
                  + (SELECT count(*) FROM user_secondary_groups WHERE group_id = g.id) AS members
         FROM groups g
         WHERE g.id <> 1 AND NOT EXISTS (SELECT 1 FROM clubs c WHERE c.group_id = g.id)
         ORDER BY g.is_admin DESC, g.is_moderator DESC, g.is_staff DESC, g.name",
    )
    .fetch_all(pool)
    .await
}

#[derive(Debug, Clone, FromRow)]
pub struct BadgeRow {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub points: i32,
    pub holders: i64,
    #[sqlx(default)]
    pub tier: String,
    #[sqlx(default)]
    pub icon: String,
    #[sqlx(default)]
    pub category: String,
    #[sqlx(default)]
    pub color: i16,
    #[sqlx(default)]
    pub level: i32,
}

pub async fn badges(pool: &PgPool) -> Result<Vec<BadgeRow>, Error> {
    sqlx::query_as(
        "SELECT b.id, b.name, b.description, b.points,
                (SELECT count(*) FROM user_badges ub WHERE ub.badge_id = b.id) AS holders, b.tier, b.icon, b.category, b.color, b.level
         FROM badges b WHERE b.enabled
         ORDER BY array_position(ARRAY['Getting started', 'Posting', 'Reactions', 'Community', 'Chat', 'Help', 'Trust', 'Time', 'Invitations'], b.category) NULLS LAST, b.points, b.name",
    )
    .fetch_all(pool)
    .await
}

pub async fn badge(pool: &PgPool, id: i64) -> Result<Option<BadgeRow>, Error> {
    sqlx::query_as(
        "SELECT b.id, b.name, b.description, b.points,
                (SELECT count(*) FROM user_badges ub WHERE ub.badge_id = b.id) AS holders, b.tier, b.icon, b.category, b.color, b.level
         FROM badges b WHERE b.id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

/// (name, earned) of a badge's most recent public holders.
pub async fn badge_holders(pool: &PgPool, id: i64, limit: i64) -> Result<Vec<(String, DateTime<Utc>)>, Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT u.username, ub.earned_at FROM user_badges ub JOIN users u ON u.id = ub.user_id
         WHERE ub.badge_id = $1 AND {PUBLIC} ORDER BY ub.earned_at DESC LIMIT $2"
    )))
    .bind(id)
    .bind(limit)
    .fetch_all(pool)
    .await
}

#[derive(Debug, Clone, FromRow)]
pub struct AboutStats {
    pub members: i64,
    pub topics: i64,
    pub posts: i64,
    pub online: i64,
    pub since: Option<DateTime<Utc>>,
}

pub async fn about_stats(pool: &PgPool) -> Result<AboutStats, Error> {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM users WHERE approved AND email_lower NOT LIKE '%.invalid') AS members,
                coalesce((SELECT sum(topic_count) FROM nodes), 0)::bigint AS topics,
                coalesce((SELECT sum(post_count) FROM nodes), 0)::bigint AS posts,
                (SELECT count(*) FROM users WHERE approved AND last_seen_at > now() - interval '15 minutes' AND NOT hide_last_seen AND profile_visibility = 'everyone') AS online,
                (SELECT min(joined_at) FROM users) AS since",
    )
    .fetch_one(pool)
    .await
}

/// The newest topic with this slug in forums the viewer can read.
pub async fn topic_by_slug(pool: &PgPool, slug: &str, readable: &[i64]) -> Result<Option<i64>, Error> {
    sqlx::query_scalar(
        "SELECT id FROM items WHERE slug = $1 AND node_id = ANY($2) AND state = 'visible' ORDER BY id DESC LIMIT 1",
    )
    .bind(slug)
    .bind(readable)
    .fetch_optional(pool)
    .await
}

/// Which of these members were active in the last few minutes, leaving out anyone who hides it.
pub async fn online_among(pool: &PgPool, names: &[String], minutes: i32) -> Result<Vec<String>, Error> {
    sqlx::query_scalar(
        "SELECT username FROM users
         WHERE approved AND username = ANY($1) AND last_seen_at > now() - make_interval(mins => $2) AND NOT hide_last_seen
           AND profile_visibility = 'everyone'",
    )
    .bind(names)
    .bind(minutes)
    .fetch_all(pool)
    .await
}
