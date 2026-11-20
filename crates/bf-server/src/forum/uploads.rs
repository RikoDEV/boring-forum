//! Uploads, uploaded avatars and composer drafts.
//!
//! Files are content-addressed under `BF_UPLOAD_DIR` (default `data/uploads`). The type
//! comes from the file's leading bytes, never from its name or the client's claim, and
//! only a fixed allow-list is accepted (no SVG or HTML, which could carry scripts).
//! Files are served with `nosniff` and a `sandbox` CSP; non-images download as attachments.

use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::{DefaultBodyLimit, Multipart, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bf_core::AppError;
use bf_db::forum::uploads;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::LazyLock;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/uploads", post(upload).layer(DefaultBodyLimit::max(BODY_CAP)))
        .route("/settings/avatar", post(set_avatar).layer(DefaultBodyLimit::max(SMALL_BODY_CAP)))
        .route("/settings/avatar/remove", post(remove_avatar))
        .route("/settings/cover/remove", post(remove_cover))
        .route("/settings/cover/position", post(set_cover_position))
        .route("/settings/cover", post(set_cover).layer(DefaultBodyLimit::max(SMALL_BODY_CAP)))
        .route("/uploads/{key}", get(serve))
        .route("/api/v1/drafts", get(get_draft).put(put_draft).delete(delete_draft))
}

static DIR: LazyLock<PathBuf> = LazyLock::new(|| {
    std::env::var("BF_UPLOAD_DIR").ok().filter(|d| !d.is_empty()).unwrap_or_else(|| "data/uploads".into()).into()
});

/// Hard cap on request bodies for upload routes; the per-site setting is checked too.
pub(crate) const BODY_CAP: usize = 100 * 1024 * 1024;

/// Avatars (1 MB) and covers (2 MB) never need more than this on the wire.
const SMALL_BODY_CAP: usize = 4 * 1024 * 1024;

/// A single range response is at most this long; players ask again for the rest.
const RANGE_CHUNK: usize = 8 * 1024 * 1024;

/// (mime, extension, is_image) from magic bytes.
pub(crate) fn sniff(bytes: &[u8], filename: &str) -> Option<(&'static str, &'static str, bool)> {
    let starts = |m: &[u8]| bytes.starts_with(m);
    Some(if starts(b"\x89PNG\r\n\x1a\n") {
        ("image/png", "png", true)
    } else if starts(b"\xFF\xD8\xFF") {
        ("image/jpeg", "jpg", true)
    } else if starts(b"GIF87a") || starts(b"GIF89a") {
        ("image/gif", "gif", true)
    } else if bytes.len() >= 12 && starts(b"RIFF") && &bytes[8..12] == b"WEBP" {
        ("image/webp", "webp", true)
    } else if starts(b"%PDF-") {
        ("application/pdf", "pdf", false)
    } else if starts(b"PK\x03\x04") {
        ("application/zip", "zip", false)
    } else if filename.to_ascii_lowercase().ends_with(".txt")
        && !bytes.contains(&0)
        && std::str::from_utf8(bytes).is_ok()
    {
        ("text/plain; charset=utf-8", "txt", false)
    } else {
        return None;
    })
}

/// What a route takes: images only (avatars), or any allowed file (posts).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Accept {
    Images,
    Files,
}

fn clean_filename(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("file");
    let cleaned: String = base.chars().filter(|c| !c.is_control() && !"<>:\"|?*".contains(*c)).take(120).collect();
    if cleaned.trim().is_empty() { "file".into() } else { cleaned }
}

pub(crate) struct Stored {
    pub upload: uploads::Upload,
    pub is_image: bool,
}

/// Who a file is stored for: a member uploading it (counted against their daily limit, announced
/// to webhooks) or an import filling the site (neither: a backfill is not a member's day).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Origin {
    Member,
    Import,
}

/// Validates and stores one file for `user_id`. `images_only` is used for avatars.
pub(crate) async fn store(
    state: &AppState,
    user_id: i64,
    filename: &str,
    bytes: &[u8],
    accept: Accept,
    max_kb: i64,
) -> Result<Stored, AppError> {
    store_from(state, user_id, filename, bytes, accept, max_kb, Origin::Member).await
}

/// `store`, for a file that is not a member's upload of the moment.
pub(crate) async fn store_from(
    state: &AppState,
    user_id: i64,
    filename: &str,
    bytes: &[u8],
    accept: Accept,
    max_kb: i64,
    origin: Origin,
) -> Result<Stored, AppError> {
    if bytes.is_empty() {
        return Err(AppError::Validation("The file is empty.".into()));
    }
    if bytes.len() as i64 > max_kb * 1024 {
        return Err(AppError::invalid("Files can be at most {max_kb} KB.", &[("max_kb", &max_kb)]));
    }
    let found = sniff(bytes, filename);
    let (mime, ext, is_image) = found.ok_or_else(|| AppError::Validation("That file type is not allowed.".into()))?;
    if accept == Accept::Images && !is_image {
        return Err(AppError::Validation("Please choose a PNG, JPEG, GIF or WebP image.".into()));
    }
    // A picture that claims more pixels than any decoder here accepts (a few KB of PNG can claim
    // hundreds of megabytes) is refused before anything reads it.
    if is_image && too_many_pixels(bytes) {
        return Err(AppError::invalid("Pictures can be at most {side}×{side} pixels.", &[("side", &MAX_DECODE_SIDE)]));
    }
    let pool = state.db()?;
    // Nothing is stored before the scans have passed.
    if is_image {
        crate::moderation::scan::check_image(state, user_id, bytes).await?;
    }
    // Location never leaves the uploader's device by way of this site (see `metadata`); an image
    // that cannot be cleaned is refused rather than stored with whatever it carries.
    let scrubbed = if is_image {
        crate::forum::metadata::scrub(mime, bytes, state.settings().bool("upload_strip_metadata"))
            .ok_or_else(|| AppError::Validation("That image could not be read.".into()))?
    } else {
        bytes.to_vec()
    };
    let bytes = scrubbed.as_slice();
    // Every upload counts, also when the same bytes are already stored under another name.
    let daily = state.settings().int("upload_daily_mb") * 1024 * 1024;
    if origin == Origin::Member && !uploads::reserve_quota(pool, user_id, bytes.len() as i64, daily).await.db()? {
        return Err(AppError::Validation("You have reached today's upload limit.".into()));
    }
    let (width, height) = if is_image {
        imagesize::blob_size(bytes).map_or((None, None), |s| (Some(s.width as i32), Some(s.height as i32)))
    } else {
        (None, None)
    };
    let sha: String = Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
    let key = format!("{sha}.{ext}");
    byte_store().put(&format!("{}/{key}", &sha[..2]), bytes.to_vec()).await.map_err(AppError::internal)?;
    let upload = uploads::insert(
        pool,
        &uploads::NewUpload {
            sha256: &sha,
            user_id,
            storage_key: &key,
            filename: &clean_filename(filename),
            mime,
            size: bytes.len() as i64,
            width,
            height,
        },
    )
    .await
    .db()?;
    // The row belongs to whoever stored these bytes first; this upload is shown under its own name.
    let upload = uploads::Upload { filename: clean_filename(filename), ..upload };
    // Every stored file (posts, chat, avatars, covers) is announced here.
    if origin == Origin::Member {
        crate::platform::webhooks::fire(
            state,
            "upload.created",
            serde_json::json!({
                "user_id": user_id, "name": upload.filename, "key": upload.storage_key, "size": upload.size, "is_image": is_image,
            }),
        );
    }
    Ok(Stored { upload, is_image })
}

/// At most this many uploads are read from the network at once, so parallel requests cannot
/// hold the server's memory.
static READING: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(8);

/// Longest text field (the CSRF token and similar) a multipart body may carry.
const MAX_TEXT_FIELD: usize = 64 * 1024;

/// Reads the `file` field of a multipart body, plus any text fields, and stops reading as soon as
/// the file is over `max_kb`.
pub(crate) async fn read_multipart_max(
    mut m: Multipart,
    max_kb: i64,
) -> Result<(String, Vec<u8>, Vec<(String, String)>), AppError> {
    let _slot = READING.try_acquire().map_err(|_| AppError::RateLimited { retry_after_secs: 5 })?;
    let too_big = || AppError::invalid("Files can be at most {max_kb} KB.", &[("max_kb", &max_kb)]);
    let limit = usize::try_from(max_kb).unwrap_or(0).saturating_mul(1024);
    let mut file = None;
    let mut fields = Vec::new();
    while let Some(mut field) = m.next_field().await.map_err(|e| AppError::BadRequest(e.to_string()))? {
        let name = field.name().unwrap_or_default().to_owned();
        let is_file = name == "file";
        let cap = if is_file { limit } else { MAX_TEXT_FIELD };
        let filename = field.file_name().unwrap_or("file").to_owned();
        let mut data = Vec::new();
        while let Some(chunk) = field.chunk().await.map_err(|e| AppError::BadRequest(e.to_string()))? {
            if data.len() + chunk.len() > cap {
                return Err(if is_file { too_big() } else { AppError::BadRequest("The field is too long.".into()) });
            }
            data.extend_from_slice(&chunk);
        }
        if is_file {
            file = Some((filename, data));
        } else {
            let text = String::from_utf8(data).map_err(|e| AppError::BadRequest(e.to_string()))?;
            fields.push((name, text));
        }
    }
    let (filename, bytes) = file.ok_or_else(|| AppError::Validation("No file was sent.".into()))?;
    Ok((filename, bytes, fields))
}

/// The kinds of file the site accepts for posts, messages and chat (`upload_allowed_types`), or,
/// for chat, those chat accepts too.
pub(crate) fn allowed_types(state: &AppState, chat: bool) -> Vec<&'static str> {
    let s = state.settings();
    let site = bf_core::settings::file_types(s.str("upload_allowed_types"));
    if !chat {
        return site;
    }
    let chat_list = bf_core::settings::file_types(s.str("chat_allowed_types"));
    site.into_iter().filter(|t| chat_list.contains(t)).collect()
}

/// The kinds of file referenced by the site's own upload links in a text, one entry for each link:
/// `/uploads/<64 hex>.<ext>`.
pub(crate) fn upload_refs(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, _) in raw.match_indices("/uploads/") {
        let rest = &raw[i + "/uploads/".len()..];
        let hex = rest.bytes().take_while(u8::is_ascii_hexdigit).count();
        if hex == 64
            && let Some(ext) = rest[64..].strip_prefix('.')
        {
            let ext: String = ext.chars().take_while(char::is_ascii_alphanumeric).collect();
            if !ext.is_empty() {
                out.push(ext.to_ascii_lowercase());
            }
        }
    }
    out
}

#[derive(Deserialize, Default)]
struct UploadQuery {
    /// Where the file is going, so its rules apply when it is picked: `chat`.
    #[serde(default, rename = "for")]
    target: String,
}

/// `POST /api/v1/uploads` (multipart `file`), CSRF token in `X-CSRF-Token`.
async fn upload(
    State(state): State<AppState>,
    v: Viewer,
    Query(q): Query<UploadQuery>,
    headers: HeaderMap,
    m: Multipart,
) -> Result<Response, AppError> {
    let user = v.require_user()?;
    v.check_csrf_headers(&headers)?;
    if v.actor.silenced {
        return Err(AppError::Forbidden);
    }
    crate::moderation::gate::account_ready(&state, user).map_err(AppError::Validation)?;
    crate::http::ratelimit::UPLOADS_BY_USER.enforce(&format!("upload:{}", user.id))?;
    let staff = crate::http::viewer::is_staff(&v);
    let chat = q.target == "chat";
    let s = state.settings();
    if !staff {
        let need = if chat {
            s.int("upload_min_trust_level").max(s.int("chat_upload_min_trust_level"))
        } else {
            s.int("upload_min_trust_level")
        };
        if i64::from(user.trust_level) < need {
            return Err(AppError::invalid("Files can be added from trust level {n}.", &[("n", &need)]));
        }
        if chat && !s.bool("chat_allow_uploads") {
            return Err(AppError::Validation("Files are not allowed in chat.".into()));
        }
    }
    // A group limit, when every one of the member's groups sets one, overrides the site default.
    let group = bf_db::accounts::limits::limits(state.db()?, &user.groups).await.db()?;
    let max_kb = group.max_upload_kb.unwrap_or_else(|| state.settings().int("max_upload_kb"));
    let (filename, bytes, _) = read_multipart_max(m, max_kb).await?;
    // The kind is checked before anything is scanned or stored.
    if let Some((_, ext, _)) = sniff(&bytes, &filename) {
        let allowed = allowed_types(&state, chat && !staff);
        if !allowed.contains(&ext) {
            return Err(if allowed.is_empty() {
                AppError::Validation("Files cannot be added here.".into())
            } else {
                AppError::invalid(
                    "Only these kinds of file can be added here: {types}.",
                    &[("types", &allowed.join(", "))],
                )
            });
        }
    }
    let s = store(&state, user.id.get(), &filename, &bytes, Accept::Files, max_kb).await?;
    let u = &s.upload;
    let url = format!("/uploads/{}", u.storage_key);
    let label = u.filename.replace(['[', ']'], "");
    // The size travels in the fragment, so the cooker can add dimensions and a srcset.
    let size = u.width.zip(u.height).map(|(w, h)| format!("#{w}x{h}")).unwrap_or_default();
    let markdown = if s.is_image {
        format!("![{label}]({url}{size})")
    } else {
        format!("[{label} ({})]({url})", bf_core::html::human_size(u.size))
    };
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "url": url, "markdown": markdown, "filename": u.filename, "size": u.size,
            "width": u.width, "height": u.height, "is_image": s.is_image,
        })),
    )
        .into_response())
}

pub(crate) fn valid_key(key: &str) -> bool {
    match key.split_once('.') {
        Some((sha, ext)) => {
            sha.len() == 64
                && sha.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                && matches!(
                    ext,
                    "png" | "jpg" | "gif" | "webp" | "pdf" | "zip" | "txt" | "mp4" | "webm" | "7z" | "rar" | "gz"
                )
        }
        None => false,
    }
}

/// Removes an upload's file and its thumbnails; returns how many files went away.
pub(crate) async fn delete_files(key: &str) -> u32 {
    if !valid_key(key) {
        return 0;
    }
    let Some((name, _)) = key.split_once('.') else { return 0 };
    let mut gone = 0;
    let thumbs = bf_cook::THUMB_WIDTHS
        .iter()
        .flat_map(|w| ["avif", "webp", "png", "jpg"].map(|format| format!("{name}-{w}.{format}")));
    for file in std::iter::once(key.to_owned()).chain(thumbs) {
        if byte_store().delete(&format!("{}/{file}", &key[..2])).await {
            gone += 1;
        }
    }
    gone
}

/// Splits `{sha}-{width}.{ext}` into the original's key and a known thumbnail width.
fn thumb_key(key: &str) -> Option<(String, u32)> {
    let (name, ext) = key.split_once('.')?;
    let (sha, width) = name.split_once('-')?;
    let parsed: u32 = width.parse().ok()?;
    // Only the canonical spelling: `+480` or `0480` parse to 480 but would be separate cache files,
    // each decoded and stored again (a way to make the server resize one image without end).
    (bf_cook::THUMB_WIDTHS.contains(&parsed) && parsed.to_string() == width).then(|| (format!("{sha}.{ext}"), parsed))
}

/// Resizes at most this many images at once, whatever the request rate.
/// The slot travels into the blocking thread, so it is free only when the work has really ended,
/// also after the request that asked for it gave up.
static RESIZING: LazyLock<std::sync::Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| std::sync::Arc::new(tokio::sync::Semaphore::new(2)));

/// Thumbnails that could not be made (too big, broken, too slow), so a bad file costs one attempt
/// an hour and not one per request.
static NOT_RESIZABLE: LazyLock<moka::sync::Cache<String, ()>> = LazyLock::new(|| {
    moka::sync::Cache::builder().max_capacity(20_000).time_to_live(std::time::Duration::from_secs(3600)).build()
});

/// Largest image that may be stored or decoded, whatever its file size claims (decompression bombs).
pub(crate) const MAX_DECODE_SIDE: u32 = 8_000;

/// Total pixels: 40 MP is 160 MB as RGBA.
pub(crate) const MAX_DECODE_PIXELS: u64 = 40_000_000;

const MAX_DECODE_BYTES: u64 = 192 * 1024 * 1024;

/// Does the picture's header claim a size no decoder here will take?
pub(crate) fn too_many_pixels(bytes: &[u8]) -> bool {
    imagesize::blob_size(bytes).is_ok_and(|s| {
        s.width as u64 > u64::from(MAX_DECODE_SIDE)
            || s.height as u64 > u64::from(MAX_DECODE_SIDE)
            || (s.width as u64) * (s.height as u64) > MAX_DECODE_PIXELS
    })
}

/// The best thumbnail format this browser takes. Lossless WebP only replaces PNG:
/// it would make a JPEG photo bigger.
fn thumb_format(headers: &HeaderMap, ext: &str) -> &'static str {
    let accept = headers.get(header::ACCEPT).and_then(|h| h.to_str().ok()).unwrap_or_default();
    match ext {
        _ if accept.contains("image/avif") => "avif",
        "png" if accept.contains("image/webp") => "webp",
        "webp" => "webp",
        "png" => "png",
        _ => "jpg",
    }
}

fn resize(bytes: &[u8], ext: &str, width: u32) -> Result<Option<Vec<u8>>, image::ImageError> {
    use image::{ImageFormat, ImageReader, Limits, codecs::jpeg::JpegEncoder};
    // Files stored before the upload check existed are never decoded.
    if too_many_pixels(bytes) {
        return Ok(None);
    }
    let mut reader = ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DECODE_SIDE);
    limits.max_image_height = Some(MAX_DECODE_SIDE);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    let img = reader.decode()?;
    if img.width() <= width {
        return Ok(None);
    }
    let height = (u64::from(img.height()) * u64::from(width) / u64::from(img.width())).max(1) as u32;
    let small = img.resize_exact(width, height, image::imageops::FilterType::Triangle);
    let mut out = std::io::Cursor::new(Vec::new());
    match ext {
        // Speed 8 of 10: thumbnails are made on demand, so encoding must stay quick.
        "avif" => small
            .to_rgba8()
            .write_with_encoder(image::codecs::avif::AvifEncoder::new_with_speed_quality(&mut out, 8, 70))?,
        "jpg" => small.to_rgb8().write_with_encoder(JpegEncoder::new_with_quality(&mut out, 82))?,
        "png" => small.write_to(&mut out, ImageFormat::Png)?,
        "webp" => small.write_to(&mut out, ImageFormat::WebP)?,
        _ => return Ok(None),
    }
    Ok(Some(out.into_inner()))
}

/// The thumbnail's bytes, created on first request and kept next to the original. Images
/// that are too small or cannot be resized are served as they are.
async fn thumbnail(key: &str, original_key: &str, width: u32, format: &str) -> Result<(Vec<u8>, bool), AppError> {
    let prefix = &key[..2];
    let path = format!("{prefix}/{}.{format}", key.rsplit_once('.').map_or(key, |(n, _)| n));
    if let Some(bytes) = byte_store().get(&path).await {
        return Ok((bytes, true));
    }
    let original_path = format!("{prefix}/{original_key}");
    if NOT_RESIZABLE.contains_key(&path) {
        let original = byte_store().get(&original_path).await.ok_or(AppError::NotFound)?;
        return Ok((original, false));
    }
    // Wait for a slot before the original is read into memory, and not for long: a queue of
    // requests each holding a whole file is how many parallel requests would exhaust memory.
    let permit = tokio::time::timeout(std::time::Duration::from_secs(5), RESIZING.clone().acquire_owned())
        .await
        .map_err(|_| AppError::RateLimited { retry_after_secs: 5 })?
        .map_err(AppError::internal)?;
    // Another request may have made it while this one waited.
    if let Some(bytes) = byte_store().get(&path).await {
        return Ok((bytes, true));
    }
    let original = byte_store().get(&original_path).await.ok_or(AppError::NotFound)?;
    let ext = format.to_owned();
    let source = original.clone();
    let task = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        resize(&source, &ext, width)
    });
    let made = tokio::time::timeout(std::time::Duration::from_secs(20), task).await;
    let small = match made {
        Ok(Ok(Ok(Some(small)))) => small,
        other => {
            match other {
                Err(_) => tracing::warn!(key, "thumbnail took too long; serving the original"),
                Ok(Ok(Err(e))) => tracing::warn!(error = %e, key, "thumbnail failed; serving the original"),
                Ok(Err(e)) => tracing::warn!(error = %e, key, "thumbnail task failed; serving the original"),
                Ok(Ok(Ok(_))) => {}
            }
            NOT_RESIZABLE.insert(path, ());
            return Ok((original, false));
        }
    };
    byte_store().put(&path, small.clone()).await.map_err(AppError::internal)?;
    Ok((small, true))
}

/// What `serve` sends: a thumbnail made in memory, or a stored file read as it goes out.
enum Payload {
    Bytes(Vec<u8>),
    Stored(String),
}

async fn serve(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    axum::extract::RawQuery(query): axum::extract::RawQuery,
    Path(key): Path<String>,
) -> Result<Response, AppError> {
    let thumb = thumb_key(&key);
    let original_key = thumb.as_ref().map_or(key.as_str(), |(k, _)| k.as_str());
    if !valid_key(original_key) {
        return Err(AppError::NotFound);
    }
    let pool = state.db()?;
    let u = uploads::by_key(pool, original_key).await.db()?.ok_or(AppError::NotFound)?;
    // Who may have it follows where it is used; unknown and not allowed look the same.
    let access = super::upload_access::access(&state, &v, &u).await?;
    if !access.allowed {
        return Err(AppError::NotFound);
    }
    // The hotlink rule: for the kinds of file it covers, a signed link or a signed-in member.
    let mode = state.settings().str("hotlink_protection").to_owned();
    let ext = original_key.rsplit('.').next().unwrap_or_default();
    let covered = super::upload_links::covers(&mode, ext);
    let now = chrono::Utc::now().timestamp();
    let signed_until = super::upload_access::signature_until(query.as_deref(), &original_key[..64], now);
    if super::upload_access::link_refused(covered, &headers, v.user.is_some(), signed_until.is_some()) {
        return Err(AppError::Forbidden);
    }
    let mut served_as = None;
    let payload = match &thumb {
        Some((_, width)) if u.mime.starts_with("image/") && u.mime != "image/gif" => {
            let ext = original_key.rsplit('.').next().unwrap_or_default();
            let format = thumb_format(&headers, ext);
            let (bytes, converted) = thumbnail(&key, original_key, *width, format).await?;
            if converted {
                served_as = Some(match format {
                    "avif" => "image/avif",
                    "webp" => "image/webp",
                    "png" => "image/png",
                    _ => "image/jpeg",
                });
            }
            Payload::Bytes(bytes)
        }
        Some(_) => return Err(AppError::NotFound),
        None => Payload::Stored(format!("{}/{key}", &key[..2])),
    };
    let is_image = u.mime.starts_with("image/");
    let is_video = u.mime.starts_with("video/");
    let mut res = match payload {
        Payload::Bytes(bytes) => bytes.into_response(),
        // The file is read from the store as it is sent, never held whole in memory.
        Payload::Stored(path) => {
            let total = byte_store().size(&path).await.ok_or(AppError::NotFound)?;
            let total = usize::try_from(total).map_err(AppError::internal)?;
            // Video players seek with Range requests; one range is enough for every browser.
            let range = if is_video {
                headers.get(header::RANGE).and_then(|r| r.to_str().ok()).and_then(|r| byte_range(r, total))
            } else {
                None
            };
            if let Some((start, end)) = range {
                let end = end.min(start.saturating_add(RANGE_CHUNK - 1));
                let part = byte_store().get_range(&path, start as u64, (end - start + 1) as u64).await;
                let mut r = part.ok_or(AppError::NotFound)?.into_response();
                *r.status_mut() = StatusCode::PARTIAL_CONTENT;
                r.headers_mut().insert(
                    header::CONTENT_RANGE,
                    HeaderValue::from_str(&format!("bytes {start}-{end}/{total}")).map_err(AppError::internal)?,
                );
                r
            } else {
                let mut r = Response::new(byte_store().stream(&path).await.ok_or(AppError::NotFound)?);
                r.headers_mut().insert(header::CONTENT_LENGTH, HeaderValue::from(total));
                r
            }
        }
    };
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_str(served_as.unwrap_or(&u.mime)).map_err(AppError::internal)?);
    if thumb.is_some() {
        h.insert(header::VARY, HeaderValue::from_static("Accept"));
    }
    // A signed link may be kept as long as it is good; an avatar, cover or emoji anyone could have
    // without one, for good, a file in a post for an hour; anything only some may have, or that needs a signature, only by the one who asked.
    let cache = match (access.public, covered, signed_until) {
        (true, true, Some(until)) => format!("public, max-age={}", (until - now).clamp(0, 86_400)),
        // Only avatars, covers and emoji are kept for good; a file in a post may be deleted or hidden.
        (true, false, _) if !access.in_content => "public, max-age=31536000, immutable".to_owned(),
        (true, false, _) => "public, max-age=3600".to_owned(),
        _ => "private, max-age=3600".to_owned(),
    };
    h.insert(header::CACHE_CONTROL, HeaderValue::from_str(&cache).map_err(AppError::internal)?);
    if !access.public || covered {
        h.append(header::VARY, HeaderValue::from_static("Cookie"));
    }
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    // Other sites' pages may embed it, unless the hotlink rule covers this kind of file.
    h.insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static(if covered { "same-site" } else { "cross-origin" }),
    );
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'none'; img-src 'self'; sandbox"));
    if is_video {
        h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    }
    if !is_image && !is_video {
        let disp = bf_core::html::attachment(&u.filename);
        h.insert(header::CONTENT_DISPOSITION, HeaderValue::from_str(&disp).map_err(AppError::internal)?);
        uploads::count_download(pool, u.id).await.db()?;
    }
    Ok(res)
}

/// What every route that stores a picture for a member checks first, as `/api/v1/uploads` does.
fn picture_gate(state: &AppState, v: &Viewer) -> Result<(), AppError> {
    let user = v.require_user()?;
    if !user.approved {
        return Err(AppError::Validation("Your account is waiting for approval by the staff.".into()));
    }
    if v.actor.silenced {
        return Err(AppError::Forbidden);
    }
    crate::moderation::gate::account_ready(state, user).map_err(AppError::Validation)?;
    crate::http::ratelimit::UPLOADS_BY_USER.enforce(&format!("upload:{}", user.id))
}

async fn set_avatar(State(state): State<AppState>, v: Viewer, m: Multipart) -> Result<Response, AppError> {
    let user = v.require_user()?;
    picture_gate(&state, &v)?;
    let (filename, bytes, fields) = read_multipart_max(m, 1024).await?;
    let csrf = fields.iter().find(|(k, _)| k == "csrf").map(|(_, v)| v.as_str()).unwrap_or_default();
    v.check_csrf(csrf)?;
    // Checked from the header before anything is scanned, scrubbed or stored.
    if imagesize::blob_size(&bytes).is_ok_and(|s| s.width > 2048 || s.height > 2048) {
        return Err(AppError::Validation("Avatars can be at most 2048×2048 pixels.".into()));
    }
    let s = store(&state, user.id.get(), &filename, &bytes, Accept::Images, 1024).await?;
    uploads::set_avatar(state.db()?, user.id.get(), Some(s.upload.id)).await.db()?;
    state.microcache.clear();
    Ok(Redirect::to(&bf_core::slug::profile_path(user.id.get(), &user.username)).into_response())
}

async fn remove_avatar(
    State(state): State<AppState>,
    v: Viewer,
    axum::Form(f): axum::Form<crate::http::viewer::CsrfForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    uploads::set_avatar(state.db()?, user.id.get(), None).await.db()?;
    state.microcache.clear();
    Ok(Redirect::to(&bf_core::slug::profile_path(user.id.get(), &user.username)).into_response())
}

/// The uploaded avatar for `/avatar/{name}.svg`, if the member has one.
pub(crate) async fn uploaded_avatar(state: &AppState, username: &str) -> Option<String> {
    let pool = state.pool.as_ref()?;
    uploads::avatar_key(pool, username).await.ok().flatten().map(|k| format!("/uploads/{k}"))
}

#[derive(Deserialize)]
struct DraftQuery {
    /// Also delete the files only this draft used (the Discard button; not after posting).
    #[serde(default)]
    purge: String,
    context: String,
}

/// `new-topic:{forum id}` or `reply:{topic id}`: one draft per place you can write.
fn valid_context(c: &str) -> bool {
    c.split_once(':').is_some_and(|(kind, id)| {
        matches!(kind, "new-topic" | "reply")
            && !id.is_empty()
            && id.len() <= 18
            && id.bytes().all(|b| b.is_ascii_digit())
    })
}

async fn get_draft(
    State(state): State<AppState>,
    v: Viewer,
    Query(q): Query<DraftQuery>,
) -> Result<Response, AppError> {
    let user = v.require_user()?;
    if !valid_context(&q.context) {
        return Err(AppError::BadRequest("bad context".into()));
    }
    let d = uploads::draft(state.db()?, user.id.get(), &q.context).await.db()?;
    let mut res = match d {
        Some(d) => Json(json!({ "data": d.data, "updated_at": d.updated_at })).into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    };
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(res)
}

async fn put_draft(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Query(q): Query<DraftQuery>,
    Json(data): Json<serde_json::Value>,
) -> Result<Response, AppError> {
    let user = v.require_user()?;
    v.check_csrf_headers(&headers)?;
    if !valid_context(&q.context) {
        return Err(AppError::BadRequest("bad context".into()));
    }
    if !data.is_object() || data.to_string().len() > 200_000 {
        return Err(AppError::Validation("Drafts are a JSON object up to 200 KB.".into()));
    }
    uploads::save_draft(state.db()?, user.id.get(), &q.context, &data).await.db()?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn delete_draft(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Query(q): Query<DraftQuery>,
) -> Result<Response, AppError> {
    let user = v.require_user()?;
    v.check_csrf_headers(&headers)?;
    let pool = state.db()?;
    // A draft thrown away on purpose takes the files only it used. (After posting the draft goes
    // too, without `purge`: a post being saved at that moment must not lose its files.)
    let keys = if q.purge == "1" {
        match uploads::draft(pool, user.id.get(), &q.context).await.db()? {
            Some(d) => draft_upload_keys(&d.data.to_string()),
            None => Vec::new(),
        }
    } else {
        Vec::new()
    };
    uploads::delete_draft(pool, user.id.get(), &q.context).await.db()?;
    for key in keys {
        if bf_db::admin::maintenance::remove_unused_upload(pool, user.id.get(), &key).await.db()? {
            delete_files(&key).await;
        }
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// The storage keys of the site's own files named in a draft's text.
fn draft_upload_keys(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (i, _) in text.match_indices("/uploads/") {
        let rest = &text[i + "/uploads/".len()..];
        let hex = rest.bytes().take_while(u8::is_ascii_hexdigit).count();
        if hex == 64
            && let Some(ext) = rest[64..].strip_prefix('.')
        {
            let ext: String = ext.chars().take_while(char::is_ascii_alphanumeric).collect();
            let key = format!("{}.{}", &rest[..64], ext.to_ascii_lowercase());
            if !ext.is_empty() && valid_key(&key) && !out.contains(&key) {
                out.push(key);
            }
        }
    }
    out
}

/// Profile cover image, an image up to 2 MB.
async fn set_cover(State(state): State<AppState>, v: Viewer, m: Multipart) -> Result<Response, AppError> {
    let user = v.require_user()?;
    picture_gate(&state, &v)?;
    let (filename, bytes, fields) = read_multipart_max(m, 2048).await?;
    let csrf = fields.iter().find(|(k, _)| k == "csrf").map(|(_, v)| v.as_str()).unwrap_or_default();
    v.check_csrf(csrf)?;
    if imagesize::blob_size(&bytes).is_ok_and(|s| s.width > 8192 || s.height > 8192) {
        return Err(AppError::Validation("Covers can be at most 8192×8192 pixels.".into()));
    }
    let s = store(&state, user.id.get(), &filename, &bytes, Accept::Images, 2048).await?;
    sqlx::query("UPDATE users SET cover_upload_id = $2, cover_y = 50 WHERE id = $1")
        .bind(user.id)
        .bind(s.upload.id)
        .execute(state.db()?)
        .await
        .db()?;
    state.microcache.clear();
    Ok(Redirect::to(&bf_core::slug::profile_path(user.id.get(), &user.username)).into_response())
}

async fn remove_cover(
    State(state): State<AppState>,
    v: Viewer,
    axum::Form(f): axum::Form<crate::http::viewer::CsrfForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    sqlx::query("UPDATE users SET cover_upload_id = NULL, cover_y = 50 WHERE id = $1")
        .bind(user.id)
        .execute(state.db()?)
        .await
        .db()?;
    state.microcache.clear();
    Ok(Redirect::to(&bf_core::slug::profile_path(user.id.get(), &user.username)).into_response())
}

#[derive(serde::Deserialize)]
struct CoverPosition {
    csrf: String,
    y: i16,
}

async fn set_cover_position(
    State(state): State<AppState>,
    v: Viewer,
    axum::Form(f): axum::Form<CoverPosition>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    if !(0..=100).contains(&f.y) {
        return Err(AppError::Validation("The position must be 0 to 100.".into()));
    }
    sqlx::query("UPDATE users SET cover_y = $2 WHERE id = $1")
        .bind(user.id)
        .bind(f.y)
        .execute(state.db()?)
        .await
        .db()?;
    state.microcache.clear();
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Where upload bytes live (`storage_backend`). The default is the local directory; a
/// compiled-in plugin can call `set_store` before the server starts, for object storage. Paths
/// are `ab/name.ext`, made here from content hashes, never from anything a visitor typed.
pub trait UploadStore: Send + Sync {
    fn put<'a>(&'a self, path: &'a str, bytes: Vec<u8>) -> BoxFuture<'a, Result<(), String>>;
    fn get<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Option<Vec<u8>>>;
    fn delete<'a>(&'a self, path: &'a str) -> BoxFuture<'a, bool>;

    /// The file's length. The defaults of the next three read the whole file with `get`; a store
    /// that can seek and stream (the local directory does) overrides them.
    fn size<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Option<u64>> {
        Box::pin(async move { self.get(path).await.map(|b| b.len() as u64) })
    }
    /// `len` bytes from `start`, or fewer at the end of the file.
    fn get_range<'a>(&'a self, path: &'a str, start: u64, len: u64) -> BoxFuture<'a, Option<Vec<u8>>> {
        Box::pin(async move {
            let all = self.get(path).await?;
            let from = usize::try_from(start).ok()?.min(all.len());
            let to = usize::try_from(start.saturating_add(len)).unwrap_or(usize::MAX).min(all.len());
            Some(all[from..to].to_vec())
        })
    }
    /// The whole file as a response body.
    fn stream<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Option<axum::body::Body>> {
        Box::pin(async move { self.get(path).await.map(axum::body::Body::from) })
    }
}

pub type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

struct LocalStore;

impl UploadStore for LocalStore {
    fn put<'a>(&'a self, path: &'a str, bytes: Vec<u8>) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let full = DIR.join(path);
            if tokio::fs::try_exists(&full).await.unwrap_or(false) {
                return Ok(());
            }
            let parent = full.parent().ok_or("no parent")?;
            tokio::fs::create_dir_all(parent).await.map_err(|e| e.to_string())?;
            // Write then rename, so a crash never leaves a half-written file under the final name.
            let tmp = full.with_extension(format!("tmp{}", std::process::id()));
            tokio::fs::write(&tmp, bytes).await.map_err(|e| e.to_string())?;
            tokio::fs::rename(&tmp, &full).await.map_err(|e| e.to_string())
        })
    }
    fn get<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Option<Vec<u8>>> {
        Box::pin(async move { tokio::fs::read(DIR.join(path)).await.ok() })
    }
    fn delete<'a>(&'a self, path: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async move { tokio::fs::remove_file(DIR.join(path)).await.is_ok() })
    }
    fn size<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Option<u64>> {
        Box::pin(async move { tokio::fs::metadata(DIR.join(path)).await.ok().map(|m| m.len()) })
    }
    fn get_range<'a>(&'a self, path: &'a str, start: u64, len: u64) -> BoxFuture<'a, Option<Vec<u8>>> {
        Box::pin(async move {
            use tokio::io::{AsyncReadExt, AsyncSeekExt};
            let mut file = tokio::fs::File::open(DIR.join(path)).await.ok()?;
            file.seek(std::io::SeekFrom::Start(start)).await.ok()?;
            let mut out = Vec::new();
            file.take(len).read_to_end(&mut out).await.ok()?;
            Some(out)
        })
    }
    fn stream<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Option<axum::body::Body>> {
        Box::pin(async move {
            let file = tokio::fs::File::open(DIR.join(path)).await.ok()?;
            let chunks = futures_util::stream::unfold(file, |mut file| async move {
                use tokio::io::AsyncReadExt;
                let mut buf = vec![0u8; 64 * 1024];
                match file.read(&mut buf).await {
                    Ok(0) => None,
                    Ok(n) => {
                        buf.truncate(n);
                        Some((Ok(axum::body::Bytes::from(buf)), file))
                    }
                    Err(e) => Some((Err(e), file)),
                }
            });
            // Fused: the compression layer polls a body once more after its end, and an unfold
            // stream panics when that happens (a text file downloaded by a signed-in member).
            Some(axum::body::Body::from_stream(futures_util::StreamExt::fuse(chunks)))
        })
    }
}

static STORE: std::sync::OnceLock<Box<dyn UploadStore>> = std::sync::OnceLock::new();

/// False if a store is already in use.
/// `bytes=a-b`, `bytes=a-` or `bytes=-n` as an inclusive (start, end) within `len`.
fn byte_range(header: &str, len: usize) -> Option<(usize, usize)> {
    let spec = header.strip_prefix("bytes=")?;
    if spec.contains(',') || len == 0 {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    let (start, end) = match (a.trim(), b.trim()) {
        ("", n) => {
            let n: usize = n.parse().ok()?;
            (len.saturating_sub(n), len - 1)
        }
        (a, "") => (a.parse().ok()?, len - 1),
        (a, b) => (a.parse().ok()?, b.parse::<usize>().ok()?.min(len - 1)),
    };
    (start <= end && end < len).then_some((start, end))
}

pub fn set_store(store: Box<dyn UploadStore>) -> bool {
    STORE.set(store).is_ok()
}

pub(crate) fn byte_store() -> &'static dyn UploadStore {
    STORE.get_or_init(|| Box::new(LocalStore)).as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_links_are_found_by_their_extension() {
        let a = "a".repeat(64);
        let raw = format!(
            "see ![x](/uploads/{a}.png#10x10) and [f](https://h.test/uploads/{a}.PDF) /uploads/{a}.zip, /uploads/short.png"
        );
        assert_eq!(upload_refs(&raw), vec!["png", "pdf", "zip"]);
        assert!(upload_refs("no links /uploads/ here").is_empty());
    }

    #[test]
    fn sniffing_ignores_names_and_rejects_scripts() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\nrest", "evil.html").map(|s| s.0), Some("image/png"));
        assert!(sniff(b"<svg onload=alert(1)>", "a.svg").is_none());
        assert!(sniff(b"<html><script>", "a.txt").map(|s| s.0) == Some("text/plain; charset=utf-8"));
        assert!(sniff(b"<html><script>", "a.html").is_none());
        assert!(sniff(b"MZ\x90\x00", "setup.exe").is_none());
        assert!(sniff(b"ab\0cd", "a.txt").is_none());
    }

    #[test]
    fn keys_and_contexts() {
        let sha = "a".repeat(64);
        assert!(valid_key(&format!("{sha}.png")));
        assert!(!valid_key(&format!("{sha}.svg")));
        assert!(!valid_key("../../etc/passwd"));
        assert!(!valid_key(&format!("{}.png", "A".repeat(64))));
        assert!(valid_context("reply:12") && valid_context("new-topic:3"));
        assert!(!valid_context("reply:") && !valid_context("pm:1") && !valid_context("reply:1x"));
    }

    async fn multipart(file: &[u8]) -> Multipart {
        use axum::extract::FromRequest;
        let mut body = b"--X\r\nContent-Disposition: form-data; name=\"csrf\"\r\n\r\ntok\r\n--X\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.png\"\r\n\r\n".to_vec();
        body.extend_from_slice(file);
        body.extend_from_slice(b"\r\n--X--\r\n");
        let req = axum::http::Request::builder()
            .header(header::CONTENT_TYPE, "multipart/form-data; boundary=X")
            .body(axum::body::Body::from(body))
            .unwrap();
        Multipart::from_request(req, &()).await.unwrap()
    }

    #[tokio::test]
    async fn a_file_over_the_limit_is_refused_while_it_is_read() {
        let big = vec![7u8; 3 * 1024];
        assert!(matches!(read_multipart_max(multipart(&big).await, 2).await, Err(AppError::Invalid { .. })));
        let (name, bytes, fields) = read_multipart_max(multipart(&big).await, 3).await.unwrap();
        assert_eq!((name.as_str(), bytes.len()), ("a.png", big.len()));
        assert_eq!(fields, [("csrf".to_owned(), "tok".to_owned())]);
    }

    /// A store that only has `get`: the seek and stream methods fall back to it.
    struct Mem(Vec<u8>);
    impl UploadStore for Mem {
        fn put<'a>(&'a self, _: &'a str, _: Vec<u8>) -> BoxFuture<'a, Result<(), String>> {
            Box::pin(async { Ok(()) })
        }
        fn get<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Option<Vec<u8>>> {
            Box::pin(async { Some(self.0.clone()) })
        }
        fn delete<'a>(&'a self, _: &'a str) -> BoxFuture<'a, bool> {
            Box::pin(async { true })
        }
    }

    #[tokio::test]
    async fn stores_without_seeking_still_serve_ranges() {
        let s = Mem((0u8..100).collect());
        assert_eq!(s.size("x").await, Some(100));
        assert_eq!(s.get_range("x", 10, 5).await.unwrap(), [10, 11, 12, 13, 14]);
        assert_eq!(s.get_range("x", 98, 50).await.unwrap(), [98, 99]);
        let body = axum::body::to_bytes(s.stream("x").await.unwrap(), usize::MAX).await.unwrap();
        assert_eq!(body.len(), 100);
    }

    #[test]
    fn filenames_are_cleaned() {
        assert_eq!(clean_filename("C:\\Users\\x\\report.pdf"), "report.pdf");
        assert_eq!(clean_filename("../../a<b>?.pdf"), "ab.pdf");
        assert_eq!(clean_filename("  "), "file");
    }
}

#[cfg(test)]
mod pipeline_tests;
