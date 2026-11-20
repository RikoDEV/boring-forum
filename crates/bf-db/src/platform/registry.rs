//! Rows that plugins declare in their manifests (`[[profile_fields]]`, `[[groups]]`, ...). Each
//! kind is seeded from the manifest of every enabled, approved plugin whenever plugins load, owned
//! by that plugin, and goes away with it.

use crate::accounts::fields::Field;
use sqlx::PgPool;

type Error = sqlx::Error;

/// Makes the `profile_fields` table match the plugins' declarations: rows of listed plugins are
/// created or updated (never repositioned), rows a plugin no longer declares are removed, and
/// rows of plugins that are not loaded are hidden (their answers stay).
pub async fn sync_profile_fields(
    pool: &PgPool,
    declared: &[(String, String, Field)],
    active_plugins: &[String],
) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    for (plugin, key, f) in declared {
        sqlx::query(
            "INSERT INTO profile_fields (label, kind, options, required, show_on_register, public, position, icon,
                    url_prefixes, url_shorthand, https_only, plugin, plugin_key, editable, active)
             VALUES ($1, $2, $3, $4, $5, $6, (SELECT coalesce(max(position), -1) + 1 FROM profile_fields), $7, $8, $9, $10, $11, $12, $13, true)
             ON CONFLICT (plugin, plugin_key) WHERE plugin IS NOT NULL DO UPDATE SET
               label = EXCLUDED.label, kind = EXCLUDED.kind, options = EXCLUDED.options, required = EXCLUDED.required,
               show_on_register = EXCLUDED.show_on_register, public = EXCLUDED.public, icon = EXCLUDED.icon,
               url_prefixes = EXCLUDED.url_prefixes, url_shorthand = EXCLUDED.url_shorthand,
               https_only = EXCLUDED.https_only, editable = EXCLUDED.editable, active = true",
        )
        .bind(&f.label)
        .bind(&f.kind)
        .bind(&f.options)
        .bind(f.required)
        .bind(f.show_on_register)
        .bind(f.public)
        .bind(&f.icon)
        .bind(&f.url_prefixes)
        .bind(f.url_shorthand)
        .bind(f.https_only)
        .bind(plugin)
        .bind(key)
        .bind(f.editable)
        .execute(&mut *tx)
        .await?;
    }
    let keep: Vec<String> = declared.iter().map(|(p, k, _)| format!("{p}\u{1}{k}")).collect();
    sqlx::query(
        "DELETE FROM profile_fields WHERE plugin IS NOT NULL AND plugin = ANY($1) AND NOT (plugin || E'\x01' || plugin_key) = ANY($2)",
    )
    .bind(active_plugins)
    .bind(&keep)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE profile_fields SET active = false WHERE plugin IS NOT NULL AND NOT plugin = ANY($1)")
        .bind(active_plugins)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

/// The id of the field a plugin declared under `key`.
pub async fn plugin_field_id(pool: &PgPool, plugin: &str, key: &str) -> Result<Option<i64>, Error> {
    sqlx::query_scalar("SELECT id FROM profile_fields WHERE plugin = $1 AND plugin_key = $2 AND active")
        .bind(plugin)
        .bind(key)
        .fetch_optional(pool)
        .await
}
