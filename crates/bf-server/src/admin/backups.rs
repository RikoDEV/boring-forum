//! Backups in the admin panel: list, back up now, download, restore, delete, and the schedule.
//! The archive format and the work itself are in `platform::backup`.

use crate::admin::api::{Admin, AdminRead, AdminWrite, audit_log};
use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::platform::backup::{self, Running};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bf_core::{AppError, auth};
use bf_db::platform::mail as outbox;
use serde::Deserialize;
use serde_json::json;
use tokio::io::AsyncReadExt;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/admin/backups", get(overview).post(start))
        .route("/api/v1/admin/backups/{name}", axum::routing::delete(remove))
        .route("/api/v1/admin/backups/{name}/download-link", post(send_download_link))
        .route("/backup-download/{token}", get(download))
        .route("/api/v1/admin/backups/{name}/restore", post(restore))
}

fn busy() -> AppError {
    AppError::Validation("A backup or restore is already running.".into())
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, AppError> {
    tokio::task::spawn_blocking(f).await.map_err(AppError::internal)
}

async fn overview(State(state): State<AppState>, Admin(..): AdminRead) -> Result<Response, AppError> {
    let pool = state.db()?;
    let dir = backup::dir();
    let current = bf_db::platform::backup::schema_version(pool).await.map_err(AppError::internal)?;
    let listed = blocking({
        let dir = dir.clone();
        move || backup::list(&dir)
    })
    .await?;
    let backups: Vec<_> = listed
        .iter()
        .map(|b| {
            json!({
                "name": b.name,
                "size": b.size,
                "created_at": b.manifest.created_at,
                "trigger": b.manifest.trigger,
                "app_version": b.manifest.app_version,
                "schema_version": b.manifest.schema_version,
                "uploads": b.manifest.uploads,
                "rows": b.manifest.tables.iter().map(|t| t.rows).sum::<u64>(),
                "restorable": b.manifest.schema_version <= current,
            })
        })
        .collect();
    Ok(Json(json!({ "backups": backups, "activity": backup::activity(), "directory": dir.display().to_string() }))
        .into_response())
}

#[derive(Deserialize, Default)]
struct StartBody {
    /// Leave the uploaded files out. Default: include them.
    #[serde(default)]
    skip_uploads: bool,
}

/// Starts a backup in the background; the page polls `overview` for how it is going.
async fn start(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    body: Option<Json<StartBody>>,
) -> Result<Response, AppError> {
    let skip = body.is_some_and(|b| b.skip_uploads);
    if !spawn_backup(&state, "manual", !skip, Some(who.unwrap_or(0))) {
        return Err(busy());
    }
    Ok((StatusCode::ACCEPTED, Json(json!({ "started": true }))).into_response())
}

/// Runs one backup in the background. `false` when another backup or restore is running.
pub(crate) fn spawn_backup(state: &AppState, trigger: &'static str, uploads: bool, who: Option<i64>) -> bool {
    let Some(pool) = state.pool.clone() else { return false };
    let Some(run) = Running::start("backup") else { return false };
    let keep = usize::try_from(state.settings().int("backup_keep")).unwrap_or(7);
    tokio::spawn(async move {
        let dir = backup::dir();
        let up = backup::uploads_dir();
        let result = backup::create(&pool, &dir, uploads.then_some(up.as_path()), trigger).await;
        match &result {
            Ok(b) => {
                let details = json!({ "name": b.name, "size": b.size, "trigger": trigger });
                if let Err(e) = audit_log(&pool, who, "backup.create", "site", details).await {
                    tracing::warn!(error = %e, "backup not logged");
                }
                let _ = blocking(move || backup::prune(&dir, keep)).await;
            }
            Err(e) => tracing::error!(error = %e, "backup failed"),
        }
        run.finish(&result);
    });
    true
}

async fn remove(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path(name): Path<String>,
) -> Result<Response, AppError> {
    let pool = state.db()?;
    backup::delete(&backup::dir(), &name).map_err(|_| AppError::NotFound)?;
    audit_log(pool, Some(who.unwrap_or(0)), "backup.delete", "site", json!({ "name": name })).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// How long the emailed link works.
const LINK_MINUTES: i32 = 15;

/// Emails the administrator a link to download a backup. The archive holds everybody's password
/// hashes and email addresses, so it is not served to a session alone: the link goes to the
/// address on the administrator's account, works once and expires.
async fn send_download_link(
    State(state): State<AppState>,
    Admin(who, v): AdminWrite,
    Path(name): Path<String>,
) -> Result<Response, AppError> {
    // The bootstrap token has no mailbox.
    let Some(user) = who.filter(|id| *id > 0) else { return Err(AppError::Forbidden) };
    backup::path_of(&backup::dir(), &name).map_err(|_| AppError::NotFound)?;
    if !crate::http::ratelimit::MAIL_BY_ACCOUNT.allow(&format!("backup:{user}")) {
        return Err(AppError::RateLimited { retry_after_secs: 600 });
    }
    let pool = state.db()?;
    let account = bf_db::accounts::security::account(pool, user.into()).await.db()?.ok_or(AppError::NotFound)?;
    let token = auth::random_token();
    bf_db::accounts::security::create_email_token(
        pool,
        &auth::token_digest(&token),
        account.id,
        "backup",
        Some(&name),
        LINK_MINUTES,
    )
    .await
    .db()?;
    let s = state.settings();
    let link = format!("{}/backup-download/{token}", state.config.base_url);
    let vars = [
        ("site", s.str("site_name")),
        ("username", account.username.as_str()),
        ("name", name.as_str()),
        ("link", link.as_str()),
    ];
    // In the administrator's language, as for any mail sent on a member's behalf.
    let (subject, body) = crate::admin::mail_templates::render(&bf_core::i18n::T::new(v.locale), "backup", &vars);
    outbox::queue_mail(pool, &account.email, &subject, &body).await.db()?;
    audit_log(pool, Some(user), "backup.link", "site", json!({ "name": name })).await?;
    Ok(Json(json!({ "sent_to": mask(&account.email) })).into_response())
}

/// `jane@example.com` as `j***@example.com`.
fn mask(email: &str) -> String {
    match email.split_once('@') {
        Some((local, domain)) => format!("{}***@{domain}", local.chars().next().map(String::from).unwrap_or_default()),
        None => "***".to_owned(),
    }
}

/// The emailed link. It works only for the administrator it was made for, while they are signed
/// in, so a mail scanner that follows it gets nothing and does not use it up.
async fn download(State(state): State<AppState>, v: Viewer, Path(token): Path<String>) -> Result<Response, AppError> {
    let Some(user) = v.user.as_ref() else { return Ok(Redirect::to("/login").into_response()) };
    // Anyone else sees what a guest sees of the admin panel: nothing.
    if !user.is_admin || user.needs_2fa {
        return Err(AppError::NotFound);
    }
    let pool = state.db()?;
    let digest = auth::token_digest(&token);
    if bf_db::accounts::security::email_token_owner(pool, &digest, "backup").await.db()? != Some(user.id) {
        return Err(AppError::NotFound);
    }
    let (_, name) = bf_db::accounts::security::consume_email_token(pool, &digest, "backup")
        .await
        .db()?
        .ok_or(AppError::NotFound)?;
    let name = name.ok_or(AppError::NotFound)?;
    let path = backup::path_of(&backup::dir(), &name).map_err(|_| AppError::NotFound)?;
    let file = tokio::fs::File::open(&path).await.map_err(AppError::internal)?;
    let size = file.metadata().await.map_err(AppError::internal)?.len();
    audit_log(pool, Some(user.id.get()), "backup.download", "site", json!({ "name": name })).await?;
    let body = axum::body::Body::from_stream(futures_util::stream::unfold(file, |mut f| async move {
        let mut buf = vec![0u8; 256 * 1024];
        match f.read(&mut buf).await {
            Ok(0) => None,
            Ok(n) => {
                buf.truncate(n);
                Some((Ok::<_, std::io::Error>(buf), f))
            }
            Err(e) => Some((Err(e), f)),
        }
    }));
    Ok((
        [
            (header::CONTENT_TYPE, "application/gzip".to_owned()),
            (header::CONTENT_LENGTH, size.to_string()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\"")),
            (header::CACHE_CONTROL, "no-store".to_owned()),
        ],
        body,
    )
        .into_response())
}

#[derive(Deserialize)]
struct RestoreBody {
    /// Must be the word `restore`: a restore replaces everything.
    confirm: String,
}

/// Replaces the site with a backup. Starts in the background with the site offline.
async fn restore(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path(name): Path<String>,
    Json(body): Json<RestoreBody>,
) -> Result<Response, AppError> {
    if body.confirm != "restore" {
        return Err(AppError::Validation("Type restore to confirm.".into()));
    }
    let pool = state.db()?.clone();
    let path = backup::path_of(&backup::dir(), &name).map_err(|_| AppError::NotFound)?;
    let Some(run) = Running::start("restore") else { return Err(busy()) };
    tokio::spawn(async move {
        let result = run_restore(&state, &pool, &path).await;
        match &result {
            Ok(()) => {
                // The restore replaced the log, so this line is the first of the new one.
                let details = json!({ "name": name });
                if let Err(e) = audit_log(&pool, Some(who.unwrap_or(0)), "backup.restore", "site", details).await {
                    tracing::warn!(error = %e, "restore not logged");
                }
            }
            Err(e) => tracing::error!(error = %e, "restore failed"),
        }
        run.finish(&result);
    });
    Ok((StatusCode::ACCEPTED, Json(json!({ "started": true }))).into_response())
}

async fn set_offline(state: &AppState, on: bool) {
    let Some(pool) = &state.pool else { return };
    if let Err(e) = bf_db::settings::save(pool, "offline", &json!(on)).await {
        tracing::warn!(error = %e, "offline mode not changed");
    }
    state.reload_settings().await;
}

async fn run_restore(state: &AppState, pool: &sqlx::PgPool, path: &std::path::Path) -> Result<(), backup::BackupError> {
    let was_offline = state.settings().bool("offline");
    set_offline(state, true).await;
    // The role that owns the tables: the application's own role may not truncate them.
    let outcome = async {
        let ddl = bf_db::ddl_pool(pool).await?;
        backup::restore(&ddl, path, &backup::uploads_dir()).await
    }
    .await;
    match outcome {
        Ok(_) => {
            // Everything kept in memory came from the data that was just replaced.
            state.reload_settings().await;
            state.forum.invalidate();
            state.reload_site_data().await;
            state.microcache.clear();
            Ok(())
        }
        Err(e) => {
            // Nothing changed (the load is one transaction): put the site back as it was.
            set_offline(state, was_offline).await;
            Err(e)
        }
    }
}

/// Called every tick: takes a backup when automatic backups are on and the last one is old enough.
pub(crate) async fn run_tick(state: &AppState) {
    let s = state.settings();
    if !s.bool("backup_enabled") {
        return;
    }
    let interval = chrono::Duration::hours(s.int("backup_interval_hours").max(1));
    let dir = backup::dir();
    let newest = blocking(move || backup::list(&dir).into_iter().next()).await.ok().flatten();
    // A little early is fine: the tick is not exact.
    let due = newest.is_none_or(|b| {
        chrono::DateTime::parse_from_rfc3339(&b.manifest.created_at)
            .map_or(true, |t| chrono::Utc::now().signed_duration_since(t) >= interval - chrono::Duration::minutes(5))
    });
    if due && !spawn_backup(state, "automatic", s.bool("backup_uploads"), None) {
        tracing::debug!("automatic backup skipped: one is already running");
    }
}
