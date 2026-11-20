//! API scopes, tags, forums, metadata and files that plugins declare or keep (see `registry.rs`).

use bf_core::perms::Perms;
use sqlx::PgPool;

type Error = sqlx::Error;

/// (plugin, key, label)
pub type ScopeRow = (String, String, String);

/// Replaces the API scopes plugins declare.
pub async fn sync_api_scopes(pool: &PgPool, declared: &[ScopeRow], active_plugins: &[String]) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    for (plugin, key, label) in declared {
        sqlx::query(
            "INSERT INTO plugin_api_scopes (plugin, key, label) VALUES ($1, $2, $3)
             ON CONFLICT (plugin, key) DO UPDATE SET label = EXCLUDED.label",
        )
        .bind(plugin)
        .bind(key)
        .bind(label)
        .execute(&mut *tx)
        .await?;
    }
    let keep: Vec<String> = declared.iter().map(|(p, k, _)| format!("{p}\u{1}{k}")).collect();
    sqlx::query("DELETE FROM plugin_api_scopes WHERE plugin = ANY($1) AND NOT (plugin || chr(1) || key) = ANY($2)")
        .bind(active_plugins)
        .bind(&keep)
        .execute(&mut *tx)
        .await?;
    // Keys keep only scopes that still exist.
    sqlx::query(
        "UPDATE api_keys k SET plugin_scopes = ARRAY(SELECT s FROM unnest(k.plugin_scopes) s
                WHERE s NOT LIKE 'plugin:%' OR s IN (SELECT 'plugin:' || plugin || ':' || key FROM plugin_api_scopes))
         WHERE cardinality(k.plugin_scopes) > 0",
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

/// The scopes a member may put on a key: (full scope `plugin:<id>:<key>`, label, plugin).
pub async fn api_scopes(pool: &PgPool) -> Result<Vec<(String, String, String)>, Error> {
    sqlx::query_as(
        "SELECT 'plugin:' || s.plugin || ':' || s.key, s.label, s.plugin
         FROM plugin_api_scopes s JOIN plugins p ON p.id = s.plugin WHERE p.enabled AND p.approved ORDER BY s.plugin, s.key",
    )
    .fetch_all(pool)
    .await
}

/// Which of these scopes exist (so a key never holds an invented one).
pub async fn existing_scopes(pool: &PgPool, wanted: &[String]) -> Result<Vec<String>, Error> {
    sqlx::query_scalar(
        "SELECT w FROM unnest($1::text[]) w WHERE w IN (SELECT 'plugin:' || plugin || ':' || key FROM plugin_api_scopes)",
    )
    .bind(wanted)
    .fetch_all(pool)
    .await
}

/// The plugin scopes an API key holds.
pub async fn key_scopes(pool: &PgPool, digest: &str) -> Result<Vec<String>, Error> {
    Ok(sqlx::query_scalar("SELECT plugin_scopes FROM api_keys WHERE key_digest = $1")
        .bind(digest)
        .fetch_optional(pool)
        .await?
        .unwrap_or_default())
}

/// A tag a plugin declares: (plugin, key, name).
pub type TagRow = (String, String, String);

pub fn tag_slug(plugin: &str, key: &str) -> String {
    format!("plugin-{plugin}-{key}")
}

/// Seeds the plugins' tags and drops ones no longer declared.
pub async fn sync_tags(pool: &PgPool, declared: &[TagRow], active_plugins: &[String]) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    for (plugin, key, name) in declared {
        sqlx::query(
            "INSERT INTO tags (name, slug, plugin, plugin_key)
             SELECT $1, $2, $3, $4
              WHERE NOT EXISTS (SELECT 1 FROM tags WHERE slug = $2 AND (plugin IS DISTINCT FROM $3 OR plugin_key IS DISTINCT FROM $4))
             ON CONFLICT (plugin, plugin_key) WHERE plugin IS NOT NULL DO UPDATE SET name = EXCLUDED.name",
        )
        .bind(name)
        .bind(tag_slug(plugin, key))
        .bind(plugin)
        .bind(key)
        .execute(&mut *tx)
        .await?;
    }
    let keep: Vec<String> = declared.iter().map(|(p, k, _)| format!("{p}\u{1}{k}")).collect();
    sqlx::query(
        "DELETE FROM tags WHERE plugin IS NOT NULL AND plugin = ANY($1) AND NOT (plugin || chr(1) || plugin_key) = ANY($2)",
    )
    .bind(active_plugins)
    .bind(&keep)
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

/// `tags.set`: makes these (the plugin's own, by key) the plugin's tags on a topic, leaving every
/// other tag on it alone. Returns false when the topic does not exist.
pub async fn set_topic_tags(pool: &PgPool, plugin: &str, topic: i64, keys: &[String]) -> Result<bool, Error> {
    let mut tx = pool.begin().await?;
    let there: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM items WHERE id = $1)").bind(topic).fetch_one(&mut *tx).await?;
    if !there {
        return Ok(false);
    }
    sqlx::query("DELETE FROM item_tags WHERE item_id = $1 AND tag_id IN (SELECT id FROM tags WHERE plugin = $2)")
        .bind(topic)
        .bind(plugin)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO item_tags (item_id, tag_id) SELECT $1, id FROM tags WHERE plugin = $2 AND plugin_key = ANY($3)
         ON CONFLICT DO NOTHING",
    )
    .bind(topic)
    .bind(plugin)
    .bind(keys)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(true)
}

/// Keys of the plugin's tags (for validation).
pub async fn tag_keys(pool: &PgPool, plugin: &str) -> Result<Vec<String>, Error> {
    sqlx::query_scalar("SELECT plugin_key FROM tags WHERE plugin = $1").bind(plugin).fetch_all(pool).await
}

/// A forum a plugin declares.
pub struct NodeRow {
    pub plugin: String,
    pub key: String,
    /// `forum` or `category`.
    pub kind: String,
    pub title: String,
    pub description: String,
    /// Another of this plugin's keys, or an existing forum id as text; empty for the top.
    pub parent: String,
}

/// What members get on a new forum until the administrator changes it: guests read; members post.
fn default_grants() -> Vec<(i64, Perms)> {
    let read = Perms::READER;
    vec![(1, read), (2, read.union(Perms::CREATE).union(Perms::REPLY).union(Perms::ATTACH))]
}

pub fn node_slug(plugin: &str, key: &str) -> String {
    format!("plugin-{plugin}-{key}").replace(':', "-")
}

async fn insert_node(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    n: &NodeRow,
    parent: Option<i64>,
) -> Result<i64, Error> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO nodes (parent_id, kind, title, slug, description_html, position, plugin, plugin_key)
         VALUES ($1, $2, $3, $4, $5, coalesce((SELECT max(position) + 1 FROM nodes WHERE parent_id IS NOT DISTINCT FROM $1), 0), $6, $7)
         RETURNING id",
    )
    .bind(parent)
    .bind(&n.kind)
    .bind(&n.title)
    .bind(node_slug(&n.plugin, &n.key))
    .bind(format!("<p>{}</p>", bf_core::html::esc(&n.description)))
    .bind(&n.plugin)
    .bind(&n.key)
    .fetch_one(&mut **tx)
    .await?;
    // A category must be viewable too, or the forums inside it are hidden.
    {
        let grants = if n.kind == "forum" {
            default_grants()
        } else {
            let read = Perms::READER;
            vec![(1, read), (2, read)]
        };
        for (group, perms) in grants {
            sqlx::query("INSERT INTO node_permissions (node_id, group_id, perms) VALUES ($1, $2, $3)")
                .bind(id)
                .bind(group)
                .bind(perms.0 as i32)
                .execute(&mut **tx)
                .await?;
        }
    }
    Ok(id)
}

/// Creates the plugins' declared forums (parents first), renames the ones that exist, and
/// removes ones no longer declared if they hold no topics. Returns whether the tree changed.
pub async fn sync_nodes(pool: &PgPool, declared: &[NodeRow], active_plugins: &[String]) -> Result<bool, Error> {
    let mut tx = pool.begin().await?;
    let mut changed = false;
    for n in declared {
        let parent: Option<i64> = if n.parent.is_empty() {
            None
        } else if let Ok(id) = n.parent.parse::<i64>() {
            Some(id)
        } else {
            sqlx::query_scalar("SELECT id FROM nodes WHERE plugin = $1 AND plugin_key = $2")
                .bind(&n.plugin)
                .bind(&n.parent)
                .fetch_optional(&mut *tx)
                .await?
        };
        let existing: Option<(i64, String, String)> =
            sqlx::query_as("SELECT id, title, description_html FROM nodes WHERE plugin = $1 AND plugin_key = $2")
                .bind(&n.plugin)
                .bind(&n.key)
                .fetch_optional(&mut *tx)
                .await?;
        match existing {
            None => {
                insert_node(&mut tx, n, parent).await?;
                changed = true;
            }
            Some((id, title, html)) => {
                let want = format!("<p>{}</p>", bf_core::html::esc(&n.description));
                if title != n.title || html != want {
                    sqlx::query("UPDATE nodes SET title = $2, description_html = $3 WHERE id = $1")
                        .bind(id)
                        .bind(&n.title)
                        .bind(want)
                        .execute(&mut *tx)
                        .await?;
                    changed = true;
                }
            }
        }
    }
    let keep: Vec<String> = declared.iter().map(|n| format!("{}\u{1}{}", n.plugin, n.key)).collect();
    let gone = sqlx::query(
        "DELETE FROM nodes WHERE plugin = ANY($1) AND plugin_key NOT LIKE 'dyn:%' AND topic_count = 0
           AND NOT (plugin || chr(1) || plugin_key) = ANY($2)
           AND NOT EXISTS (SELECT 1 FROM nodes c WHERE c.parent_id = nodes.id)",
    )
    .bind(active_plugins)
    .bind(&keep)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(changed || gone.rows_affected() > 0)
}

/// How many forums a plugin may create at run time.
const MAX_DYNAMIC_NODES: i64 = 200;

/// `nodes.create`: a forum under one of the plugin's own declared forums or categories. False
/// when the parent is not one of its own or the plugin has made too many.
pub async fn create_dynamic_node(
    pool: &PgPool,
    plugin: &str,
    parent_key: &str,
    title: &str,
    description: &str,
) -> Result<Option<i64>, Error> {
    let mut tx = pool.begin().await?;
    let parent: Option<i64> = sqlx::query_scalar("SELECT id FROM nodes WHERE plugin = $1 AND plugin_key = $2")
        .bind(plugin)
        .bind(parent_key)
        .fetch_optional(&mut *tx)
        .await?;
    let Some(parent) = parent else { return Ok(None) };
    let made: i64 = sqlx::query_scalar("SELECT count(*) FROM nodes WHERE plugin = $1 AND plugin_key LIKE 'dyn:%'")
        .bind(plugin)
        .fetch_one(&mut *tx)
        .await?;
    if made >= MAX_DYNAMIC_NODES {
        return Ok(None);
    }
    let serial: i64 =
        sqlx::query_scalar("SELECT nextval(pg_get_serial_sequence('nodes', 'id'))").fetch_one(&mut *tx).await?;
    let key = format!("dyn:{serial}");
    let n = NodeRow {
        plugin: plugin.to_owned(),
        key: key.clone(),
        kind: "forum".into(),
        title: title.to_owned(),
        description: description.to_owned(),
        parent: String::new(),
    };
    let id = insert_node(&mut tx, &n, Some(parent)).await?;
    tx.commit().await?;
    Ok(Some(id))
}

/// Before the plugin's row goes: forums that hold topics stay (as the administrators' own),
/// the empty ones are removed, and its stored settings go.
pub async fn purge_plugin(pool: &PgPool, plugin: &str) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE nodes SET plugin = NULL, plugin_key = NULL WHERE plugin = $1 AND (topic_count > 0 OR EXISTS (SELECT 1 FROM items i WHERE i.node_id = nodes.id))")
        .bind(plugin)
        .execute(&mut *tx)
        .await?;
    // Children before parents: a few passes cover any depth a plugin can declare.
    for _ in 0..6 {
        sqlx::query(
            "DELETE FROM nodes WHERE plugin = $1 AND NOT EXISTS (SELECT 1 FROM nodes c WHERE c.parent_id = nodes.id)",
        )
        .bind(plugin)
        .execute(&mut *tx)
        .await?;
    }
    // What is left holds a forum that stayed: it stays too, as the administrators' own.
    sqlx::query("UPDATE nodes SET plugin = NULL, plugin_key = NULL WHERE plugin = $1")
        .bind(plugin)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM site_settings WHERE starts_with(key, $1)")
        .bind(format!("plugin:{plugin}:"))
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

/// Metadata: JSON per (plugin, key, topic or post).
pub async fn meta_get(
    pool: &PgPool,
    plugin: &str,
    target: &str,
    id: i64,
    key: Option<&str>,
) -> Result<serde_json::Value, Error> {
    let rows: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT key, value FROM plugin_meta WHERE plugin = $1 AND target = $2 AND target_id = $3 AND ($4::text IS NULL OR key = $4)",
    )
    .bind(plugin)
    .bind(target)
    .bind(id)
    .bind(key)
    .fetch_all(pool)
    .await?;
    Ok(serde_json::Value::Object(rows.into_iter().collect()))
}

/// Sets (or with `None` clears) one value. False when the topic or post does not exist.
pub async fn meta_set(
    pool: &PgPool,
    plugin: &str,
    target: &str,
    id: i64,
    key: &str,
    value: Option<&serde_json::Value>,
) -> Result<bool, Error> {
    let table = match target {
        "topic" => "items",
        "user" => "users",
        _ => "posts",
    };
    // `table` is one of two constants.
    let there: bool =
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT EXISTS (SELECT 1 FROM {table} WHERE id = $1)")))
            .bind(id)
            .fetch_one(pool)
            .await?;
    if !there {
        return Ok(false);
    }
    match value {
        Some(v) => {
            sqlx::query(
                "INSERT INTO plugin_meta (plugin, key, target, target_id, value) VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (plugin, key, target, target_id) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
            )
            .bind(plugin)
            .bind(key)
            .bind(target)
            .bind(id)
            .bind(v)
            .execute(pool)
            .await?;
        }
        None => {
            sqlx::query("DELETE FROM plugin_meta WHERE plugin = $1 AND key = $2 AND target = $3 AND target_id = $4")
                .bind(plugin)
                .bind(key)
                .bind(target)
                .bind(id)
                .execute(pool)
                .await?;
        }
    }
    Ok(true)
}

/// The public metadata of a topic or post, `{plugin: {key: value}}`: only keys the plugin
/// declared `public`, and only plugins that are on.
pub async fn public_meta(pool: &PgPool, target: &str, id: i64) -> Result<serde_json::Value, Error> {
    let rows: Vec<(String, String, serde_json::Value)> = sqlx::query_as(
        "SELECT m.plugin, m.key, m.value FROM plugin_meta m JOIN plugins p ON p.id = m.plugin
         WHERE m.target = $1 AND m.target_id = $2 AND p.enabled AND p.approved
           AND coalesce((SELECT bool_or(d->>'public' IS DISTINCT FROM 'false')
                         FROM jsonb_array_elements(coalesce(p.schema->'x-reg'->'meta', '[]'::jsonb)) d
                         WHERE d->>'key' = m.key AND d->>'target' = m.target), false)
         ORDER BY m.plugin, m.key",
    )
    .bind(target)
    .bind(id)
    .fetch_all(pool)
    .await?;
    let mut out = serde_json::Map::new();
    for (plugin, key, value) in rows {
        out.entry(plugin).or_insert_with(|| serde_json::json!({}))[key] = value;
    }
    Ok(serde_json::Value::Object(out))
}

/// `public_meta` for a page of rows at once: `{id: {plugin: {key: value}}}`, one query.
pub async fn public_meta_many(
    pool: &PgPool,
    target: &str,
    ids: &[i64],
) -> Result<std::collections::HashMap<i64, serde_json::Value>, Error> {
    let rows: Vec<(i64, String, String, serde_json::Value)> = sqlx::query_as(
        "SELECT m.target_id, m.plugin, m.key, m.value FROM plugin_meta m JOIN plugins p ON p.id = m.plugin
         WHERE m.target = $1 AND m.target_id = ANY($2) AND p.enabled AND p.approved
           AND coalesce((SELECT bool_or(d->>'public' IS DISTINCT FROM 'false')
                         FROM jsonb_array_elements(coalesce(p.schema->'x-reg'->'meta', '[]'::jsonb)) d
                         WHERE d->>'key' = m.key AND d->>'target' = m.target), false)
         ORDER BY m.target_id, m.plugin, m.key",
    )
    .bind(target)
    .bind(ids)
    .fetch_all(pool)
    .await?;
    let mut out: std::collections::HashMap<i64, serde_json::Value> = std::collections::HashMap::new();
    for (id, plugin, key, value) in rows {
        let entry = out.entry(id).or_insert_with(|| serde_json::json!({}));
        entry[plugin.as_str()][key.as_str()] = value;
    }
    Ok(out)
}

/// Files: a plugin's own folder of small files, served publicly from `/plugin-files/<id>/<path>`.
pub async fn file_put(pool: &PgPool, plugin: &str, path: &str, content_type: &str, data: &[u8]) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO plugin_files (plugin, path, content_type, data, size) VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (plugin, path) DO UPDATE SET content_type = EXCLUDED.content_type, data = EXCLUDED.data,
           size = EXCLUDED.size, created_at = now()",
    )
    .bind(plugin)
    .bind(path)
    .bind(content_type)
    .bind(data)
    .bind(i32::try_from(data.len()).unwrap_or(i32::MAX))
    .execute(pool)
    .await
    .map(|_| ())
}

pub async fn file_get(pool: &PgPool, plugin: &str, path: &str) -> Result<Option<(String, Vec<u8>)>, Error> {
    sqlx::query_as("SELECT content_type, data FROM plugin_files WHERE plugin = $1 AND path = $2")
        .bind(plugin)
        .bind(path)
        .fetch_optional(pool)
        .await
}

pub async fn file_delete(pool: &PgPool, plugin: &str, path: &str) -> Result<bool, Error> {
    Ok(sqlx::query("DELETE FROM plugin_files WHERE plugin = $1 AND path = $2")
        .bind(plugin)
        .bind(path)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

/// How many files a plugin keeps.
pub async fn file_count(pool: &PgPool, plugin: &str) -> Result<i64, Error> {
    sqlx::query_scalar("SELECT count(*) FROM plugin_files WHERE plugin = $1").bind(plugin).fetch_one(pool).await
}

/// Edits a post the plugin's own account wrote: (topic id, post number) of the post.
pub async fn own_post(pool: &PgPool, bot: i64, topic: i64, number: i32) -> Result<Option<i64>, Error> {
    sqlx::query_scalar(
        "SELECT id FROM posts WHERE item_id = $1 AND post_number = $2 AND author_id = $3 AND deleted_at IS NULL",
    )
    .bind(topic)
    .bind(number)
    .bind(bot)
    .fetch_optional(pool)
    .await
}
