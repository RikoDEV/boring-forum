//! The least-privilege database roles (`deploy/postgres/roles.sql`), applied from inside the server
//! binary so a Docker Compose install needs no extra file and no `psql`: `boring-forum
//! provision-roles` connects as the database owner and runs the script once on every start.
//! Running it again is safe, because every statement in it is idempotent.

use sqlx::Executor;

/// The script `psql` users run by hand, embedded so the binary and the file cannot drift apart.
const SCRIPT: &str = include_str!("../../../deploy/postgres/roles.sql");

/// One SQL string literal: quotes doubled, which is all a standard-conforming string needs.
fn literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// The script with psql's own syntax made plain SQL: its `\set` lines dropped and its three
/// `:variable` passwords replaced by literals.
pub fn script(app: &str, migrate: &str, readonly: &str) -> String {
    let body: String = SCRIPT.lines().filter(|l| !l.trim_start().starts_with('\\')).collect::<Vec<_>>().join("\n");
    body.replace(":app_password", &literal(app))
        .replace(":migrate_password", &literal(migrate))
        .replace(":readonly_password", &literal(readonly))
}

/// Creates or updates the roles. `url` is a connection as the database's owner.
pub async fn provision(url: &str, app: &str, migrate: &str, readonly: &str) -> Result<(), sqlx::Error> {
    let mut conn = <sqlx::PgConnection as sqlx::Connection>::connect(url).await?;
    // The three passwords are the operator's own configuration, never request data, and
    // `literal` quotes them, so this string cannot carry anything an attacker chose.
    conn.execute(sqlx::raw_sql(sqlx::AssertSqlSafe(script(app, migrate, readonly)))).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::script;

    #[test]
    fn the_script_is_plain_sql_with_the_passwords_in_place() {
        let sql = script("a'p", "m", "r");
        assert!(!sql.lines().any(|l| l.trim_start().starts_with('\\')), "a psql meta-command is left");
        assert!(
            !sql.contains(":app_password") && !sql.contains(":migrate_password") && !sql.contains(":readonly_password")
        );
        assert!(sql.contains("ALTER ROLE forum_app      PASSWORD 'a''p';"), "quotes are doubled");
        assert!(sql.contains("PASSWORD 'm';") && sql.contains("PASSWORD 'r';"));
    }
}
