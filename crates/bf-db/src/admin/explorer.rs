//! Data Explorer: read-only SQL reports for administrators.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
pub use sqlx::Error;
use sqlx::{FromRow, PgPool};

/// Most rows one query returns.
pub const MAX_ROWS: i64 = 1000;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct SavedQuery {
    #[serde(default)]
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub sql: String,
    #[serde(default = "now")]
    pub updated_at: DateTime<Utc>,
}

fn now() -> DateTime<Utc> {
    Utc::now()
}

/// A startup self-test of the Explorer connection: it must log in as the restricted
/// `bf_explorer` role and must not be a superuser. Returns the current role name when it is safe,
/// or an error describing why the connection is unsafe to run untrusted SQL on.
pub async fn check_connection(pool: &PgPool) -> Result<String, String> {
    let (role, superuser): (String, bool) =
        sqlx::query_as("SELECT current_user::text, current_setting('is_superuser') = 'on'")
            .fetch_one(pool)
            .await
            .map_err(|e| e.to_string())?;
    if superuser {
        return Err(format!("BF_EXPLORER_DATABASE_URL logs in as superuser {role}; use the bf_explorer role."));
    }
    if role != "bf_explorer" {
        return Err(format!(
            "BF_EXPLORER_DATABASE_URL logs in as {role}, not bf_explorer; the Explorer must run as the restricted role (see deploy/postgres/roles.sql)."
        ));
    }
    Ok(role)
}

/// The views a query can read, with their columns: (view, columns).
pub async fn schema(pool: &PgPool) -> Result<Vec<(String, Vec<String>)>, Error> {
    sqlx::query_as(
        "SELECT table_name::text, array_agg(column_name::text ORDER BY ordinal_position)
         FROM information_schema.columns WHERE table_schema = 'explorer' GROUP BY table_name ORDER BY table_name",
    )
    .fetch_all(pool)
    .await
}

/// Runs one query in a read-only transaction that is always rolled back, with a time limit. The
/// query is wrapped as a subquery, so it must be a single SELECT (or WITH); one statement is all
/// the protocol takes. Returns the rows as a JSON array, in column order, and whether there were
/// more than `MAX_ROWS`.
///
/// `pool` MUST be [`crate::explorer_pool`], i.e. a connection whose login role is the restricted
/// `bf_explorer`. The confinement comes from that login role, not from the statements below: a
/// submitted query that does `RESET ROLE`, or calls a built-in that executes SQL, still only
/// reaches `bf_explorer`'s rights. The earlier design used `SET LOCAL ROLE bf_explorer` on the
/// application's own connection, which the submitted SQL could switch back out of to the
/// privileged application role (Data Explorer).
pub async fn run(pool: &PgPool, sql: &str, timeout_ms: i64) -> Result<(String, bool), Error> {
    let body = sql.trim().trim_end_matches(';').trim();
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION READ ONLY").execute(&mut *tx).await?;
    sqlx::query("SELECT set_config('statement_timeout', $1, true)")
        .bind(timeout_ms.to_string())
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL search_path = explorer, pg_catalog").execute(&mut *tx).await?;
    // The admin's text is the subquery; it is run as the restricted login role, read only, and
    // rolled back, which is what makes running it safe (Data Explorer).
    let wrapped = format!(
        "SELECT coalesce(json_agg(q)::text, '[]'), count(*) > {MAX_ROWS}
         FROM (SELECT * FROM ({body}\n) explorer_query LIMIT {}) q",
        MAX_ROWS + 1
    );
    // The admin's query, run as bf_explorer, read only and rolled back (see `run`'s doc).
    let (rows, more): (String, bool) = sqlx::query_as(sqlx::AssertSqlSafe(wrapped)).fetch_one(&mut *tx).await?;
    tx.rollback().await?;
    if more {
        // Drop the extra row that only showed there were more.
        return Ok((preserve_order_json(&rows, MAX_ROWS as usize), true));
    }
    Ok((rows, false))
}

/// The first `n` elements of a JSON array, cut from the text so each object keeps its key order.
fn preserve_order_json(array: &str, n: usize) -> String {
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escaped = false;
    let mut count = 0;
    for (i, c) in array.char_indices() {
        if in_str {
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => in_str = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '[' | '{' => depth += 1,
            ']' | '}' => {
                depth -= 1;
                if depth == 1 {
                    count += 1;
                    if count == n {
                        return format!("{}]", &array[..=i]);
                    }
                }
            }
            _ => {}
        }
    }
    array.to_owned()
}

pub async fn saved(pool: &PgPool) -> Result<Vec<SavedQuery>, Error> {
    sqlx::query_as("SELECT id, name, description, sql, updated_at FROM explorer_queries ORDER BY name")
        .fetch_all(pool)
        .await
}

pub async fn save(pool: &PgPool, id: Option<i64>, q: &SavedQuery, actor: Option<i64>) -> Result<Option<i64>, Error> {
    match id {
        None => sqlx::query_scalar(
            "INSERT INTO explorer_queries (name, description, sql, created_by) VALUES ($1, $2, $3, $4) RETURNING id",
        )
        .bind(&q.name)
        .bind(&q.description)
        .bind(&q.sql)
        .bind(actor)
        .fetch_optional(pool)
        .await,
        Some(id) => {
            sqlx::query_scalar(
                "UPDATE explorer_queries SET name = $2, description = $3, sql = $4, updated_at = now() WHERE id = $1
             RETURNING id",
            )
            .bind(id)
            .bind(&q.name)
            .bind(&q.description)
            .bind(&q.sql)
            .fetch_optional(pool)
            .await
        }
    }
}

crate::sql::delete_by_id!(delete, "explorer_queries");

#[cfg(test)]
mod tests {
    use super::preserve_order_json;

    #[test]
    fn cuts_a_json_array_without_reordering() {
        let a = r#"[{"b":1,"a":"x]}"},{"b":2,"a":"\"{"},{"b":3,"a":null}]"#;
        assert_eq!(preserve_order_json(a, 2), r#"[{"b":1,"a":"x]}"},{"b":2,"a":"\"{"}]"#);
        assert_eq!(preserve_order_json(a, 5), a);
    }
}
