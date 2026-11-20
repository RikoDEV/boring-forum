//! Database access: connection pool, embedded migrations and query functions.
//!
//! Most queries use the runtime API, so `cargo build` needs no database; the ones
//! in `platform/idempotency.rs` use the checked `sqlx::query!` macros against `.sqlx/`, and more
//! move over as they are touched.

use sqlx::postgres::{PgPool, PgPoolOptions};
use std::time::Duration;

pub use sqlx::Error;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Caps how long one statement may run, so a pathological query cannot hold a connection for
/// ever (time budgets, statement timeouts). Generous enough for the heaviest page
/// query and for migrations, which run through their own short-lived pool.
const STATEMENT_TIMEOUT_MS: u32 = 15_000;

/// A pool that may change the schema: the migration role when one is configured (the application
/// role has no DDL rights then), otherwise the application's own pool.
pub async fn ddl_pool(app: &PgPool) -> Result<PgPool, Error> {
    match std::env::var("BF_MIGRATE_DATABASE_URL").ok().filter(|u| !u.is_empty()) {
        Some(url) => connect(&url, 1).await,
        None => Ok(app.clone()),
    }
}

/// The pool the Data Explorer runs untrusted admin SQL on, from `BF_EXPLORER_DATABASE_URL`.
///
/// This must authenticate *as* the restricted `bf_explorer` login role (see
/// `deploy/postgres/roles.sql`), never as the application role. Running the query on a
/// connection whose login role is `bf_explorer` is what confines it: a submitted query can
/// `RESET ROLE` or call a SQL-executing built-in, but either way it only ever reaches
/// `bf_explorer`'s own rights (the `explorer.*` views). An in-session `SET ROLE` on the
/// application's connection does not confine it, because the submitted SQL can switch straight
/// back to the privileged application role.
///
/// Returns `None` when unset: the Explorer is then disabled rather than run with weaker
/// isolation (fail closed).
pub async fn explorer_pool() -> Result<Option<PgPool>, Error> {
    match std::env::var("BF_EXPLORER_DATABASE_URL").ok().filter(|u| !u.is_empty()) {
        Some(url) => connect(&url, 2).await.map(Some),
        None => Ok(None),
    }
}

/// Whether this connection's login role is a Postgres superuser. The application must not run as
/// one: a SQL-injection or a Data Explorer escape would then mean code execution on the database
/// host (`COPY ... PROGRAM`, untrusted PL, `pg_read_file`). Deploy with the least-privilege roles
/// in `deploy/postgres/roles.sql`.
pub async fn is_superuser(pool: &PgPool) -> Result<bool, Error> {
    sqlx::query_scalar::<_, bool>("SELECT current_setting('is_superuser') = 'on'").fetch_one(pool).await
}

pub async fn connect(url: &str, max_connections: u32) -> Result<PgPool, Error> {
    let timeout = std::env::var("BF_STATEMENT_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(STATEMENT_TIMEOUT_MS);
    PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(Duration::from_secs(5))
        // Set per connection rather than per role, so the limits hold however the database user
        // was provisioned. A transaction left open by a panicking task is rolled back too.
        .after_connect(move |conn, _| {
            Box::pin(async move {
                // One `SET` per call: Postgres refuses several commands in a single prepared
                // statement, and a failure here makes every connection unusable.
                //
                // `SET` takes no bind parameters; `timeout` is a u32, so there is nothing to
                // inject through it.
                sqlx::query(sqlx::AssertSqlSafe(format!("SET statement_timeout = {timeout}")))
                    .execute(&mut *conn)
                    .await?;
                // Rolls back a transaction left open by a task that stopped making progress,
                // instead of holding its locks for ever.
                sqlx::query("SET idle_in_transaction_session_timeout = 30000").execute(conn).await.map(|_| ())
            })
        })
        .connect(url)
        .await
}

pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    MIGRATOR.run(pool).await
}

/// Whether every migration this binary embeds has been applied (readiness).
pub async fn migrations_current(pool: &PgPool) -> Result<bool, Error> {
    let latest = MIGRATOR.iter().map(|m| m.version).max().unwrap_or(0);
    let applied: Option<i64> =
        sqlx::query_scalar("SELECT max(version) FROM _sqlx_migrations WHERE success").fetch_one(pool).await?;
    Ok(applied.unwrap_or(0) >= latest)
}

pub async fn ping(pool: &PgPool) -> Result<(), Error> {
    sqlx::query("SELECT 1").execute(pool).await.map(|_| ())
}

// Query functions, by domain. Each module covers one concern.
mod sql;
pub mod accounts;
pub mod admin;
pub mod ai;
pub mod community;
pub mod federation;
pub mod forum;
pub mod moderation;
pub mod platform;
pub mod settings;
