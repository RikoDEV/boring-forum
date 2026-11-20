//! Promotion rules, warnings, announcements and member directories.

use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};

type Error = sqlx::Error;

/// Plugin accounts are not people, so rules and directories skip them.
macro_rules! real {
    () => {
        "u.email_lower NOT LIKE '%@plugins.invalid' AND NOT u.remote"
    };
}
pub(crate) const REAL: &str = real!();

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Rule {
    #[serde(default)]
    pub id: i64,
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub min_posts: i32,
    #[serde(default)]
    pub min_days: i32,
    #[serde(default)]
    pub min_reputation: i64,
    #[serde(default)]
    pub min_topics_read: i32,
    pub set_trust_level: Option<i16>,
    pub add_group: Option<i64>,
}

pub(crate) fn yes() -> bool {
    true
}

pub async fn rules(pool: &PgPool) -> Result<Vec<Rule>, Error> {
    sqlx::query_as("SELECT id, name, enabled, min_posts, min_days, min_reputation, min_topics_read, set_trust_level, add_group FROM promotion_rules ORDER BY id")
        .fetch_all(pool)
        .await
}

pub async fn save_rule(pool: &PgPool, id: i64, r: &Rule) -> Result<Option<i64>, Error> {
    let sql = if id == 0 {
        "INSERT INTO promotion_rules (name, enabled, min_posts, min_days, min_reputation, min_topics_read, set_trust_level, add_group)
         SELECT $2, $3, $4, $5, $6, $7, $8, $9 WHERE $1 = 0 RETURNING id"
    } else {
        "UPDATE promotion_rules SET name = $2, enabled = $3, min_posts = $4, min_days = $5, min_reputation = $6,
           min_topics_read = $7, set_trust_level = $8, add_group = $9 WHERE id = $1 RETURNING id"
    };
    sqlx::query_scalar(sql)
        .bind(id)
        .bind(&r.name)
        .bind(r.enabled)
        .bind(r.min_posts)
        .bind(r.min_days)
        .bind(r.min_reputation)
        .bind(r.min_topics_read)
        .bind(r.set_trust_level)
        .bind(r.add_group)
        .fetch_optional(pool)
        .await
}

crate::sql::delete_by_id!(delete_rule, "promotion_rules");

/// Applies enabled rules. Rules only ever raise a trust level or add a group, so running them
/// again changes nothing, and they leave staff rights alone.
/// Adds each enabled rule's group to the members who meet its conditions. Returns (member, group)
/// for each new membership. Trust levels are not set here: their requirements are the
/// `trust_level_N_*` settings.
pub async fn run_rules(pool: &PgPool) -> Result<Vec<(i64, i64)>, Error> {
    let mut joined = Vec::new();
    for r in rules(pool).await?.into_iter().filter(|r| r.enabled) {
        let Some(group) = r.add_group else { continue };
        let ids: Vec<i64> = sqlx::query_scalar(concat!("INSERT INTO user_secondary_groups (user_id, group_id)
             SELECT m.id, $5 FROM (
               SELECT u.id FROM users u WHERE ", real!(), " AND u.approved
                 AND (u.suspended_until IS NULL OR u.suspended_until < now())
                 AND (u.silenced_until IS NULL OR u.silenced_until < now()) AND u.post_count >= $1
                 AND u.joined_at <= now() - make_interval(days => $2) AND u.reputation >= $3
                 AND ($4 = 0 OR (SELECT count(*) FROM read_markers rm WHERE rm.user_id = u.id AND NOT rm.bulk) + u.read_credit >= $4)
             ) m ON CONFLICT DO NOTHING RETURNING user_id"))
        .bind(r.min_posts)
        .bind(r.min_days)
        .bind(r.min_reputation)
        .bind(i64::from(r.min_topics_read))
        .bind(group)
        .fetch_all(pool)
        .await?;
        joined.extend(ids.into_iter().map(|id| (id, group)));
    }
    Ok(joined)
}

pub struct Thresholds {
    pub silence_points: i64,
    pub suspend_points: i64,
    pub days: i32,
}
