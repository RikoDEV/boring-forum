//! Reactions and report reasons that plugins declare (see `registry.rs`).

use sqlx::PgPool;

type Error = sqlx::Error;

pub struct ReactionRow {
    pub plugin: String,
    pub key: String,
    pub name: String,
    pub icon: String,
    pub score: i16,
}

/// Seeds the plugins' reactions. One that is no longer declared, or whose plugin is off, is
/// switched off rather than deleted while members have used it.
pub async fn sync_reactions(pool: &PgPool, declared: &[ReactionRow], active_plugins: &[String]) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    for r in declared {
        sqlx::query(
            "INSERT INTO reactions (name, icon, score, enabled, position, plugin, plugin_key)
             VALUES ($1, $2, $3, true, (SELECT coalesce(max(position), 0) + 1 FROM reactions), $4, $5)
             ON CONFLICT (plugin, plugin_key) WHERE plugin IS NOT NULL DO UPDATE SET
               name = EXCLUDED.name, icon = EXCLUDED.icon, score = EXCLUDED.score, enabled = true",
        )
        .bind(&r.name)
        .bind(&r.icon)
        .bind(r.score)
        .bind(&r.plugin)
        .bind(&r.key)
        .execute(&mut *tx)
        .await?;
    }
    let keep: Vec<String> = declared.iter().map(|r| format!("{}\u{1}{}", r.plugin, r.key)).collect();
    sqlx::query(
        "UPDATE reactions SET enabled = false WHERE plugin IS NOT NULL
           AND NOT (plugin = ANY($1) AND (plugin || chr(1) || plugin_key) = ANY($2))",
    )
    .bind(active_plugins)
    .bind(&keep)
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

/// The id of the reaction a plugin declared, for display and checks.
pub async fn plugin_reaction_id(pool: &PgPool, plugin: &str, key: &str) -> Result<Option<i64>, Error> {
    sqlx::query_scalar("SELECT id FROM reactions WHERE plugin = $1 AND plugin_key = $2")
        .bind(plugin)
        .bind(key)
        .fetch_optional(pool)
        .await
}

pub struct ReasonRow {
    pub plugin: String,
    pub key: String,
    pub label: String,
    pub hint: String,
    pub weight: f32,
    pub needs_note: bool,
    pub illegal: bool,
}

/// The `report_reasons.key` a plugin's reason is stored under: `p_<plugin>_<key>`, within the
/// table's `^[a-z0-9_]{1,40}$`.
pub fn reason_key(plugin: &str, key: &str) -> String {
    let k = format!("p_{}_{}", plugin.replace('-', "_"), key.replace('-', "_"));
    k.chars().take(40).collect()
}

/// Seeds the plugins' report reasons; ones no longer declared (or of plugins that are off) are
/// switched off, so reports already made keep their reason.
pub async fn sync_reasons(pool: &PgPool, declared: &[ReasonRow], active_plugins: &[String]) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    for r in declared {
        sqlx::query(
            "INSERT INTO report_reasons (key, label, hint, weight, needs_note, illegal, enabled, position, plugin, plugin_key)
             VALUES ($1, $2, $3, $4, $5, $6, true, (SELECT coalesce(max(position), 0) + 10 FROM report_reasons), $7, $8)
             ON CONFLICT (key) DO UPDATE SET label = EXCLUDED.label, hint = EXCLUDED.hint, weight = EXCLUDED.weight,
               needs_note = EXCLUDED.needs_note, illegal = EXCLUDED.illegal, enabled = true
             WHERE report_reasons.plugin = EXCLUDED.plugin",
        )
        .bind(reason_key(&r.plugin, &r.key))
        .bind(&r.label)
        .bind(&r.hint)
        .bind(r.weight)
        .bind(r.needs_note)
        .bind(r.illegal)
        .bind(&r.plugin)
        .bind(&r.key)
        .execute(&mut *tx)
        .await?;
    }
    let keep: Vec<String> = declared.iter().map(|r| reason_key(&r.plugin, &r.key)).collect();
    sqlx::query(
        "UPDATE report_reasons SET enabled = false WHERE plugin IS NOT NULL
           AND NOT (plugin = ANY($1) AND key = ANY($2))",
    )
    .bind(active_plugins)
    .bind(&keep)
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

/// Removes what a plugin seeded into the shared lists, before the plugin's own row goes. Likes
/// given with its reactions go with them, and the posts' counters follow.
pub async fn purge_plugin(pool: &PgPool, plugin: &str) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    let posts: Vec<i64> = sqlx::query_scalar(
        "DELETE FROM post_likes WHERE reaction_id IN (SELECT id FROM reactions WHERE plugin = $1) RETURNING post_id",
    )
    .bind(plugin)
    .fetch_all(&mut *tx)
    .await?;
    if !posts.is_empty() {
        sqlx::query("UPDATE posts p SET like_count = (SELECT count(*) FROM post_likes l WHERE l.post_id = p.id) WHERE p.id = ANY($1)")
            .bind(&posts)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("DELETE FROM reactions WHERE plugin = $1").bind(plugin).execute(&mut *tx).await?;
    tx.commit().await
}
