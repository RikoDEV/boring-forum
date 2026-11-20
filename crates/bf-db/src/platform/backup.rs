//! The database half of a site backup: every table of the `public` schema as PostgreSQL `COPY`
//! text, read from one consistent snapshot, and the way back in.
//!
//! The schema itself is not saved: the migrations in the binary are its source of truth, so a
//! backup restores into a database the server has already migrated. A backup from an older
//! schema loads into a newer one as long as the newer one only added columns and tables.

use futures_util::StreamExt;
use sqlx::{PgConnection, PgPool, Row};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt};

type Error = sqlx::Error;

/// A table and the columns a copy of it carries (generated columns are rebuilt, not copied).
#[derive(Clone, Debug)]
pub struct Table {
    pub name: String,
    pub columns: Vec<String>,
}

/// One table's file in a restore.
pub struct TableFile {
    pub table: Table,
    pub path: PathBuf,
}

/// The newest migration the database has applied.
pub async fn schema_version(pool: &PgPool) -> Result<i64, Error> {
    let v: Option<i64> =
        sqlx::query_scalar("SELECT max(version) FROM _sqlx_migrations WHERE success").fetch_one(pool).await?;
    Ok(v.unwrap_or(0))
}

fn quote(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

fn io_error(e: std::io::Error) -> Error {
    Error::Io(e)
}

/// The tables a backup holds: everything in `public` except the migration ledger.
pub async fn tables(conn: &mut PgConnection) -> Result<Vec<Table>, Error> {
    let rows = sqlx::query(
        "SELECT t.tablename,
                coalesce((SELECT array_agg(c.column_name::text ORDER BY c.ordinal_position)
                          FROM information_schema.columns c
                          WHERE c.table_schema = 'public' AND c.table_name = t.tablename
                            AND c.is_generated = 'NEVER'), '{}')
         FROM pg_tables t
         WHERE t.schemaname = 'public' AND t.tablename <> '_sqlx_migrations'
         ORDER BY t.tablename",
    )
    .fetch_all(&mut *conn)
    .await?;
    rows.iter().map(|r| Ok(Table { name: r.try_get(0)?, columns: r.try_get(1)? })).collect()
}

/// Every sequence with where it stands: its last value and whether that value was handed out.
/// (A sequence set with `setval(n, false)`, like the one that keeps new groups clear of the
/// built-in ones, has not handed anything out yet but must come back at `n`.)
pub async fn sequences(conn: &mut PgConnection) -> Result<Vec<(String, i64, bool)>, Error> {
    let names: Vec<String> = sqlx::query_scalar(
        "SELECT sequencename::text FROM pg_sequences WHERE schemaname = 'public' ORDER BY sequencename",
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        // A name from the database's own catalog, quoted as an identifier.
        let sql = format!("SELECT last_value, is_called FROM public.{}", quote(&name));
        let (last, called): (i64, bool) = sqlx::query_as(sqlx::AssertSqlSafe(sql)).fetch_one(&mut *conn).await?;
        out.push((name, last, called));
    }
    Ok(out)
}

/// Starts the read-only snapshot every table is read from. Call before `tables`.
pub async fn begin_snapshot(pool: &PgPool) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY").execute(&mut *tx).await?;
    Ok(tx)
}

/// Writes one table's rows to `out` as COPY text. Returns the number of rows.
pub async fn copy_out(
    conn: &mut PgConnection,
    table: &Table,
    out: &mut (impl AsyncWrite + Unpin),
) -> Result<u64, Error> {
    let columns = table.columns.iter().map(|c| quote(c)).collect::<Vec<_>>().join(", ");
    // The names come from the database's own catalog and are quoted as identifiers.
    let sql = format!("COPY public.{} ({columns}) TO STDOUT", quote(&table.name));
    let mut stream = conn.copy_out_raw(&sql).await?;
    let mut rows = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        // In COPY text a newline inside a value is written as `\n`, so each line is one row.
        rows += chunk.iter().filter(|b| **b == b'\n').count() as u64;
        out.write_all(&chunk).await.map_err(io_error)?;
    }
    out.flush().await.map_err(io_error)?;
    Ok(rows)
}

/// Replaces the contents of the database with the files, in one transaction: either the whole
/// backup is loaded or nothing changes. Tables the database no longer has are skipped; tables
/// the backup lacks are left empty. Foreign keys are checked at the end, so load order does not
/// matter. Needs to own the tables (the migration role, when there is one).
pub async fn restore(pool: &PgPool, files: &[TableFile], sequences: &[(String, i64, bool)]) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET CONSTRAINTS ALL DEFERRED").execute(&mut *tx).await?;
    let current = tables(&mut tx).await?;

    // Triggers that keep derived tables in step (upload references, for one) would run for each
    // copied row; the backup already holds what they would write.
    let triggered: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT c.relname::text FROM pg_trigger t
         JOIN pg_class c ON c.oid = t.tgrelid JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE NOT t.tgisinternal AND n.nspname = 'public'",
    )
    .fetch_all(&mut *tx)
    .await?;
    for t in &triggered {
        // Identifiers from the catalog, quoted.
        sqlx::query(sqlx::AssertSqlSafe(format!("ALTER TABLE public.{} DISABLE TRIGGER USER", quote(t))))
            .execute(&mut *tx)
            .await?;
    }

    if !current.is_empty() {
        let all = current.iter().map(|t| format!("public.{}", quote(&t.name))).collect::<Vec<_>>().join(", ");
        // Identifiers from the catalog, quoted.
        sqlx::query(sqlx::AssertSqlSafe(format!("TRUNCATE TABLE {all}"))).execute(&mut *tx).await?;
    }

    for f in files {
        let Some(live) = current.iter().find(|t| t.name == f.table.name) else { continue };
        if let Some(missing) = f.table.columns.iter().find(|c| !live.columns.contains(c)) {
            return Err(Error::Protocol(format!(
                "the backup has a column {}.{missing} that this version of the forum no longer has",
                f.table.name
            )));
        }
        let columns = f.table.columns.iter().map(|c| quote(c)).collect::<Vec<_>>().join(", ");
        // Identifiers from the catalog (the backup's own list is checked against it just above).
        let sql = format!("COPY public.{} ({columns}) FROM STDIN", quote(&f.table.name));
        let mut copy = tx.copy_in_raw(&sql).await?;
        let mut file = tokio::fs::File::open(&f.path).await.map_err(io_error)?;
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            let n = file.read(&mut buf).await.map_err(io_error)?;
            if n == 0 {
                break;
            }
            copy.send(&buf[..n]).await?;
        }
        copy.finish().await?;
    }

    for (name, value, called) in sequences {
        // A sequence the new schema no longer has is not an error.
        sqlx::query("SELECT setval(to_regclass($1), $2, $3) WHERE to_regclass($1) IS NOT NULL")
            .bind(format!("public.{}", quote(name)))
            .bind(value)
            .bind(called)
            .execute(&mut *tx)
            .await?;
    }

    // Every row is in: check the foreign keys now (a table cannot be altered while checks are
    // still waiting), then put the triggers back.
    sqlx::query("SET CONSTRAINTS ALL IMMEDIATE").execute(&mut *tx).await?;
    for t in &triggered {
        // Identifiers from the catalog, quoted.
        sqlx::query(sqlx::AssertSqlSafe(format!("ALTER TABLE public.{} ENABLE TRIGGER USER", quote(t))))
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await
}

/// Where a table's copy lives inside a backup.
pub fn entry_name(table: &str) -> String {
    format!("db/{table}.copy")
}

/// Whether a name from a backup file can be a table name: nothing that could leave a folder.
pub fn safe_table_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// A file under `dir` for a table.
pub fn table_path(dir: &Path, table: &str) -> PathBuf {
    dir.join(format!("{table}.copy"))
}
