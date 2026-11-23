//! Data export, account deletion and account merging.

use crate::admin::api::{Admin, AdminWrite};
use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use bf_core::AppError;
use bf_db::accounts::lifecycle;
use serde::Deserialize;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/settings/export", get(export))
        .route("/settings/export.zip", get(export_zip))
        .route("/settings/delete", post(request_delete))
        .route("/settings/delete/cancel", post(cancel_delete))
        .route("/api/v1/admin/users/{id}/merge", post(merge))
}

const GRACE_DAYS: i32 = 14;

fn grace_days() -> i32 {
    std::env::var("BF_DELETE_GRACE_DAYS").ok().and_then(|s| s.parse().ok()).unwrap_or(GRACE_DAYS)
}

/// The whole export holds the address, private messages and post IPs: a browser session that has
/// just proved who it is (sudo). An API key or OAuth token, even a read-only one, never gets it.
async fn export(State(state): State<AppState>, v: Viewer) -> Result<Response, AppError> {
    let user = v.require_user()?;
    if v.api_scope.is_some() {
        return Err(AppError::Forbidden);
    }
    if let Some(confirm) = v.sudo_redirect("/settings/export") {
        return Ok(confirm);
    }
    let data = lifecycle::export(state.db()?, user.id.get()).await.db()?;
    let body = serde_json::to_vec_pretty(&data).map_err(AppError::internal)?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/json".to_owned()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"forum-data-{}.json\"", user.id)),
            (header::CACHE_CONTROL, "no-store".to_owned()),
        ],
        body,
    )
        .into_response())
}

/// Most files and bytes one ZIP takes. A member with more gets the rest listed by name, so the
/// download stays bounded; the JSON export still describes every upload.
const ZIP_MAX_FILES: usize = 2_000;
const ZIP_MAX_BYTES: i64 = 512 * 1024 * 1024;

/// One ZIP at a time per two exports in flight: each reads files from the store.
static ZIPPING: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

type Zip = zip::ZipWriter<std::fs::File>;

/// Writes one entry on a blocking thread, handing the writer back.
async fn zip_add(z: Zip, name: String, bytes: Vec<u8>, compress: bool) -> Result<Zip, AppError> {
    tokio::task::spawn_blocking(move || -> Result<Zip, String> {
        use std::io::Write;
        let method = if compress { zip::CompressionMethod::Deflated } else { zip::CompressionMethod::Stored };
        let mut z = z;
        z.start_file(name, zip::write::SimpleFileOptions::default().compression_method(method))
            .map_err(|e| e.to_string())?;
        z.write_all(&bytes).map_err(|e| e.to_string())?;
        Ok(z)
    })
    .await
    .map_err(AppError::internal)?
    .map_err(AppError::internal)
}

/// A file name that is safe inside an archive.
fn zip_name(id: usize, name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
        .take(80)
        .collect();
    format!("uploads/{id:05}-{clean}")
}

/// The JSON export plus the member's uploaded files, as one ZIP. It is written to a temporary
/// file, never held in memory, and sent in chunks; the number and total size of files are capped.
async fn export_zip(State(state): State<AppState>, v: Viewer) -> Result<Response, AppError> {
    let user = v.require_user()?;
    if v.api_scope.is_some() {
        return Err(AppError::Forbidden);
    }
    if let Some(confirm) = v.sudo_redirect("/settings/export.zip") {
        return Ok(confirm);
    }
    crate::http::ratelimit::EXPORTS_BY_USER.enforce(&format!("export:{}", user.id))?;
    let _slot = ZIPPING.try_acquire().map_err(|_| AppError::RateLimited { retry_after_secs: 30 })?;
    let pool = state.db()?;
    let data = lifecycle::export(pool, user.id.get()).await.db()?;
    let json = serde_json::to_vec_pretty(&data).map_err(AppError::internal)?;
    let files = lifecycle::upload_files(pool, user.id.get(), ZIP_MAX_FILES as i64 + 1).await.db()?;
    let path = std::env::temp_dir().join(format!("bf-export-{}.zip", bf_core::auth::random_token()));
    let file = std::fs::File::create(&path).map_err(AppError::internal)?;
    let mut z = zip_add(zip::ZipWriter::new(file), "data.json".into(), json, true).await?;
    let (mut total, mut left_out) = (0_i64, Vec::<String>::new());
    for (n, (key, name, size)) in files.into_iter().enumerate() {
        let bytes = if n >= ZIP_MAX_FILES || total.saturating_add(size) > ZIP_MAX_BYTES {
            None
        } else {
            match key.get(..2) {
                Some(prefix) => crate::forum::uploads::byte_store().get(&format!("{prefix}/{key}")).await,
                None => None,
            }
        };
        match bytes {
            Some(b) => {
                total = total.saturating_add(b.len() as i64);
                z = zip_add(z, zip_name(n, &name), b, false).await?;
            }
            None => left_out.push(name),
        }
    }
    if !left_out.is_empty() {
        let head = v.t().msg("These uploads are not in this archive (size limit, or the file is missing):");
        let text = format!("{head}\n\n{}\n", left_out.join("\n"));
        z = zip_add(z, "uploads-not-included.txt".into(), text.into_bytes(), true).await?;
    }
    tokio::task::spawn_blocking(move || z.finish().map(drop).map_err(|e| e.to_string()))
        .await
        .map_err(AppError::internal)?
        .map_err(AppError::internal)?;
    let file = tokio::fs::File::open(&path).await.map_err(AppError::internal)?;
    let len = file.metadata().await.map_err(AppError::internal)?.len();
    // On Linux the name can go now; the open handle keeps the bytes until the download ends.
    let _ = tokio::fs::remove_file(&path).await;
    let chunks = futures_util::stream::unfold(file, |mut file| async move {
        use tokio::io::AsyncReadExt;
        let mut buf = vec![0u8; 64 * 1024];
        match file.read(&mut buf).await {
            Ok(0) => None,
            Ok(n) => {
                buf.truncate(n);
                Some((Ok::<_, std::io::Error>(axum::body::Bytes::from(buf)), file))
            }
            Err(e) => Some((Err(e), file)),
        }
    });
    Ok((
        [
            (header::CONTENT_TYPE, "application/zip".to_owned()),
            (header::CONTENT_LENGTH, len.to_string()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"forum-data-{}.zip\"", user.id)),
            (header::CACHE_CONTROL, "no-store".to_owned()),
        ],
        axum::body::Body::from_stream(chunks),
    )
        .into_response())
}

#[derive(Deserialize)]
struct DeleteForm {
    csrf: String,
    confirm: String,
    /// "delete" takes the member's posts along; anything else keeps them without the name.
    #[serde(default)]
    content: String,
}

/// Starts the grace period. Typing the username guards against a stray click; a mail tells the
/// member, so someone using a stolen session cannot do this unnoticed.
async fn request_delete(
    State(state): State<AppState>,
    v: Viewer,
    Form(f): Form<DeleteForm>,
) -> Result<Response, AppError> {
    v.check_session_csrf(&f.csrf)?;
    if let Some(confirm) = v.sudo_redirect("/settings") {
        return Ok(confirm);
    }
    let user = v.require_user()?;
    if user.is_admin {
        return Err(AppError::Validation("An administrator must give up that role first.".into()));
    }
    if !f.confirm.trim().eq_ignore_ascii_case(&user.username) {
        return Err(AppError::Validation("Type your username to confirm.".into()));
    }
    let pool = state.db()?;
    lifecycle::request_deletion(pool, user.id.get(), grace_days()).await.db()?;
    lifecycle::set_delete_content(pool, user.id.get(), f.content == "delete").await.db()?;
    let site = state.settings().str("site_name").to_owned();
    let t = crate::http::i18n::user_t(&state, user.id.get()).await;
    let body = t.msgf(
        "Hi {name},\n\nYour account on {site} will be deleted in {days} days.\n\nIf you did not ask for this, log in and cancel it under Account settings: {link}\n\n— {site}\n",
        &[
            ("name", &user.username),
            ("site", &site),
            ("days", &grace_days().to_string()),
            ("link", &format!("{}/settings", state.config.base_url)),
        ],
    );
    let subject = t.msgf("Your account will be deleted - {site}", &[("site", &site)]);
    bf_db::platform::mail::queue_mail(pool, &user.email, &subject, &body).await.db()?;
    Ok(Redirect::to("/settings").into_response())
}

async fn cancel_delete(
    State(state): State<AppState>,
    v: Viewer,
    Form(f): Form<crate::http::viewer::CsrfForm>,
) -> Result<Response, AppError> {
    v.check_session_csrf(&f.csrf)?;
    let user = v.require_user()?;
    lifecycle::cancel_deletion(state.db()?, user.id.get()).await.db()?;
    Ok(Redirect::to("/settings").into_response())
}

/// Runs from the maintenance tick.
pub(crate) async fn run_deletions(state: &AppState) {
    let Some(pool) = &state.pool else { return };
    let due = match lifecycle::due_deletions(pool).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e, "deletion query failed");
            return;
        }
    };
    for user in due {
        crate::federation::actor_deleted(state, user).await;
        // Plugins are called and awaited before the data goes, so they can purge what they keep.
        crate::platform::plugins::dispatch_wait(state, "user.erased", &serde_json::json!({ "user_id": user })).await;
        if let Err(e) = lifecycle::anonymize(pool, user).await {
            tracing::error!(error = %e, user, "account deletion failed");
        }
    }
    state.microcache.clear();
}

#[derive(Deserialize)]
struct MergeBody {
    into: i64,
}

/// Merges `id` into another account. The source must not be an administrator: this removes its
/// sign-in for good.
async fn merge(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path(id): Path<i64>,
    Json(b): Json<MergeBody>,
) -> Result<Response, AppError> {
    let pool = state.db()?;
    if id == b.into {
        return Err(AppError::Validation("Choose two different accounts.".into()));
    }
    let source = bf_db::accounts::users::user_by_id(pool, id.into()).await.db()?.ok_or(AppError::NotFound)?;
    bf_db::accounts::users::user_by_id(pool, b.into.into()).await.db()?.ok_or(AppError::NotFound)?;
    if source.is_admin {
        return Err(AppError::Validation("An administrator cannot be merged into another account.".into()));
    }
    crate::federation::actor_deleted(&state, id).await;
    lifecycle::merge(pool, id, b.into, who).await.db()?;
    state.forum.invalidate();
    state.microcache.clear();
    Ok(StatusCode::NO_CONTENT.into_response())
}
