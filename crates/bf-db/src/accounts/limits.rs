//! Per-group posting and upload limits.

use bf_core::ids::{GroupId, UserId};
pub use sqlx::Error;
use crate::admin::audit::{Entry, record};
use sqlx::PgPool;

/// Group limits: the most generous value among the member's groups wins, and any group that sets
/// no value for a key means none: the result is `None` and the caller falls back to the site
/// setting (a group's limit applies only when every group of the member sets one).
///
/// What 0 means depends on the key:
/// - `edit_minutes`: 0 is unlimited (no time limit), so it beats any number in another group and
///   comes back as 0, which the edit check reads as "no limit".
/// - `max_upload_kb`, `posts_per_day`, `max_images_per_post`: 0 is zero (nothing allowed), as in
///   the site settings these override.
/// - `pm_per_hour`, `pm_max_participants`: a number; the caller applies it as is.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_upload_kb: Option<i64>,
    pub posts_per_day: Option<i64>,
    /// How long authors can edit, in minutes; overrides the site setting.
    pub edit_minutes: Option<i64>,
    pub max_images_per_post: Option<i64>,
    pub pm_per_hour: Option<i64>,
    pub pm_max_participants: Option<i64>,
}

pub async fn limits(pool: &PgPool, groups: &[GroupId]) -> Result<Limits, Error> {
    let rows: Vec<serde_json::Value> =
        sqlx::query_scalar("SELECT limits FROM groups WHERE id = ANY($1)").bind(groups).fetch_all(pool).await?;
    let pick = |k: &str, zero_is_unlimited: bool| -> Option<i64> {
        let mut best: Option<i64> = None;
        for r in &rows {
            // A group without a value for this key: no group limit applies.
            let v = r.get(k).and_then(serde_json::Value::as_i64)?;
            // Unlimited is the largest value while the groups are compared, and 0 again at the end.
            let v = if zero_is_unlimited && v == 0 { i64::MAX } else { v };
            best = Some(best.map_or(v, |b| b.max(v)));
        }
        best.map(|b| if b == i64::MAX { 0 } else { b })
    };
    Ok(Limits {
        max_upload_kb: pick("max_upload_kb", false),
        posts_per_day: pick("posts_per_day", false),
        edit_minutes: pick("edit_minutes", true),
        max_images_per_post: pick("max_images_per_post", false),
        pm_per_hour: pick("pm_per_hour", false),
        pm_max_participants: pick("pm_max_participants", false),
    })
}

pub async fn set_group_limits(
    pool: &PgPool,
    group: i64,
    limits: &serde_json::Value,
    actor: i64,
) -> Result<bool, Error> {
    let mut tx = pool.begin().await?;
    let n = sqlx::query("UPDATE groups SET limits = $2 WHERE id = $1")
        .bind(group)
        .bind(limits)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    record(&mut *tx, Entry::new("group.limits").by(actor).on(format!("group:{group}")).details(limits.clone())).await?;
    tx.commit().await?;
    Ok(n > 0)
}

pub async fn posts_today(pool: &PgPool, user_id: UserId) -> Result<i64, Error> {
    sqlx::query_scalar(
        "SELECT count(*) FROM posts WHERE (author_id = $1 OR real_author_id = $1) AND action IS NULL AND created_at > now() - interval '1 day'",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
}
