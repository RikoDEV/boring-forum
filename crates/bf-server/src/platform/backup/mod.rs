//! Site backups: one `.tar.gz` holding the database and, when asked, the uploaded files.
//!
//! ```text
//! boring-forum-2026-11-16-031500-v1.tar.gz
//!   manifest.json            version, schema version, tables, row counts
//!   db/<table>.copy          each table as PostgreSQL COPY text, from one consistent snapshot
//!   uploads/<path>           the uploaded files
//! ```
//!
//! The schema is not in the archive: the server's own migrations are its source of truth, so a
//! restore loads the data into a database the server has already migrated. A backup from an
//! older version loads into a newer one when the newer schema only added things.
//!
//! The archive holds every password hash, email address and private message. It is written with
//! owner-only permissions, and downloading one needs a single-use link emailed to the
//! administrator.

mod tar;

use bf_db::platform::backup::{self as db, Table, TableFile};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

/// The layout of the archive. Bumped when a restore of an older one would no longer work.
pub const FORMAT: u32 = 1;

const PREFIX: &str = "boring-forum-";
const SUFFIX: &str = ".tar.gz";

/// What went wrong, in words an administrator can act on.
#[derive(Debug)]
pub struct BackupError(pub String);

impl fmt::Display for BackupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BackupError {}

impl From<sqlx::Error> for BackupError {
    fn from(e: sqlx::Error) -> Self {
        Self(format!("database: {e}"))
    }
}

impl From<io::Error> for BackupError {
    fn from(e: io::Error) -> Self {
        Self(e.to_string())
    }
}

impl From<serde_json::Error> for BackupError {
    fn from(e: serde_json::Error) -> Self {
        Self(format!("the manifest is not valid: {e}"))
    }
}

type Result<T> = std::result::Result<T, BackupError>;

fn fail<T>(msg: impl Into<String>) -> Result<T> {
    Err(BackupError(msg.into()))
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TableEntry {
    pub name: String,
    pub columns: Vec<String>,
    pub rows: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Manifest {
    pub format: u32,
    pub app_version: String,
    pub schema_version: i64,
    pub created_at: String,
    /// `manual`, `automatic` or `command line`.
    pub trigger: String,
    pub uploads: bool,
    pub tables: Vec<TableEntry>,
    /// Each sequence: name, last value, and whether that value was handed out.
    pub sequences: Vec<(String, i64, bool)>,
}

/// A backup on disk.
#[derive(Clone, Debug, Serialize)]
pub struct Backup {
    pub name: String,
    pub size: u64,
    pub manifest: Manifest,
}

/// Where backups are kept: `BF_BACKUP_DIR`, `data/backups` by default.
pub fn dir() -> PathBuf {
    std::env::var("BF_BACKUP_DIR").ok().filter(|d| !d.is_empty()).unwrap_or_else(|| "data/backups".into()).into()
}

/// Where uploaded files are kept: `BF_UPLOAD_DIR`.
pub fn uploads_dir() -> PathBuf {
    std::env::var("BF_UPLOAD_DIR").ok().filter(|d| !d.is_empty()).unwrap_or_else(|| "data/uploads".into()).into()
}

/// Whether `name` is the name of a backup file: no path, nothing that could leave the folder.
pub fn valid_name(name: &str) -> bool {
    name.starts_with(PREFIX)
        && name.ends_with(SUFFIX)
        && name.len() <= 120
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        && !name.contains("..")
}

// --- What is running -------------------------------------------------------------------------

/// What the server is doing about backups right now, for the admin page.
#[derive(Clone, Debug, Serialize, Default)]
pub struct Activity {
    /// `backup` or `restore` while one runs.
    pub running: Option<&'static str>,
    /// A short code (`database`, `packing`, `unpacking`, `loading`, `uploads`); the page words it.
    pub phase: String,
    pub started_at: Option<String>,
    /// How the last one ended: an error message, or `None` when it worked.
    pub last_error: Option<String>,
    pub last_finished_at: Option<String>,
}

static ACTIVITY: Mutex<Option<Activity>> = Mutex::new(None);

fn lock() -> std::sync::MutexGuard<'static, Option<Activity>> {
    ACTIVITY.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub fn activity() -> Activity {
    lock().clone().unwrap_or_default()
}

fn phase(text: &str) {
    if let Some(a) = lock().as_mut() {
        text.clone_into(&mut a.phase);
    }
}

/// Held while a backup or a restore runs: only one at a time, and the admin page can see it.
pub struct Running(());

impl Running {
    pub fn start(kind: &'static str) -> Option<Self> {
        let mut g = lock();
        let a = g.get_or_insert_with(Activity::default);
        if a.running.is_some() {
            return None;
        }
        a.running = Some(kind);
        a.phase = String::new();
        a.started_at = Some(chrono::Utc::now().to_rfc3339());
        Some(Self(()))
    }

    /// Records how it ended.
    pub fn finish(self, outcome: &Result<impl Sized>) {
        if let Some(a) = lock().as_mut() {
            a.last_error = outcome.as_ref().err().map(ToString::to_string);
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(a) = lock().as_mut() {
            a.running = None;
            a.phase = String::new();
            a.last_finished_at = Some(chrono::Utc::now().to_rfc3339());
        }
    }
}

// --- Listing and removing --------------------------------------------------------------------

fn read_manifest_file(path: &Path) -> Result<Manifest> {
    let mut r = tar::Reader::new(GzDecoder::new(BufReader::new(File::open(path)?)));
    let Some(first) = r.next_entry()? else { return fail("the file is empty") };
    if first.name != "manifest.json" || !first.is_file || first.size > 64 * 1024 * 1024 {
        return fail("the file is not a Boring Forum backup");
    }
    let mut bytes = Vec::new();
    r.read_body(first.size, &mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}

/// The backups in `dir`, newest first. Files that are not backups are left out.
pub fn list(dir: &Path) -> Vec<Backup> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<Backup> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok().filter(|n| valid_name(n))?;
            let size = e.metadata().ok().filter(std::fs::Metadata::is_file)?.len();
            let manifest = read_manifest_file(&e.path()).ok()?;
            Some(Backup { name, size, manifest })
        })
        .collect();
    out.sort_by(|a, b| b.manifest.created_at.cmp(&a.manifest.created_at));
    out
}

pub fn delete(dir: &Path, name: &str) -> Result<()> {
    if !valid_name(name) {
        return fail("that is not a backup");
    }
    fs::remove_file(dir.join(name))?;
    Ok(())
}

/// Deletes the oldest backups beyond `keep`.
pub fn prune(dir: &Path, keep: usize) {
    for old in list(dir).into_iter().skip(keep.max(1)) {
        if let Err(e) = delete(dir, &old.name) {
            tracing::warn!(error = %e, backup = %old.name, "old backup not deleted");
        }
    }
}

/// Opens a backup for download or restore after checking its name.
pub fn path_of(dir: &Path, name: &str) -> Result<PathBuf> {
    if !valid_name(name) {
        return fail("that is not a backup");
    }
    let p = dir.join(name);
    if !p.is_file() {
        return fail("there is no such backup");
    }
    Ok(p)
}

// --- Writing files only the owner can read ---------------------------------------------------

fn private_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().recursive(true).mode(0o700).create(path)
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(path)
    }
}

fn private_file(path: &Path) -> io::Result<File> {
    let mut o = fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)
}

/// Removes a work folder when dropped, whatever happened.
struct Cleanup(PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

// --- Backing up ------------------------------------------------------------------------------

/// Takes a backup into `dir`. `uploads` is the uploaded files' folder, or `None` to leave them out.
pub async fn create(pool: &PgPool, dir: &Path, uploads: Option<&Path>, trigger: &str) -> Result<Backup> {
    private_dir(dir)?;
    let now = chrono::Utc::now();
    let schema_version = db::schema_version(pool).await?;
    let name = format!("{PREFIX}{}-v{schema_version}{SUFFIX}", now.format("%Y-%m-%d-%H%M%S"));
    let work = dir.join(format!(".work-{}", now.format("%Y%m%d%H%M%S%3f")));
    private_dir(&work)?;
    let _cleanup = Cleanup(work.clone());

    phase("database");
    let mut tx = db::begin_snapshot(pool).await?;
    let tables: Vec<Table> = db::tables(&mut tx).await?;
    let sequences = db::sequences(&mut tx).await?;
    let mut entries = Vec::with_capacity(tables.len());
    for t in &tables {
        let mut file = tokio::fs::File::from_std(private_file(&db::table_path(&work, &t.name))?);
        let rows = db::copy_out(&mut tx, t, &mut file).await?;
        entries.push(TableEntry { name: t.name.clone(), columns: t.columns.clone(), rows });
    }
    // Nothing was written: the snapshot is only read from.
    tx.rollback().await?;

    let manifest = Manifest {
        format: FORMAT,
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        schema_version,
        created_at: now.to_rfc3339(),
        trigger: trigger.to_owned(),
        uploads: uploads.is_some(),
        tables: entries,
        sequences,
    };

    phase("packing");
    let final_path = dir.join(&name);
    let partial = dir.join(format!("{name}.partial"));
    let packed = {
        let (work, partial, manifest, uploads) = (work.clone(), partial.clone(), manifest.clone(), uploads.map(Path::to_path_buf));
        tokio::task::spawn_blocking(move || pack(&work, &partial, &manifest, uploads.as_deref()))
            .await
            .map_err(|e| BackupError(e.to_string()))?
    };
    if let Err(e) = packed {
        let _ = fs::remove_file(&partial);
        return Err(e);
    }
    fs::rename(&partial, &final_path)?;
    let size = fs::metadata(&final_path)?.len();
    Ok(Backup { name, size, manifest })
}

fn pack(work: &Path, partial: &Path, manifest: &Manifest, uploads: Option<&Path>) -> Result<()> {
    let mtime = u64::try_from(chrono::Utc::now().timestamp()).unwrap_or(0);
    let out = BufWriter::new(private_file(partial)?);
    let mut w = tar::Writer::new(GzEncoder::new(out, Compression::default()));
    w.append_bytes("manifest.json", &serde_json::to_vec_pretty(manifest)?, mtime)?;
    for t in &manifest.tables {
        let path = db::table_path(work, &t.name);
        let file = File::open(&path)?;
        let size = file.metadata()?.len();
        w.append(&db::entry_name(&t.name), size, mtime, BufReader::new(file))?;
    }
    if let Some(root) = uploads {
        add_uploads(&mut w, root, root, mtime)?;
    }
    let mut out = w.finish()?.finish()?;
    out.flush()?;
    out.into_inner().map_err(|e| BackupError(e.to_string()))?.sync_all()?;
    Ok(())
}

fn add_uploads<W: Write>(w: &mut tar::Writer<W>, root: &Path, dir: &Path, mtime: u64) -> Result<()> {
    let Ok(read) = fs::read_dir(dir) else { return Ok(()) };
    for entry in read.flatten() {
        let path = entry.path();
        // Links are not followed: a backup holds files, not what they point at.
        let Ok(meta) = fs::symlink_metadata(&path) else { continue };
        if meta.is_dir() {
            add_uploads(w, root, &path, mtime)?;
        } else if meta.is_file() {
            let Ok(rel) = path.strip_prefix(root) else { continue };
            let Some(rel) = rel.to_str().map(|s| s.replace('\\', "/")) else { continue };
            let file = match File::open(&path) {
                Ok(f) => f,
                // Deleted since the listing: not part of this backup.
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            w.append(&format!("uploads/{rel}"), meta.len(), mtime, BufReader::new(file))?;
        }
    }
    Ok(())
}

// --- Restoring -------------------------------------------------------------------------------

/// A name inside the archive's `uploads/` folder, as a relative path that stays inside it.
fn safe_relative(rel: &str) -> Option<PathBuf> {
    if rel.is_empty() || rel.len() > 400 || rel.contains(['\\', '\0', ':']) {
        return None;
    }
    let path = Path::new(rel);
    path.components().all(|c| matches!(c, Component::Normal(_))).then(|| path.to_path_buf())
}

/// Unpacks an archive into `work`: the table files under `db/`, the uploads under `uploads/`.
fn unpack(archive: &Path, work: &Path) -> Result<Manifest> {
    let mut r = tar::Reader::new(GzDecoder::new(BufReader::new(File::open(archive)?)));
    let Some(first) = r.next_entry()? else { return fail("the file is empty") };
    if first.name != "manifest.json" || !first.is_file || first.size > 64 * 1024 * 1024 {
        return fail("the file is not a Boring Forum backup");
    }
    let mut bytes = Vec::new();
    r.read_body(first.size, &mut bytes)?;
    let manifest: Manifest = serde_json::from_slice(&bytes)?;
    if manifest.format != FORMAT {
        return fail(format!("this backup uses format {}, which this version cannot read", manifest.format));
    }
    for t in &manifest.tables {
        if !db::safe_table_name(&t.name) {
            return fail("the backup lists a table with an unusable name");
        }
    }
    private_dir(&work.join("db"))?;
    private_dir(&work.join("uploads"))?;
    while let Some(entry) = r.next_entry()? {
        if !entry.is_file {
            continue;
        }
        let target = if let Some(table) = entry.name.strip_prefix("db/").and_then(|n| n.strip_suffix(".copy")) {
            if !manifest.tables.iter().any(|t| t.name == table) {
                continue;
            }
            db::table_path(&work.join("db"), table)
        } else if let Some(rel) = entry.name.strip_prefix("uploads/") {
            let Some(rel) = safe_relative(rel) else { continue };
            let target = work.join("uploads").join(rel);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            target
        } else {
            continue;
        };
        let mut out = BufWriter::new(File::create(&target)?);
        r.read_body(entry.size, &mut out)?;
        out.flush()?;
    }
    Ok(manifest)
}

/// Moves the unpacked uploads into the live folder. A file that is already there is the same
/// file (names are content hashes), so it is left alone.
fn place_uploads(from: &Path, to: &Path) -> Result<()> {
    let Ok(read) = fs::read_dir(from) else { return Ok(()) };
    for entry in read.flatten() {
        let src = entry.path();
        let dest = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            fs::create_dir_all(&dest)?;
            place_uploads(&src, &dest)?;
        } else if !dest.exists() {
            // A move when both are on one disk; a copy when the work folder is elsewhere.
            if fs::rename(&src, &dest).is_err() {
                fs::copy(&src, &dest)?;
            }
        }
    }
    Ok(())
}

/// Replaces the database (and adds the uploads) with the contents of a backup. `pool` must be
/// allowed to truncate and rewrite every table: the migration role when one is configured.
pub async fn restore(pool: &PgPool, archive: &Path, uploads_to: &Path) -> Result<Manifest> {
    let parent = archive.parent().unwrap_or_else(|| Path::new("."));
    let work = parent.join(format!(".restore-{}", chrono::Utc::now().format("%Y%m%d%H%M%S%3f")));
    private_dir(&work)?;
    let _cleanup = Cleanup(work.clone());

    phase("unpacking");
    let manifest = {
        let (archive, work) = (archive.to_path_buf(), work.clone());
        tokio::task::spawn_blocking(move || unpack(&archive, &work)).await.map_err(|e| BackupError(e.to_string()))??
    };
    let current = db::schema_version(pool).await?;
    if manifest.schema_version > current {
        return fail(format!(
            "this backup is from a newer version of the forum (schema {}, this one is {current}); update the server first",
            manifest.schema_version
        ));
    }

    phase("loading");
    let files: Vec<TableFile> = manifest
        .tables
        .iter()
        .map(|t| TableFile {
            table: Table { name: t.name.clone(), columns: t.columns.clone() },
            path: db::table_path(&work.join("db"), &t.name),
        })
        .filter(|f| f.path.is_file())
        .collect();
    db::restore(pool, &files, &manifest.sequences).await?;

    if manifest.uploads {
        phase("uploads");
        private_dir(uploads_to)?;
        let (from, to) = (work.join("uploads"), uploads_to.to_path_buf());
        tokio::task::spawn_blocking(move || place_uploads(&from, &to)).await.map_err(|e| BackupError(e.to_string()))??;
    }
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each table's row count and a hash of all its rows, to compare a database before and after.
    async fn fingerprint(pool: &PgPool) -> Vec<(String, i64, String)> {
        let mut conn = pool.acquire().await.expect("connection");
        let tables = db::tables(&mut conn).await.expect("tables");
        let mut out = Vec::new();
        for t in tables {
            let sql = format!(
                "SELECT count(*), coalesce(md5(string_agg(x::text, '' ORDER BY x::text)), '') FROM public.\"{}\" x",
                t.name
            );
            let (n, h): (i64, String) =
                sqlx::query_as(sqlx::AssertSqlSafe(sql)).fetch_one(&mut *conn).await.expect("fingerprint");
            out.push((t.name, n, h));
        }
        out
    }

    /// A backup of a real database restores to exactly the same rows, and a restore undoes
    /// changes made since. Needs `BF_DATABASE_URL` to name an empty, disposable database owned
    /// by a superuser: `cargo test -p bf-server backup -- --ignored`.
    #[tokio::test]
    #[ignore = "needs a database"]
    async fn a_backup_restores_to_the_same_rows() {
        let url = std::env::var("BF_DATABASE_URL").expect("BF_DATABASE_URL");
        let pool = bf_db::connect(&url, 2).await.expect("connect");
        bf_db::MIGRATOR.run(&pool).await.expect("migrate");

        let root = std::env::temp_dir().join(format!("bf-backup-test-{}", std::process::id()));
        let uploads = root.join("uploads");
        fs::create_dir_all(uploads.join("ab")).expect("uploads");
        fs::write(uploads.join("ab").join("abcdef.bin"), b"picture bytes").expect("file");

        // Something besides the seeded rows: a setting, and a row that references another table.
        sqlx::query("INSERT INTO site_settings (key, value) VALUES ('backup_test', '\"before\"') ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value")
            .execute(&pool).await.expect("setting");
        // Rows that point at each other (a member and the upload that is their avatar) can only be
        // loaded with the foreign keys checked at the end.
        sqlx::query("INSERT INTO users (id, username, username_lower, email, email_lower, username_skeleton) OVERRIDING SYSTEM VALUE VALUES (900001, 'bt', 'bt', 'bt@example.com', 'bt@example.com', 'bt') ON CONFLICT DO NOTHING")
            .execute(&pool).await.expect("user");
        sqlx::query("INSERT INTO uploads (id, sha256, storage_key, filename, mime, size, user_id) OVERRIDING SYSTEM VALUE VALUES (900001, 'aa', 'bt/aa', 'a.png', 'image/png', 1, 900001) ON CONFLICT DO NOTHING")
            .execute(&pool).await.expect("upload");
        sqlx::query("UPDATE users SET avatar_upload_id = 900001 WHERE id = 900001").execute(&pool).await.expect("avatar");
        let before = fingerprint(&pool).await;
        assert!(before.iter().map(|t| t.1).sum::<i64>() > 100, "the seeded rows are there");

        let made = create(&pool, &root, Some(&uploads), "test").await.expect("backup");
        assert!(valid_name(&made.name));
        assert_eq!(list(&root).len(), 1);

        // Change things after the backup.
        sqlx::query("UPDATE site_settings SET value = '\"after\"' WHERE key = 'backup_test'").execute(&pool).await.expect("update");
        sqlx::query("DELETE FROM report_reasons").execute(&pool).await.expect("delete");
        sqlx::query("DELETE FROM badges").execute(&pool).await.expect("delete");
        fs::remove_file(uploads.join("ab").join("abcdef.bin")).expect("remove");
        assert_ne!(fingerprint(&pool).await, before);

        restore(&pool, &root.join(&made.name), &uploads).await.expect("restore");
        assert_eq!(fingerprint(&pool).await, before, "the same rows come back");
        assert_eq!(fs::read(uploads.join("ab").join("abcdef.bin")).expect("restored file"), b"picture bytes");
        // A new group still gets an id clear of the built-in ones (the sequence was set, not used).
        let next: i64 = sqlx::query_scalar("SELECT nextval('groups_id_seq')").fetch_one(&pool).await.expect("nextval");
        assert!(next >= 100, "sequences come back where they were: {next}");

        // A restore that fails leaves the database as it was.
        let mut broken = made.clone();
        broken.manifest.tables.push(TableEntry { name: "no_such_table".into(), columns: vec!["x".into()], rows: 0 });
        sqlx::query("UPDATE site_settings SET value = '\"kept\"' WHERE key = 'backup_test'").execute(&pool).await.expect("update");
        let kept = fingerprint(&pool).await;
        let file = TableFile { table: Table { name: "site_settings".into(), columns: vec!["no_such_column".into()] }, path: root.join("none") };
        assert!(db::restore(&pool, &[file], &[]).await.is_err());
        assert_eq!(fingerprint(&pool).await, kept, "a failed restore changes nothing");

        let _ = fs::remove_dir_all(&root);
    }
}
