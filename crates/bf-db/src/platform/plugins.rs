//! Installed plugins and their key-value store.

use serde::Serialize;
use sqlx::{FromRow, PgPool};

type Error = sqlx::Error;

#[derive(Debug, Clone, FromRow)]
pub struct Plugin {
    pub id: String,
    pub name: String,
    pub version: String,
    pub capabilities: Vec<String>,
    pub approved: bool,
    pub enabled: bool,
    pub hooks: Vec<String>,
    pub module: Option<Vec<u8>>,
    pub slots: serde_json::Value,
    pub client_js: Option<String>,
    pub schema: Option<serde_json::Value>,
    pub settings: serde_json::Value,
    pub checksum: String,
    pub failures: i32,
    pub approved_capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Summary {
    pub id: String,
    pub name: String,
    pub version: String,
    pub capabilities: Vec<String>,
    pub approved: bool,
    pub enabled: bool,
    pub hooks: Vec<String>,
    pub schema: Option<serde_json::Value>,
    pub settings: serde_json::Value,
    pub checksum: String,
    pub failures: i32,
    /// What an administrator has already approved: an upgrade asks only about the rest.
    pub approved_capabilities: Vec<String>,
    pub prev_version: Option<String>,
}

pub async fn all(pool: &PgPool) -> Result<Vec<Plugin>, Error> {
    sqlx::query_as(
        "SELECT id, name, version, capabilities, approved, enabled, hooks, module, slots, client_js, schema, settings, checksum, failures, approved_capabilities
         FROM plugins ORDER BY id",
    )
    .fetch_all(pool)
    .await
}

pub async fn summaries(pool: &PgPool) -> Result<Vec<Summary>, Error> {
    sqlx::query_as("SELECT id, name, version, capabilities, approved, enabled, hooks, (schema - 'x-assets') AS schema, settings, checksum, failures, approved_capabilities, prev_version FROM plugins ORDER BY id")
        .fetch_all(pool)
        .await
}

pub struct NewPlugin<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub version: &'a str,
    pub capabilities: &'a [String],
    pub hooks: &'a [String],
    pub module: Option<&'a [u8]>,
    pub slots: &'a serde_json::Value,
    pub client_js: Option<&'a str>,
    pub schema: Option<&'a serde_json::Value>,
    pub checksum: &'a str,
    /// The zip as installed, kept so the next upgrade can be undone.
    pub package: &'a [u8],
    /// Going back to the previous package: it runs again at once if what it asks for was approved.
    pub rollback: bool,
}

/// Installs, or updates in place. A new plugin starts disabled. An update keeps what an
/// administrator approved: it stays enabled only if every capability it now asks for was
/// approved before, and otherwise waits for approval of the new ones. The package it replaces
/// is kept (`prev_package`) for a one-click rollback.
pub async fn upsert(pool: &PgPool, p: &NewPlugin<'_>) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO plugins (id, name, version, capabilities, hooks, module, slots, client_js, schema, checksum, package)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
         ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name, version = EXCLUDED.version, capabilities = EXCLUDED.capabilities,
           hooks = EXCLUDED.hooks, module = EXCLUDED.module, slots = EXCLUDED.slots, client_js = EXCLUDED.client_js,
           schema = EXCLUDED.schema, checksum = EXCLUDED.checksum,
           prev_version = CASE WHEN plugins.version <> EXCLUDED.version THEN plugins.version ELSE plugins.prev_version END,
           prev_package = CASE WHEN plugins.version <> EXCLUDED.version THEN plugins.package ELSE plugins.prev_package END,
           package = EXCLUDED.package,
           approved_capabilities = ARRAY(SELECT c FROM unnest(EXCLUDED.capabilities) c WHERE c = ANY(plugins.approved_capabilities)),
           approved = CASE WHEN $12 THEN cardinality(plugins.approved_capabilities) > 0 AND EXCLUDED.capabilities <@ plugins.approved_capabilities
                           ELSE plugins.approved AND EXCLUDED.capabilities <@ plugins.approved_capabilities END,
           enabled = CASE WHEN $12 THEN cardinality(plugins.approved_capabilities) > 0 AND EXCLUDED.capabilities <@ plugins.approved_capabilities
                          ELSE plugins.enabled AND EXCLUDED.capabilities <@ plugins.approved_capabilities END,
           failures = 0",
    )
    .bind(p.id)
    .bind(p.name)
    .bind(p.version)
    .bind(p.capabilities)
    .bind(p.hooks)
    .bind(p.module)
    .bind(p.slots)
    .bind(p.client_js)
    .bind(p.schema)
    .bind(p.checksum)
    .bind(p.package)
    .bind(p.rollback)
    .execute(pool)
    .await
    .map(|_| ())
}

/// The package before the current one, for a rollback.
pub async fn previous_package(pool: &PgPool, id: &str) -> Result<Option<Vec<u8>>, Error> {
    sqlx::query_scalar("SELECT prev_package FROM plugins WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map(Option::flatten)
}

pub async fn approve_and_enable(pool: &PgPool, id: &str, actor: Option<i64>) -> Result<bool, Error> {
    let n = sqlx::query("UPDATE plugins SET approved = true, enabled = true, failures = 0, approved_capabilities = capabilities WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected();
    sqlx::query("INSERT INTO audit_log (actor_id, action, target, details) SELECT $1, 'plugin.enable', $2, jsonb_build_object('capabilities', capabilities, 'checksum', checksum) FROM plugins WHERE id = $2")
        .bind(actor)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(n > 0)
}

pub async fn disable(pool: &PgPool, id: &str) -> Result<bool, Error> {
    Ok(sqlx::query("UPDATE plugins SET enabled = false WHERE id = $1").bind(id).execute(pool).await?.rows_affected()
        > 0)
}

pub async fn delete(pool: &PgPool, id: &str) -> Result<bool, Error> {
    Ok(sqlx::query("DELETE FROM plugins WHERE id = $1").bind(id).execute(pool).await?.rows_affected() > 0)
}

pub async fn set_settings(pool: &PgPool, id: &str, settings: &serde_json::Value) -> Result<bool, Error> {
    Ok(sqlx::query("UPDATE plugins SET settings = $2 WHERE id = $1")
        .bind(id)
        .bind(settings)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

/// Counts a failure and returns the new total.
pub async fn record_failure(pool: &PgPool, id: &str) -> Result<i32, Error> {
    sqlx::query_scalar(
        "UPDATE plugins SET
           failures = CASE WHEN last_failure_at < now() - interval '1 day' THEN 1 ELSE failures + 1 END,
           last_failure_at = now()
         WHERE id = $1 RETURNING failures",
    )
    .bind(id)
    .fetch_one(pool)
    .await
}

pub async fn kv_get(pool: &PgPool, plugin: &str, key: &str) -> Result<Option<String>, Error> {
    sqlx::query_scalar("SELECT value FROM plugin_kv WHERE plugin = $1 AND key = $2")
        .bind(plugin)
        .bind(key)
        .fetch_optional(pool)
        .await
}

pub async fn kv_set(pool: &PgPool, plugin: &str, key: &str, value: &str) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO plugin_kv (plugin, key, value) VALUES ($1, $2, $3)
         ON CONFLICT (plugin, key) DO UPDATE SET value = EXCLUDED.value",
    )
    .bind(plugin)
    .bind(key)
    .bind(value)
    .execute(pool)
    .await
    .map(|_| ())
}

pub async fn kv_delete(pool: &PgPool, plugin: &str, key: &str) -> Result<(), Error> {
    sqlx::query("DELETE FROM plugin_kv WHERE plugin = $1 AND key = $2")
        .bind(plugin)
        .bind(key)
        .execute(pool)
        .await
        .map(|_| ())
}

// --- Declared tables. Plugins never write SQL: they declare tables in the manifest and
// send JSON operations. Every identifier below is checked against the declaration and every
// value is bound, so a plugin can reach only its own `p_<id>__<table>` tables.

#[derive(Debug, Clone, Default, Serialize, serde::Deserialize)]
pub struct TableSpec {
    pub name: String,
    pub columns: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub index: Vec<String>,
    /// Renamed columns, new name to old name: `renames = { body = "text" }`. Applied before
    /// columns are added, so the data moves with the name.
    #[serde(default)]
    pub renames: std::collections::BTreeMap<String, String>,
    /// Columns whose declared type may change; existing values are cast through text.
    #[serde(default)]
    pub retype: Vec<String>,
    /// An `int` column holding a member's id: those rows are deleted when the member is erased
    /// and are part of the member's data export.
    #[serde(default)]
    pub user_column: Option<String>,
    /// Columns that must hold different values in every row (a slug): inserting the same value
    /// again fails with "That value already exists." and nothing is written.
    #[serde(default)]
    pub unique: Vec<String>,
}

const MAX_ROWS: i64 = 500;

/// Deepest page a `select` may skip to (`offset`).
const MAX_OFFSET: i64 = 100_000;

/// Most values in one `in` filter, and most columns in one `order`.
const MAX_IN: usize = 50;
const MAX_ORDER: usize = 3;

/// A `where` value that is an object of operators (`{"gte": 1, "lt": 9}`): `eq`, `ne`, `gt`, `gte`,
/// `lt`, `lte`, `in` (a list), and for text `prefix` and `contains` (case-insensitive). `None` when
/// the value is a plain value to compare for equality (and always for `json` columns).
fn condition_ops<'a>(v: &'a serde_json::Value, ty: &str) -> Option<Vec<(&'a str, &'a serde_json::Value)>> {
    const NAMES: [&str; 10] = ["eq", "ne", "gt", "gte", "lt", "lte", "in", "prefix", "contains", "isnull"];
    let map = v.as_object().filter(|m| !m.is_empty() && ty != "jsonb")?;
    if !map.keys().all(|k| NAMES.contains(&k.as_str())) {
        return None;
    }
    Some(map.iter().map(|(k, v)| (k.as_str(), v)).collect())
}

/// One filter of a `where` object, with its value bound (never pasted into the statement).
fn condition(
    binds: &mut Vec<Option<String>>,
    column: &str,
    ty: &str,
    op: &str,
    arg: &serde_json::Value,
) -> Result<String, String> {
    let mut bind = |v: &serde_json::Value| {
        binds.push(bind_text(v));
        binds.len()
    };
    let compare = |sym: &str, n: usize| format!("\"{column}\" {sym} ${n}::text::{ty}");
    Ok(match op {
        "eq" => compare("=", bind(arg)),
        "ne" => format!("\"{column}\" IS DISTINCT FROM ${}::text::{ty}", bind(arg)),
        "gt" | "gte" | "lt" | "lte" if ty == "boolean" => return Err(format!("{op} does not apply to {column}.")),
        "gt" => compare(">", bind(arg)),
        "gte" => compare(">=", bind(arg)),
        "lt" => compare("<", bind(arg)),
        "lte" => compare("<=", bind(arg)),
        "in" => {
            let list = arg.as_array().filter(|l| !l.is_empty() && l.len() <= MAX_IN).ok_or("in takes 1 to 50 values.")?;
            let parts: Vec<String> = list.iter().map(|v| compare("=", bind(v))).collect();
            format!("({})", parts.join(" OR "))
        }
        "prefix" | "contains" if ty != "text" => return Err(format!("{op} applies to text columns only.")),
        "prefix" => format!("starts_with(\"{column}\", ${}::text)", bind(arg)),
        "contains" => format!("strpos(lower(\"{column}\"), lower(${}::text)) > 0", bind(arg)),
        "isnull" => format!("\"{column}\" IS {}NULL", if arg.as_bool().unwrap_or(true) { "" } else { "NOT " }),
        _ => return Err("Unknown filter.".into()),
    })
}

/// `ORDER BY` from `order`: a column, with `desc`; or a list of up to three columns, each
/// optionally prefixed `-` for descending (`["-pinned", "-created_at"]`). Empty when not given.
fn order_by(
    req: &serde_json::Value,
    col: &impl Fn(&str) -> Result<&'static str, String>,
) -> Result<String, String> {
    let desc_all = req.get("desc").and_then(serde_json::Value::as_bool).unwrap_or(false);
    let wanted: Vec<(String, bool)> = match req.get("order") {
        Some(serde_json::Value::String(c)) => vec![(c.trim_start_matches('-').to_owned(), desc_all || c.starts_with('-'))],
        Some(serde_json::Value::Array(list)) => list
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(|c| (c.trim_start_matches('-').to_owned(), c.starts_with('-')))
            .collect(),
        _ => vec![],
    };
    if wanted.len() > MAX_ORDER {
        return Err("Order by at most 3 columns.".into());
    }
    let mut parts = Vec::new();
    for (c, desc) in &wanted {
        col(c)?;
        parts.push(format!("\"{c}\" {}", if *desc { "DESC" } else { "ASC" }));
    }
    if parts.is_empty() {
        return Ok(String::new());
    }
    // A last key keeps pages stable when the chosen columns tie.
    parts.push("id".to_owned());
    Ok(format!(" ORDER BY {}", parts.join(", ")))
}

const MAX_PENDING_JOBS: i64 = 1000;

fn valid_ident(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 30
        && !s.contains("__")
        && s.starts_with(|c: char| c.is_ascii_lowercase())
        && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn sql_type(t: &str) -> Option<&'static str> {
    Some(match t {
        "text" => "text",
        "int" => "bigint",
        "bool" => "boolean",
        "json" => "jsonb",
        "timestamp" => "timestamptz",
        "float" => "double precision",
        _ => return None,
    })
}

fn table_prefix(plugin: &str) -> String {
    format!("p_{}__", plugin.replace('-', "_"))
}

pub fn validate_tables(plugin: &str, tables: &[TableSpec]) -> Result<(), String> {
    for t in tables {
        if !valid_ident(&t.name) || table_prefix(plugin).len() + t.name.len() > 63 {
            return Err(format!("Bad table name {}.", t.name));
        }
        for (c, ty) in &t.columns {
            if !valid_ident(c) || c == "id" || sql_type(ty).is_none() {
                return Err(format!("Bad column {c} in table {}.", t.name));
            }
        }
        if let Some(i) = t.index.iter().chain(&t.unique).find(|i| !t.columns.contains_key(*i)) {
            return Err(format!("Index on unknown column {i}."));
        }
        // An index is named `{table}_{column}`; Postgres cuts names at 63 bytes, which would make
        // two indexes one, and the migration drop and rebuild a cut one every time.
        let full = table_prefix(plugin).len() + t.name.len();
        if let Some(i) = t.index.iter().chain(&t.unique).find(|i| full + 1 + i.len() > 63) {
            return Err(format!("The name of the index on {i} in table {} is too long.", t.name));
        }
        if let Some((new, old)) =
            t.renames.iter().find(|(n, o)| !t.columns.contains_key(*n) || !valid_ident(o) || *o == "id")
        {
            return Err(format!("Bad rename {old} to {new} in table {}.", t.name));
        }
        if let Some(c) = t.retype.iter().find(|c| !t.columns.contains_key(*c)) {
            return Err(format!("Retype of unknown column {c}."));
        }
        if let Some(c) = &t.user_column
            && t.columns.get(c).map(String::as_str) != Some("int")
        {
            return Err(format!("user_column {c} in table {} must be a declared int column.", t.name));
        }
    }
    Ok(())
}

/// The name Postgres reports for a column of each declared type.
fn pg_type_name(t: &str) -> &'static str {
    match t {
        "int" => "bigint",
        "bool" => "boolean",
        "json" => "jsonb",
        "timestamp" => "timestamp with time zone",
        "float" => "double precision",
        _ => "text",
    }
}

async fn has_column(tx: &mut sqlx::PgConnection, table: &str, column: &str) -> Result<bool, Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM pg_attribute WHERE attrelid = to_regclass($1) AND attname = $2
                          AND NOT attisdropped AND attnum > 0)",
    )
    .bind(format!("\"{table}\""))
    .bind(column)
    .fetch_one(tx)
    .await
}

/// Creates tables, then applies what the manifest declares: renamed columns, new columns,
/// columns retyped with a cast, and the index list (indexes no longer declared are dropped), all
/// or nothing. Columns are never dropped by an update.
pub async fn migrate_tables(pool: &PgPool, plugin: &str, tables: &[TableSpec]) -> Result<(), Error> {
    if validate_tables(plugin, tables).is_err() {
        return Err(Error::Protocol("invalid plugin tables".into()));
    }
    let mut tx = pool.begin().await?;
    for t in tables {
        let name = format!("{}{}", table_prefix(plugin), t.name);
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE TABLE IF NOT EXISTS \"{name}\" (id bigserial PRIMARY KEY)")))
            .execute(&mut *tx)
            .await?;
        for (new, old) in &t.renames {
            if has_column(&mut tx, &name, old).await? && !has_column(&mut tx, &name, new).await? {
                sqlx::query(sqlx::AssertSqlSafe(format!(
                    "ALTER TABLE \"{name}\" RENAME COLUMN \"{old}\" TO \"{new}\""
                )))
                .execute(&mut *tx)
                .await?;
            }
        }
        for (c, ty) in &t.columns {
            let ty = sql_type(ty).unwrap_or("text");
            sqlx::query(sqlx::AssertSqlSafe(format!("ALTER TABLE \"{name}\" ADD COLUMN IF NOT EXISTS \"{c}\" {ty}")))
                .execute(&mut *tx)
                .await?;
        }
        for c in &t.retype {
            let declared = t.columns.get(c).map_or("text", String::as_str);
            let current: Option<String> = sqlx::query_scalar(
                "SELECT format_type(atttypid, atttypmod) FROM pg_attribute
                 WHERE attrelid = to_regclass($1) AND attname = $2 AND NOT attisdropped",
            )
            .bind(format!("\"{name}\""))
            .bind(c)
            .fetch_optional(&mut *tx)
            .await?;
            if current.as_deref().is_some_and(|cur| cur != pg_type_name(declared)) {
                let ty = sql_type(declared).unwrap_or("text");
                sqlx::query(sqlx::AssertSqlSafe(format!(
                    "ALTER TABLE \"{name}\" ALTER COLUMN \"{c}\" TYPE {ty} USING \"{c}\"::text::{ty}"
                )))
                .execute(&mut *tx)
                .await?;
            }
        }
        let wanted: Vec<String> = t.index.iter().chain(&t.unique).map(|c| format!("{name}_{c}")).collect();
        let existing: Vec<String> = sqlx::query_scalar(
            "SELECT indexname::text FROM pg_indexes WHERE schemaname = current_schema() AND tablename = $1
               AND starts_with(indexname::text, $2) AND indexname::text <> $3",
        )
        .bind(&name)
        .bind(format!("{name}_"))
        .bind(format!("{name}_pkey"))
        .fetch_all(&mut *tx)
        .await?;
        for gone in existing.iter().filter(|i| !wanted.contains(i)) {
            sqlx::query(sqlx::AssertSqlSafe(format!("DROP INDEX IF EXISTS \"{}\"", gone.replace('"', ""))))
                .execute(&mut *tx)
                .await?;
        }
        for c in t.index.iter().filter(|c| !t.unique.contains(c)) {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "CREATE INDEX IF NOT EXISTS \"{name}_{c}\" ON \"{name}\" (\"{c}\")"
            )))
            .execute(&mut *tx)
            .await?;
        }
        for c in &t.unique {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "CREATE UNIQUE INDEX IF NOT EXISTS \"{name}_{c}\" ON \"{name}\" (\"{c}\")"
            )))
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await
}

/// Uninstall: removes every table with this plugin's prefix, including ones a newer version dropped.
pub async fn drop_tables(pool: &PgPool, plugin: &str) -> Result<(), Error> {
    let names: Vec<String> = sqlx::query_scalar(
        "SELECT tablename::text FROM pg_tables WHERE schemaname = current_schema() AND starts_with(tablename::text, $1)",
    )
    .bind(table_prefix(plugin))
    .fetch_all(pool)
    .await?;
    for n in names {
        sqlx::query(sqlx::AssertSqlSafe(format!("DROP TABLE \"{}\"", n.replace('"', "")))).execute(pool).await?;
    }
    Ok(())
}

struct Built {
    sql: String,
    binds: Vec<Option<String>>,
    returns_rows: bool,
}

fn bind_text(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Null => None,
        serde_json::Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

fn build_op(plugin: &str, tables: &[TableSpec], req: &serde_json::Value) -> Result<Built, String> {
    let table = req.get("table").and_then(|v| v.as_str()).unwrap_or_default();
    let spec = tables.iter().find(|t| t.name == table).ok_or_else(|| format!("Unknown table {table}."))?;
    let name = format!("{}{}", table_prefix(plugin), spec.name);
    let col = |c: &str| -> Result<&'static str, String> {
        if c == "id" {
            return Ok("bigint");
        }
        spec.columns.get(c).and_then(|t| sql_type(t)).ok_or_else(|| format!("Unknown column {c}."))
    };
    let mut binds = Vec::new();
    let empty = serde_json::Map::new();
    let wheres = req.get("where").and_then(|v| v.as_object()).unwrap_or(&empty);
    let values = req.get("values").and_then(|v| v.as_object()).unwrap_or(&empty);
    let mut sets: Vec<(&String, &'static str, Option<String>)> = Vec::new();
    for (k, v) in values {
        if k == "id" {
            return Err("The id column cannot be set.".into());
        }
        sets.push((k, col(k)?, bind_text(v)));
    }
    let action = req.get("action").and_then(|v| v.as_str()).unwrap_or_default();
    let mut assign = Vec::new();
    if action == "update" {
        for (k, ty, v) in &sets {
            binds.push(v.clone());
            assign.push(format!("\"{k}\" = ${}::text::{ty}", binds.len()));
        }
    }
    let mut clause = Vec::new();
    for (k, v) in wheres {
        let ty = col(k)?;
        if v.is_null() {
            clause.push(format!("\"{k}\" IS NULL"));
        } else if let Some(ops) = condition_ops(v, ty) {
            for (op, arg) in ops {
                clause.push(condition(&mut binds, k, ty, op, arg)?);
            }
        } else {
            binds.push(bind_text(v));
            clause.push(format!("\"{k}\" = ${}::text::{ty}", binds.len()));
        }
    }
    let filter = if clause.is_empty() { String::new() } else { format!(" WHERE {}", clause.join(" AND ")) };
    let (sql, returns_rows) = match action {
        "select" => {
            let mut order = order_by(req, &col)?;
            if order.is_empty() {
                order = " ORDER BY id".to_owned();
            }
            let limit = req.get("limit").and_then(serde_json::Value::as_i64).unwrap_or(MAX_ROWS).clamp(1, MAX_ROWS);
            let offset = req.get("offset").and_then(serde_json::Value::as_i64).unwrap_or(0).clamp(0, MAX_OFFSET);
            (format!("SELECT to_jsonb(t) FROM \"{name}\" t{filter}{order} LIMIT {limit} OFFSET {offset}"), true)
        }
        "count" => (format!("SELECT jsonb_build_object('count', count(*)) FROM \"{name}\" t{filter}"), true),
        "insert" => {
            if sets.is_empty() {
                return Err("Nothing to insert.".into());
            }
            let cols: Vec<String> = sets.iter().map(|(k, ..)| format!("\"{k}\"")).collect();
            let mut slots = Vec::new();
            for (_, ty, v) in &sets {
                binds.push(v.clone());
                slots.push(format!("${}::text::{ty}", binds.len()));
            }
            let sql = format!(
                "INSERT INTO \"{name}\" ({}) VALUES ({}) RETURNING jsonb_build_object('id', id)",
                cols.join(", "),
                slots.join(", ")
            );
            (sql, true)
        }
        "update" if assign.is_empty() => return Err("Nothing to update.".into()),
        "update" | "delete" if clause.is_empty() => return Err("Update and delete need a where.".into()),
        "update" => (format!("UPDATE \"{name}\" SET {}{filter}", assign.join(", ")), false),
        "delete" => (format!("DELETE FROM \"{name}\"{filter}"), false),
        _ => return Err("Unknown action.".into()),
    };
    Ok(Built { sql, binds, returns_rows })
}

/// What the plugin is told when a statement fails: a repeated value in a `unique` column is
/// named, everything else is left vague.
fn op_failed(e: sqlx::Error) -> String {
    let duplicate = e.as_database_error().and_then(|d| d.code()).is_some_and(|c| c == "23505");
    if duplicate { "That value already exists.".to_owned() } else { "The operation failed.".to_owned() }
}

/// Runs one JSON operation. Errors go back to the plugin with database details left out.
pub async fn table_op(
    pool: &PgPool,
    plugin: &str,
    tables: &[TableSpec],
    req: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let b = build_op(plugin, tables, req)?;
    if b.returns_rows {
        let mut q = sqlx::query_scalar::<_, serde_json::Value>(sqlx::AssertSqlSafe(b.sql.clone()));
        for v in &b.binds {
            q = q.bind(v.clone());
        }
        let rows = q.fetch_all(pool).await.map_err(op_failed)?;
        return Ok(serde_json::Value::Array(rows));
    }
    let mut q = sqlx::query(sqlx::AssertSqlSafe(b.sql.clone()));
    for v in &b.binds {
        q = q.bind(v.clone());
    }
    let done = q.execute(pool).await.map_err(op_failed)?;
    Ok(serde_json::json!({ "affected": done.rows_affected() }))
}

/// The plugin tables that hold a member's rows: (plugin, table name, full table name, column).
async fn user_tables(conn: &mut sqlx::PgConnection) -> Result<Vec<(String, String, String, String)>, Error> {
    let rows: Vec<(String, serde_json::Value)> =
        sqlx::query_as("SELECT id, schema->'x-tables' FROM plugins WHERE schema ? 'x-tables'")
            .fetch_all(&mut *conn)
            .await?;
    let mut out = Vec::new();
    for (plugin, specs) in rows {
        let specs: Vec<TableSpec> = serde_json::from_value(specs).unwrap_or_default();
        for t in specs {
            let Some(col) = t.user_column.clone() else { continue };
            if validate_tables(&plugin, std::slice::from_ref(&t)).is_err() {
                continue;
            }
            let table = format!("{}{}", table_prefix(&plugin), t.name);
            let there: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
                .bind(format!("\"{table}\""))
                .fetch_one(&mut *conn)
                .await?;
            if there {
                out.push((plugin.clone(), t.name, table, col));
            }
        }
    }
    Ok(out)
}

/// Erasure: deletes a member's rows from every plugin table that declares a `user_column`, and
/// the key-value keys under `user:<id>:`. Runs inside the account's erasure transaction.
pub async fn purge_user(conn: &mut sqlx::PgConnection, user: i64) -> Result<(), Error> {
    for (_, _, table, col) in user_tables(&mut *conn).await? {
        // Both names are validated identifiers from the plugin's own declaration; the id is bound.
        sqlx::query(sqlx::AssertSqlSafe(format!("DELETE FROM \"{table}\" WHERE \"{col}\" = $1")))
            .bind(user)
            .execute(&mut *conn)
            .await?;
    }
    sqlx::query("DELETE FROM plugin_meta WHERE target = 'user' AND target_id = $1").bind(user).execute(&mut *conn).await?;
    sqlx::query("DELETE FROM plugin_kv WHERE starts_with(key, $1)")
        .bind(format!("user:{user}:"))
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// What plugins hold about a member, for the data export: `{plugin: {table: [rows], "kv": {...}}}`.
pub async fn export_user(pool: &PgPool, user: i64) -> Result<serde_json::Value, Error> {
    let mut conn = pool.acquire().await?;
    let mut out = serde_json::Map::new();
    for (plugin, name, table, col) in user_tables(&mut conn).await? {
        let rows: serde_json::Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT coalesce(jsonb_agg(to_jsonb(r)), '[]') FROM (SELECT * FROM \"{table}\" WHERE \"{col}\" = $1 LIMIT 10000) r"
        )))
        .bind(user)
        .fetch_one(&mut *conn)
        .await?;
        let entry = out.entry(plugin).or_insert_with(|| serde_json::json!({}));
        entry[name] = rows;
    }
    // Metadata on the member's own topics and posts.
    let meta: Vec<(String, String, String, i64, serde_json::Value)> = sqlx::query_as(
        "SELECT plugin, key, target, target_id, value FROM plugin_meta m
         WHERE (target = 'topic' AND target_id IN (SELECT id FROM items WHERE author_id = $1))
            OR (target = 'post' AND target_id IN (SELECT id FROM posts WHERE author_id = $1))
            OR (target = 'user' AND target_id = $1)
         ORDER BY plugin, key LIMIT 10000",
    )
    .bind(user)
    .fetch_all(&mut *conn)
    .await?;
    for (plugin, key, target, id, value) in meta {
        let entry = out.entry(plugin).or_insert_with(|| serde_json::json!({}));
        entry["meta"][format!("{target}:{id}:{key}")] = value;
    }
    let kv: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT plugin, key, value FROM plugin_kv WHERE starts_with(key, $1) ORDER BY plugin, key LIMIT 10000",
    )
    .bind(format!("user:{user}:"))
    .fetch_all(&mut *conn)
    .await?;
    for (plugin, key, value) in kv {
        let entry = out.entry(plugin).or_insert_with(|| serde_json::json!({}));
        entry["kv"][key] = serde_json::Value::String(value);
    }
    Ok(serde_json::Value::Object(out))
}

/// Bytes a plugin holds: its tables with their indexes, plus its key-value data.
pub async fn storage_used(pool: &PgPool, plugin: &str) -> Result<i64, Error> {
    sqlx::query_scalar(
        "SELECT COALESCE((SELECT sum(pg_total_relation_size(quote_ident(tablename)::regclass)) FROM pg_tables
                          WHERE schemaname = current_schema() AND starts_with(tablename::text, $1)), 0)::bigint
              + COALESCE((SELECT sum(octet_length(key) + octet_length(value)) FROM plugin_kv WHERE plugin = $2), 0)::bigint
              + COALESCE((SELECT sum(octet_length(value::text) + octet_length(key) + 16) FROM plugin_meta WHERE plugin = $2), 0)::bigint
              + COALESCE((SELECT sum(size + octet_length(path)) FROM plugin_files WHERE plugin = $2), 0)::bigint
              + COALESCE((SELECT sum(octet_length(title) + octet_length(body) + octet_length(url) + 64) FROM plugin_search_docs WHERE plugin = $2), 0)::bigint
              + COALESCE((SELECT sum(octet_length(title) + octet_length(excerpt) + octet_length(url) + 64) FROM plugin_review_items WHERE plugin = $2), 0)::bigint",
    )
    .bind(table_prefix(plugin))
    .bind(plugin)
    .fetch_one(pool)
    .await
}

pub async fn posts_last_hour(pool: &PgPool, author: i64) -> Result<i64, Error> {
    sqlx::query_scalar("SELECT count(*) FROM posts WHERE author_id = $1 AND created_at > now() - interval '1 hour'")
        .bind(author)
        .fetch_one(pool)
        .await
}

pub fn bot_username(plugin: &str) -> String {
    let name = format!("plugin-{plugin}");
    if name.len() <= 30 {
        return name;
    }
    // Two long ids may share their first 30 characters, so the end is a hash of the whole name.
    let hash = name.bytes().fold(0x811c_9dc5_u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193));
    // Plugin ids are ASCII, but the cut must not depend on it: a byte slice can split a character.
    format!("{}-{hash:08x}", name.chars().take(21).collect::<String>())
}

/// The account a plugin posts under. Never an admin, whatever the state of the site, and with no
/// way to sign in: no password, and a reserved mail domain, so a reset can never be delivered.
pub async fn ensure_bot(pool: &PgPool, plugin: &str) -> Result<i64, Error> {
    let email = format!("{plugin}@plugins.invalid");
    let name = bot_username(plugin);
    let existing: Option<i64> = sqlx::query_scalar("SELECT id FROM users WHERE email_lower = lower($1)")
        .bind(&email)
        .fetch_optional(pool)
        .await?;
    let id =
        match existing {
            Some(id) => id,
            None => {
                let mut tx = pool.begin().await?;
                let (id, _) = crate::accounts::users::insert_user(
                    &mut tx,
                    &crate::accounts::users::NewUser {
                        username: &name,
                        email: &email,
                        password_hash: None,
                        display_name: None,
                        primary_group: Some(2),
                        joined_at: None,
                        verified: true,
                        kind: crate::accounts::users::AccountKind::Bot,
                        fit_name: true,
                    },
                )
                .await?;
                tx.commit().await?;
                id.get()
            }
        };
    sqlx::query("UPDATE plugins SET bot_user_id = $2 WHERE id = $1").bind(plugin).bind(id).execute(pool).await?;
    Ok(id)
}

pub async fn bot_user(pool: &PgPool, plugin: &str) -> Result<Option<i64>, Error> {
    sqlx::query_scalar("SELECT bot_user_id FROM plugins WHERE id = $1").bind(plugin).fetch_one(pool).await
}

#[derive(Debug, Clone, FromRow)]
pub struct Job {
    pub id: i64,
    pub plugin: String,
    pub payload: serde_json::Value,
    pub attempts: i32,
}

/// False when the plugin already has too many waiting.
pub async fn enqueue_job(pool: &PgPool, plugin: &str, payload: &serde_json::Value, delay: i64) -> Result<bool, Error> {
    let n = sqlx::query(
        "INSERT INTO plugin_jobs (plugin, payload, run_at)
         SELECT $1, $2, now() + make_interval(secs => $3::double precision)
         WHERE (SELECT count(*) FROM plugin_jobs WHERE plugin = $1) < $4",
    )
    .bind(plugin)
    .bind(payload)
    .bind(delay as f64)
    .bind(MAX_PENDING_JOBS)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(n > 0)
}

pub async fn due_jobs(pool: &PgPool, plugins: &[String], limit: i64) -> Result<Vec<Job>, Error> {
    sqlx::query_as(
        "SELECT id, plugin, payload, attempts FROM plugin_jobs
         WHERE plugin = ANY($1) AND run_at <= now() ORDER BY run_at LIMIT $2",
    )
    .bind(plugins)
    .bind(limit)
    .fetch_all(pool)
    .await
}

pub async fn finish_job(pool: &PgPool, id: i64) -> Result<(), Error> {
    sqlx::query("DELETE FROM plugin_jobs WHERE id = $1").bind(id).execute(pool).await.map(|_| ())
}

pub async fn retry_job(pool: &PgPool, id: i64, attempts: i32) -> Result<(), Error> {
    sqlx::query("UPDATE plugin_jobs SET attempts = $2, run_at = now() + make_interval(secs => 60.0 * $2) WHERE id = $1")
        .bind(id)
        .bind(attempts)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Creates or replaces a recurring job. False when the plugin is at `max` schedules.
pub async fn set_schedule(
    pool: &PgPool,
    plugin: &str,
    name: &str,
    every_minutes: i32,
    payload: &serde_json::Value,
    max: i64,
) -> Result<bool, Error> {
    let n = sqlx::query(
        "INSERT INTO plugin_schedules (plugin, name, every_minutes, payload, next_run)
         SELECT $1, $2, $3, $4, now() + make_interval(mins => $3)
         WHERE EXISTS (SELECT 1 FROM plugin_schedules WHERE plugin = $1 AND name = $2)
            OR (SELECT count(*) FROM plugin_schedules WHERE plugin = $1) < $5
         ON CONFLICT (plugin, name) DO UPDATE SET every_minutes = EXCLUDED.every_minutes, payload = EXCLUDED.payload",
    )
    .bind(plugin)
    .bind(name)
    .bind(every_minutes)
    .bind(payload)
    .bind(max)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(n > 0)
}

pub async fn cancel_schedule(pool: &PgPool, plugin: &str, name: &str) -> Result<bool, Error> {
    Ok(sqlx::query("DELETE FROM plugin_schedules WHERE plugin = $1 AND name = $2")
        .bind(plugin)
        .bind(name)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

/// Schedules that are due for these plugins, moved on to their next run. Several nodes may call
/// it: each due row goes to one of them.
pub async fn take_due_schedules(
    pool: &PgPool,
    plugins: &[String],
    limit: i64,
) -> Result<Vec<(String, String, serde_json::Value)>, Error> {
    sqlx::query_as(
        "UPDATE plugin_schedules s SET next_run = now() + make_interval(mins => s.every_minutes)
         WHERE (s.plugin, s.name) IN (SELECT plugin, name FROM plugin_schedules
                                      WHERE plugin = ANY($1) AND next_run <= now()
                                      ORDER BY next_run LIMIT $2 FOR UPDATE SKIP LOCKED)
         RETURNING s.plugin, s.name, s.payload",
    )
    .bind(plugins)
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// Keys under a prefix, so plugins can keep collections without SQL.
pub async fn kv_keys(pool: &PgPool, plugin: &str, prefix: &str) -> Result<Vec<String>, Error> {
    sqlx::query_scalar("SELECT key FROM plugin_kv WHERE plugin = $1 AND starts_with(key, $2) ORDER BY key LIMIT 1000")
        .bind(plugin)
        .bind(prefix)
        .fetch_all(pool)
        .await
}

/// Needs a database with the `probe` plugin's `notes` table and rows for users 3 and 4:
/// BF_DATABASE_URL=... cargo test -p bf-db --lib erasure_purges -- --ignored
#[cfg(test)]
mod erasure_tests {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn erasure_purges_plugin_rows_and_export_lists_them() {
        let url = std::env::var("BF_DATABASE_URL").unwrap();
        let pool = crate::connect(&url, 1).await.unwrap();
        let before = export_user(&pool, 3).await.unwrap();
        assert_eq!(before["probe"]["notes"][0]["body"], "alice note", "{before}");
        let mut tx = pool.begin().await.unwrap();
        purge_user(&mut tx, 3).await.unwrap();
        tx.commit().await.unwrap();
        let after = export_user(&pool, 3).await.unwrap();
        assert_eq!(after["probe"]["notes"].as_array().map(Vec::len), Some(0), "{after}");
        let bob = export_user(&pool, 4).await.unwrap();
        assert_eq!(bob["probe"]["notes"][0]["body"], "bob note", "bob keeps his rows");
    }
}

#[cfg(test)]
mod table_tests {
    use super::*;
    use serde_json::json;

    fn spec() -> Vec<TableSpec> {
        vec![TableSpec {
            name: "notes".into(),
            columns: [("body".to_owned(), "text".to_owned()), ("n".to_owned(), "int".to_owned())].into(),
            index: vec![],
            ..Default::default()
        }]
    }

    #[test]
    fn content_app_queries_are_built_from_declared_names_and_bound_values() {
        let s = spec();
        let sql = |req: serde_json::Value| build_op("a-b", &s, &req).unwrap();
        let page = sql(json!({"action":"select","table":"notes","order":["-n","body"],"offset":40,"limit":20,
            "where":{"n":{"gte":3,"lt":9},"body":{"prefix":"x'; DROP TABLE users; --"}}}));
        assert!(page.sql.contains("ORDER BY \"n\" DESC, \"body\" ASC, id") && page.sql.contains("OFFSET 40"), "{}", page.sql);
        assert!(page.sql.contains(">=") && page.sql.contains("starts_with"), "{}", page.sql);
        assert!(!page.sql.contains("DROP"), "values are bound, not pasted");
        assert_eq!(page.binds.len(), 3);
        let count = sql(json!({"action":"count","table":"notes","where":{"n":{"in":[1,2,3]}}}));
        assert!(count.sql.contains("count(*)") && count.binds.len() == 3 && count.returns_rows);
        assert!(build_op("a-b", &s, &json!({"action":"select","table":"notes","order":["n","n","n","n"]})).is_err());
        assert!(build_op("a-b", &s, &json!({"action":"select","table":"notes","where":{"n":{"prefix":"1"}}})).is_err());
        assert!(build_op("a-b", &s, &json!({"action":"select","table":"notes","where":{"n":{"in":[]}}})).is_err());
    }

    #[test]
    fn only_declared_names_reach_sql() {
        let s = spec();
        assert!(build_op("a-b", &s, &json!({"action":"select","table":"users"})).is_err());
        assert!(build_op("a-b", &s, &json!({"action":"select","table":"notes","where":{"x\" OR 1=1 --":1}})).is_err());
        assert!(
            build_op("a-b", &s, &json!({"action":"select","table":"notes","order":"body; DROP TABLE users"})).is_err()
        );
        assert!(build_op("a-b", &s, &json!({"action":"insert","table":"notes","values":{"evil\"":1}})).is_err());
    }

    #[test]
    fn values_are_bound_not_written_into_sql() {
        let req = json!({"action":"insert","table":"notes","values":{"body":"'); DROP TABLE users; --"}});
        let b = build_op("a-b", &spec(), &req).unwrap();
        assert!(b.sql.starts_with("INSERT INTO \"p_a_b__notes\""));
        assert!(!b.sql.contains("DROP"));
        assert_eq!(b.binds.len(), 1);
    }

    #[test]
    fn writes_need_a_where_and_id_is_host_owned() {
        let s = spec();
        assert!(build_op("a", &s, &json!({"action":"delete","table":"notes"})).is_err());
        assert!(build_op("a", &s, &json!({"action":"update","table":"notes","values":{"n":1}})).is_err());
        assert!(build_op("a", &s, &json!({"action":"insert","table":"notes","values":{"id":1}})).is_err());
        let u =
            build_op("a", &s, &json!({"action":"update","table":"notes","values":{"n":1},"where":{"id":2}})).unwrap();
        assert_eq!(u.binds, vec![Some("1".to_owned()), Some("2".to_owned())]);
        assert!(u.sql.contains("\"n\" = $1::text::bigint") && u.sql.contains("\"id\" = $2::text::bigint"));
    }

    #[test]
    fn bot_usernames_cut_on_a_character_boundary() {
        // After the 7 bytes of "plugin-", two-byte letters put byte 21 inside a character.
        let long = "\u{e9}".repeat(30);
        let name = bot_username(&long);
        assert!(name.starts_with("plugin-") && name.len() > 21, "{name}");
        assert_eq!(bot_username("forum"), "plugin-forum");
    }

    #[test]
    fn declarations_are_checked() {
        let bad = |name: &str, col: &str, ty: &str| {
            let t = TableSpec { name: name.into(), columns: [(col.to_owned(), ty.to_owned())].into(), ..Default::default() };
            validate_tables("a", &[t]).is_err()
        };
        assert!(!bad("notes", "body", "text"));
        assert!(bad("a__b", "body", "text"), "double underscore would blur the plugin boundary");
        assert!(bad("no\"tes", "body", "text"));
        assert!(bad("notes", "id", "text"));
        assert!(bad("notes", "body", "text; DROP"));
    }
}
