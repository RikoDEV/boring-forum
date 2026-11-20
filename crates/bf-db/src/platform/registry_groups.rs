//! Groups, permissions and badges that plugins declare (see `registry.rs`).

use sqlx::PgPool;

type Error = sqlx::Error;

/// A group a plugin declares: (plugin, key, name, colour).
pub type GroupRow = (String, String, String, Option<String>);

/// Creates or renames the plugins' groups, and drops those a plugin no longer declares. A name
/// already taken by another group gets the plugin's id added; members stay in groups of plugins
/// that are switched off.
pub async fn sync_groups(pool: &PgPool, declared: &[GroupRow], active_plugins: &[String]) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    for (plugin, key, name, colour) in declared {
        let slug = format!("plugin-{plugin}-{key}");
        let taken: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM groups WHERE name = $1 AND (plugin IS DISTINCT FROM $2 OR plugin_key IS DISTINCT FROM $3))",
        )
        .bind(name)
        .bind(plugin)
        .bind(key)
        .fetch_one(&mut *tx)
        .await?;
        let name = if taken { format!("{name} ({plugin})") } else { name.clone() };
        sqlx::query(
            "INSERT INTO groups (name, slug, colour, plugin, plugin_key) VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (plugin, plugin_key) WHERE plugin IS NOT NULL DO UPDATE SET name = EXCLUDED.name, colour = EXCLUDED.colour",
        )
        .bind(&name)
        .bind(&slug)
        .bind(colour)
        .bind(plugin)
        .bind(key)
        .execute(&mut *tx)
        .await?;
    }
    let keep: Vec<String> = declared.iter().map(|(p, k, ..)| format!("{p}\u{1}{k}")).collect();
    sqlx::query(
        "DELETE FROM groups WHERE plugin IS NOT NULL AND plugin = ANY($1) AND NOT (plugin || chr(1) || plugin_key) = ANY($2)",
    )
    .bind(active_plugins)
    .bind(&keep)
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

/// The id of the group a plugin declared under `key`.
pub async fn plugin_group_id(pool: &PgPool, plugin: &str, key: &str) -> Result<Option<i64>, Error> {
    sqlx::query_scalar("SELECT id FROM groups WHERE plugin = $1 AND plugin_key = $2")
        .bind(plugin)
        .bind(key)
        .fetch_optional(pool)
        .await
}

/// Adds (or removes) a member of one of the plugin's own groups.
pub async fn set_plugin_group_member(pool: &PgPool, group: i64, user: i64, member: bool) -> Result<(), Error> {
    if member {
        let mut tx = pool.begin().await?;
        crate::community::groups::add_members(&mut tx, &[user], group, crate::community::groups::Grant::Automatic).await?;
        tx.commit().await?;
    } else {
        sqlx::query("DELETE FROM user_secondary_groups WHERE user_id = $1 AND group_id = $2")
            .bind(user)
            .bind(group)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// Replaces the permissions plugins declare: (plugin, key, label).
pub async fn sync_permissions(
    pool: &PgPool,
    declared: &[(String, String, String)],
    active_plugins: &[String],
) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    for (plugin, key, label) in declared {
        sqlx::query(
            "INSERT INTO plugin_permissions (plugin, key, label) VALUES ($1, $2, $3)
             ON CONFLICT (plugin, key) DO UPDATE SET label = EXCLUDED.label",
        )
        .bind(plugin)
        .bind(key)
        .bind(label)
        .execute(&mut *tx)
        .await?;
    }
    let keep: Vec<String> = declared.iter().map(|(p, k, _)| format!("{p}\u{1}{k}")).collect();
    sqlx::query("DELETE FROM plugin_permissions WHERE plugin = ANY($1) AND NOT (plugin || chr(1) || key) = ANY($2)")
        .bind(active_plugins)
        .bind(&keep)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

/// A plugin's permissions with the groups that hold each: (key, label, group ids).
pub async fn permissions_of(pool: &PgPool, plugin: &str) -> Result<Vec<(String, String, Vec<i64>)>, Error> {
    sqlx::query_as(
        "SELECT p.key, p.label, coalesce(array_agg(g.group_id ORDER BY g.group_id) FILTER (WHERE g.group_id IS NOT NULL), '{}')
         FROM plugin_permissions p LEFT JOIN plugin_permission_grants g ON g.plugin = p.plugin AND g.perm = p.key
         WHERE p.plugin = $1 GROUP BY p.key, p.label ORDER BY p.key",
    )
    .bind(plugin)
    .fetch_all(pool)
    .await
}

/// Sets the groups that hold a plugin permission. False when the plugin declares no such one.
pub async fn grant_permission(pool: &PgPool, plugin: &str, perm: &str, groups: &[i64]) -> Result<bool, Error> {
    let mut tx = pool.begin().await?;
    let there: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM plugin_permissions WHERE plugin = $1 AND key = $2)")
            .bind(plugin)
            .bind(perm)
            .fetch_one(&mut *tx)
            .await?;
    if !there {
        return Ok(false);
    }
    sqlx::query("DELETE FROM plugin_permission_grants WHERE plugin = $1 AND perm = $2")
        .bind(plugin)
        .bind(perm)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO plugin_permission_grants (plugin, perm, group_id)
         SELECT $1, $2, g.id FROM groups g WHERE g.id = ANY($3)",
    )
    .bind(plugin)
    .bind(perm)
    .bind(groups)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(true)
}

/// Whether a member holds the plugin permission: an administrator always does, others through a group.
pub async fn holds_permission(pool: &PgPool, plugin: &str, perm: &str, user: i64) -> Result<bool, Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM plugin_permission_grants g
                        WHERE g.plugin = $1 AND g.perm = $2
                          AND (g.group_id = (SELECT primary_group_id FROM users WHERE id = $3)
                               OR g.group_id IN (SELECT group_id FROM user_secondary_groups WHERE user_id = $3)))
             OR EXISTS (SELECT 1 FROM groups g WHERE g.is_admin
                        AND (g.id = (SELECT primary_group_id FROM users WHERE id = $3)
                             OR g.id IN (SELECT group_id FROM user_secondary_groups WHERE user_id = $3)))",
    )
    .bind(plugin)
    .bind(perm)
    .bind(user)
    .fetch_one(pool)
    .await
}

/// A badge a plugin declares.
pub struct BadgeRow {
    pub plugin: String,
    pub key: String,
    pub name: String,
    pub description: String,
    pub tier: String,
    pub icon: String,
    pub color: i16,
    pub level: i32,
    pub points: i32,
    pub category: String,
}

/// The slug a plugin's badge is kept under. It has no thresholds, so the forum's own rules never
/// award it: only `badge.grant` does.
pub fn badge_slug(plugin: &str, key: &str) -> String {
    format!("plugin:{plugin}:{key}")
}

/// Seeds the plugins' badges, enables them, removes ones no longer declared, and disables those
/// of plugins that are switched off.
pub async fn sync_badges(pool: &PgPool, declared: &[BadgeRow], active_plugins: &[String]) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    for b in declared {
        sqlx::query(
            "INSERT INTO badges (slug, name, description, tier, icon, color, level, points, category, enabled, backfilled)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, true, true)
             ON CONFLICT (slug) DO UPDATE SET name = EXCLUDED.name, description = EXCLUDED.description, tier = EXCLUDED.tier,
               icon = EXCLUDED.icon, color = EXCLUDED.color, level = EXCLUDED.level, points = EXCLUDED.points,
               category = EXCLUDED.category, enabled = true",
        )
        .bind(badge_slug(&b.plugin, &b.key))
        .bind(&b.name)
        .bind(&b.description)
        .bind(&b.tier)
        .bind(&b.icon)
        .bind(b.color)
        .bind(b.level)
        .bind(b.points)
        .bind(&b.category)
        .execute(&mut *tx)
        .await?;
    }
    let keep: Vec<String> = declared.iter().map(|b| badge_slug(&b.plugin, &b.key)).collect();
    sqlx::query(
        "DELETE FROM badges WHERE slug LIKE 'plugin:%' AND split_part(slug, ':', 2) = ANY($1) AND NOT slug = ANY($2)",
    )
    .bind(active_plugins)
    .bind(&keep)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE badges SET enabled = false WHERE slug LIKE 'plugin:%' AND NOT split_part(slug, ':', 2) = ANY($1)")
        .bind(active_plugins)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

/// Gives a member one of the plugin's badges. False when they already had it or it does not exist.
pub async fn grant_badge(pool: &PgPool, plugin: &str, key: &str, user: i64) -> Result<bool, Error> {
    Ok(sqlx::query(
        "INSERT INTO user_badges (user_id, badge_id) SELECT $2, id FROM badges WHERE slug = $1 AND enabled
         ON CONFLICT DO NOTHING",
    )
    .bind(badge_slug(plugin, key))
    .bind(user)
    .execute(pool)
    .await?
    .rows_affected()
        > 0)
}

/// Everything a plugin seeded that has no foreign key to it, removed with the plugin.
pub async fn purge_plugin(pool: &PgPool, plugin: &str) -> Result<(), Error> {
    sqlx::query("DELETE FROM badges WHERE slug LIKE $1").bind(format!("plugin:{plugin}:%")).execute(pool).await?;
    Ok(())
}
