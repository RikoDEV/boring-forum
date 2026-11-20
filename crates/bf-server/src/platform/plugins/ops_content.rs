//! Operations on content: tags, forums, metadata, edits and files (capabilities `tags`, `nodes`,
//! `meta`, `content.write` and `files`). Everything a plugin stores here counts toward its
//! `plugin_storage_mb`.

use crate::http::render::pool;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use bf_core::AppError;
use bf_core::i18n::en;
use bf_db::platform::{plugin_content as pc, plugins, registry_content as rc};
use serde_json::{Value, json};

use super::host::storage_check;
use super::ops::failed;
use super::stats::allow;

const MAX_META_BYTES: usize = 8 * 1024;
const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;
const MAX_FILES: i64 = 1000;

/// `tags.set`: the plugin's own tags (by key) on a topic. Other tags are left alone.
async fn tags_set(state: &AppState, plugin: &str, req: &Value) -> Result<Value, String> {
    let pool = pool(state).map_err(failed)?;
    let topic = req.get("topic_id").and_then(Value::as_i64).unwrap_or_default();
    let keys: Vec<String> = super::ops::list(req, "keys");
    let own = rc::tag_keys(pool, plugin).await.map_err(failed)?;
    if let Some(bad) = keys.iter().find(|k| !own.contains(k)) {
        return Err(format!("{} {bad}", en("This plugin declares no tag")));
    }
    if !rc::set_topic_tags(pool, plugin, topic, &keys).await.map_err(failed)? {
        return Err(en("No such topic.").into());
    }
    state.microcache.clear();
    Ok(json!({ "tags": keys }))
}

/// `nodes.create`: a forum under one of the plugin's own declared forums.
async fn nodes_create(state: &AppState, plugin: &str, req: &Value) -> Result<Value, String> {
    let pool = pool(state).map_err(failed)?;
    let parent = req.get("parent").and_then(Value::as_str).unwrap_or_default();
    let title = req.get("title").and_then(Value::as_str).unwrap_or_default().trim();
    let description = req.get("description").and_then(Value::as_str).unwrap_or_default();
    if title.is_empty() || title.chars().count() > 100 || description.chars().count() > 500 {
        return Err(en("The title must be 1 to 100 characters.").into());
    }
    let id = rc::create_dynamic_node(pool, plugin, parent, title, description)
        .await
        .map_err(failed)?
        .ok_or(en("The parent is not one of this plugin's forums, or it has made too many."))?;
    state.forum.invalidate();
    state.microcache.clear();
    Ok(json!({ "id": id }))
}

/// The metadata keys the plugin declared, as (target, key).
async fn declared_meta(state: &AppState, plugin: &str) -> Vec<(String, String)> {
    let Some(pool) = state.pool.as_ref() else { return vec![] };
    let rows: Option<Value> = sqlx::query_scalar("SELECT schema->'x-reg'->'meta' FROM plugins WHERE id = $1")
        .bind(plugin)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    rows.and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|d| Some((d.get("target")?.as_str()?.to_owned(), d.get("key")?.as_str()?.to_owned())))
        .collect()
}

fn meta_target(req: &Value) -> Result<(&str, i64), String> {
    let target = req.get("target").and_then(Value::as_str).unwrap_or("topic");
    let id = req.get("id").and_then(Value::as_i64).unwrap_or_default();
    if !["topic", "post", "user"].contains(&target) || id <= 0 {
        return Err(en("Give a target (topic, post or user) and its id.").into());
    }
    Ok((target, id))
}

/// `meta.get`: the plugin's metadata on a topic or post, one key or all.
async fn meta_get(state: &AppState, plugin: &str, req: &Value) -> Result<Value, String> {
    let pool = pool(state).map_err(failed)?;
    let (target, id) = meta_target(req)?;
    let key = req.get("key").and_then(Value::as_str);
    Ok(json!({ "meta": rc::meta_get(pool, plugin, target, id, key).await.map_err(failed)? }))
}

/// `meta.set`: one declared key, up to 8 KB of JSON; `null` removes it.
async fn meta_set(state: &AppState, plugin: &str, req: &Value) -> Result<Value, String> {
    let pool = pool(state).map_err(failed)?;
    let (target, id) = meta_target(req)?;
    let key = req.get("key").and_then(Value::as_str).unwrap_or_default();
    if !declared_meta(state, plugin).await.iter().any(|(t, k)| t == target && k == key) {
        return Err(en("The plugin did not declare that metadata key.").into());
    }
    let value = req.get("value").filter(|v| !v.is_null());
    if let Some(v) = value {
        if v.to_string().len() > MAX_META_BYTES {
            return Err(en("The value is too large (8 KB at most).").into());
        }
        storage_check(state, plugin).await?;
    }
    if !rc::meta_set(pool, plugin, target, id, key, value).await.map_err(failed)? {
        return Err(en("No such topic, post or member.").into());
    }
    Ok(json!({ "ok": true }))
}

/// `content.edit`: `{topic, number?, body?, reason?, title?, locked?, listed?}`.
/// - `body` (and `number`, default 1): the text of a post the plugin's own account wrote; the old
///   text is kept as a revision.
/// - `title`, `locked` and `listed`: on a topic the plugin owns, that is one in a forum the plugin
///   declared or created, or one its account opened. The plugin never edits another member's words.
async fn content_edit(state: &AppState, plugin: &str, req: &Value) -> Result<Value, String> {
    let pool = pool(state).map_err(failed)?;
    let topic = req.get("topic").and_then(Value::as_i64).unwrap_or_default();
    let number = req.get("number").and_then(Value::as_i64).and_then(|n| i32::try_from(n).ok()).unwrap_or(1);
    let body = req.get("body").and_then(Value::as_str);
    let title = req.get("title").and_then(Value::as_str).map(str::trim);
    let (locked, listed) = (req.get("locked").and_then(Value::as_bool), req.get("listed").and_then(Value::as_bool));
    if body.is_none() && title.is_none() && locked.is_none() && listed.is_none() {
        return Err(en("Send a body, a title, locked or listed.").into());
    }
    let bot = plugins::bot_user(pool, plugin).await.map_err(failed)?.ok_or(en("This plugin has no account."))?;
    let owned = pc::topic_owned(pool, plugin, bot, topic).await.map_err(failed)?;
    if let Some(title) = title
        && (title.chars().count() < 3 || title.chars().count() > 200)
    {
        return Err(en("The title must be 3 to 200 characters.").into());
    }
    if let Some(body) = body {
        if body.trim().is_empty() || body.chars().count() > 20_000 {
            return Err(en("The body must be 1 to 20000 characters.").into());
        }
        let post = rc::own_post(pool, bot, topic, number)
            .await
            .map_err(failed)?
            .ok_or(en("The plugin can only edit its own posts."))?;
        let cooked = super::cook(state, body).await;
        let reason = req.get("reason").and_then(Value::as_str).map(|r| r.chars().take(100).collect::<String>());
        bf_db::forum::posts::edit_post(pool, post, bot, body, &cooked.html, bf_cook::COOK_VERSION, reason.as_deref())
            .await
            .map_err(failed)?;
    }
    if title.is_some() || locked.is_some() || listed.is_some() {
        if !owned {
            return Err(en("The plugin can only change topics it owns.").into());
        }
        if let Some(title) = title {
            sqlx::query("UPDATE items SET title = $2 WHERE id = $1")
                .bind(topic)
                .bind(title)
                .execute(pool)
                .await
                .map_err(failed)?;
        }
        pc::set_topic_flags(pool, topic, locked, listed).await.map_err(failed)?;
    }
    state.forum.invalidate();
    state.microcache.clear();
    Ok(json!({ "edited": true }))
}

/// What a plugin's file may be. Nothing that a browser would run as a page on this origin.
const FILE_TYPES: [&str; 22] = [
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "image/avif",
    "image/svg+xml",
    "application/pdf",
    "text/plain",
    "text/csv",
    "text/markdown",
    "application/json",
    "audio/mpeg",
    "audio/ogg",
    "video/mp4",
    "video/webm",
    "application/zip",
    "application/gzip",
    "application/x-tar",
    "application/epub+zip",
    "application/octet-stream",
    "application/x-7z-compressed",
    "font/woff2",
];

/// Types the browser shows in place; every other stored file is offered as a download.
fn shown_inline(ty: &str) -> bool {
    ty.starts_with("image/")
        || ty.starts_with("audio/")
        || ty.starts_with("video/")
        || ty.starts_with("text/")
        || ty == "application/pdf"
        || ty == "application/json"
}

fn good_path(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 120
        && !p.starts_with('/')
        && !p.ends_with('/')
        && !p.contains("..")
        && !p.contains("//")
        && p.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'-' | b'_' | b'/'))
}

/// Refuses data that would take a plugin past `plugin_storage_mb`, counting the `extra` bytes
/// about to be written: one big file cannot overshoot the quota by its own size.
async fn quota_for(state: &AppState, plugin: &str, extra: usize) -> Result<(), String> {
    let full = || en("This plugin has used all its storage.").to_owned();
    let pool = pool(state).map_err(|_| full())?;
    let used = plugins::storage_used(pool, plugin).await.map_err(|_| full())?;
    let limit = state.settings().int("plugin_storage_mb").saturating_mul(1024 * 1024);
    if used.saturating_add(i64::try_from(extra).unwrap_or(i64::MAX)) > limit {
        return Err(full());
    }
    Ok(())
}

/// `file.put`: `{path, content_type, text | base64, append}`. A request holds 64 KB, so a larger
/// file (up to 8 MB) is sent in pieces with `append: true`, which adds in the database without
/// reading the file back. `plugin_storage_mb` is the quota for everything a plugin stores.
async fn file_put(state: &AppState, plugin: &str, req: &Value) -> Result<Value, String> {
    let pool = pool(state).map_err(failed)?;
    let path = req.get("path").and_then(Value::as_str).unwrap_or_default();
    let ty = req.get("content_type").and_then(Value::as_str).unwrap_or_default();
    if !good_path(path) || !FILE_TYPES.contains(&ty) {
        return Err(en(
            "Use a lowercase path of letters, digits, dots, dashes and slashes, and an allowed content type.",
        )
        .into());
    }
    let data = match (req.get("text").and_then(Value::as_str), req.get("base64").and_then(Value::as_str)) {
        (Some(t), _) => t.as_bytes().to_vec(),
        (None, Some(b)) => base64::engine::general_purpose::STANDARD
            .decode(b)
            .map_err(|_| en("The base64 is not valid.").to_owned())?,
        (None, None) => return Err(en("Send text or base64.").into()),
    };
    let append = req.get("append").and_then(Value::as_bool).unwrap_or(false);
    let head = pc::file_head(pool, plugin, path).await.map_err(failed)?;
    let old = head.as_ref().map_or(0, |h| usize::try_from(h.1).unwrap_or(usize::MAX));
    let total = if append { old.saturating_add(data.len()) } else { data.len() };
    if total > MAX_FILE_BYTES {
        return Err(en("A file may be 8 MB at most.").into());
    }
    if head.is_none() && rc::file_count(pool, plugin).await.map_err(failed)? >= MAX_FILES {
        return Err(en("This plugin keeps too many files.").into());
    }
    quota_for(state, plugin, data.len()).await?;
    let size = if append {
        pc::file_append(pool, plugin, path, ty, &data).await.map_err(failed)?
    } else {
        rc::file_put(pool, plugin, path, ty, &data).await.map_err(failed)?;
        i64::try_from(data.len()).unwrap_or(i64::MAX)
    };
    Ok(json!({ "url": format!("/plugin-files/{plugin}/{path}"), "size": size }))
}

/// `file.fetch`: `{url, path, content_type}` stores what an approved host (`http:<host>`) serves,
/// up to 8 MB, through the SSRF-safe fetcher. The type is the plugin's to declare, never the
/// remote server's.
pub(super) async fn file_fetch(state: &AppState, plugin: &str, hosts: &[String], req: &Value) -> Result<Value, String> {
    let url = req.get("url").and_then(Value::as_str).unwrap_or_default();
    if !super::http_allowed(hosts, url) {
        return Err(en("This plugin may not call that host.").into());
    }
    let path = req.get("path").and_then(Value::as_str).unwrap_or_default();
    let ty = req.get("content_type").and_then(Value::as_str).unwrap_or_default();
    if !good_path(path) || !FILE_TYPES.contains(&ty) {
        return Err(en(
            "Use a lowercase path of letters, digits, dots, dashes and slashes, and an allowed content type.",
        )
        .into());
    }
    if !allow(plugin, "http", 60, state.settings().int("plugin_http_per_minute"), 1) {
        return Err(en("This plugin has made too many requests in the last minute.").into());
    }
    let pool = pool(state).map_err(failed)?;
    if rc::file_get(pool, plugin, path).await.map_err(failed)?.is_none()
        && rc::file_count(pool, plugin).await.map_err(failed)? >= MAX_FILES
    {
        return Err(en("This plugin keeps too many files.").into());
    }
    quota_for(state, plugin, MAX_FILE_BYTES).await?;
    let fetched = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        crate::platform::outbound::fetch_public(url, MAX_FILE_BYTES),
    )
    .await;
    let Ok(Ok(bytes)) = fetched else { return Err(en("The request failed.").into()) };
    rc::file_put(pool, plugin, path, ty, &bytes).await.map_err(failed)?;
    Ok(json!({ "url": format!("/plugin-files/{plugin}/{path}"), "size": bytes.len() }))
}

async fn file_get(state: &AppState, plugin: &str, req: &Value) -> Result<Value, String> {
    let pool = pool(state).map_err(failed)?;
    let path = req.get("path").and_then(Value::as_str).unwrap_or_default();
    let (ty, data) = rc::file_get(pool, plugin, path).await.map_err(failed)?.ok_or(en("No such file."))?;
    // The answer must fit what a module can read back in one call.
    if data.len() > 48 * 1024 {
        return Err(en("The file is too large to read in one call; link to its address instead.").into());
    }
    Ok(
        json!({ "content_type": ty, "base64": base64::engine::general_purpose::STANDARD.encode(&data), "size": data.len() }),
    )
}

async fn file_delete(state: &AppState, plugin: &str, req: &Value) -> Result<Value, String> {
    let pool = pool(state).map_err(failed)?;
    let path = req.get("path").and_then(Value::as_str).unwrap_or_default();
    Ok(json!({ "deleted": rc::file_delete(pool, plugin, path).await.map_err(failed)? }))
}

pub(super) async fn more(state: &AppState, plugin: &str, op: &str, req: &Value) -> Option<Result<Value, String>> {
    Some(match op {
        "tags.set" => tags_set(state, plugin, req).await,
        "nodes.create" => nodes_create(state, plugin, req).await,
        "meta.get" => meta_get(state, plugin, req).await,
        "meta.set" => meta_set(state, plugin, req).await,
        "content.edit" => content_edit(state, plugin, req).await,
        "file.put" => file_put(state, plugin, req).await,
        "file.get" => file_get(state, plugin, req).await,
        "file.delete" => file_delete(state, plugin, req).await,
        "search.index" | "search.remove" | "review.hold" | "review.withdraw" | "content.can" => {
            let types = super::types_of(plugin);
            return super::content_types::more(state, plugin, &types, op, req).await;
        }
        _ => return None,
    })
}

fn header_map(list: &[(header::HeaderName, String)]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (k, v) in list {
        if let Ok(v) = HeaderValue::from_str(v) {
            map.insert(k.clone(), v);
        }
    }
    map
}

/// One `Range` header: `Ok(Some((first, last)))` for a single satisfiable range, `Ok(None)` when it
/// is absent or asks for several (the whole file is sent), `Err(())` when it cannot be satisfied.
fn byte_range(header: Option<&str>, size: i64) -> Result<Option<(i64, i64)>, ()> {
    let Some(spec) = header.and_then(|h| h.strip_prefix("bytes=")) else { return Ok(None) };
    if spec.contains(',') || size == 0 {
        return if size == 0 { Err(()) } else { Ok(None) };
    }
    let (from, to) = spec.split_once('-').ok_or(())?;
    let (first, last) = match (from.trim(), to.trim()) {
        ("", n) => {
            let n: i64 = n.parse().map_err(|_| ())?;
            if n == 0 {
                return Err(());
            }
            ((size - n).max(0), size - 1)
        }
        (a, "") => (a.parse::<i64>().map_err(|_| ())?, size - 1),
        (a, b) => (a.parse::<i64>().map_err(|_| ())?, b.parse::<i64>().map_err(|_| ())?.min(size - 1)),
    };
    if first < 0 || first > last || first >= size {
        return Err(());
    }
    Ok(Some((first, last)))
}

/// `/plugin-files/<id>/<path>`: a file a plugin stored. Public, like an upload; served with no
/// way to run script (sandboxed, `nosniff`) and only the types the plugin was allowed to store.
/// Supports `Range` (one range; seeking in audio and video, resuming a download), `ETag` and
/// `If-None-Match`. Types a browser would not show are sent as an attachment; `?download=1` asks
/// for that for any.
pub(super) async fn serve_file(
    State(state): State<AppState>,
    Path((id, path)): Path<(String, String)>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
    headers: axum::http::HeaderMap,
) -> Result<Response, AppError> {
    let on = super::ACTIVE
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .any(|p| p.id == id && p.capabilities.iter().any(|c| c == "files"));
    if !on {
        return Err(AppError::NotFound);
    }
    let pool = state.db()?;
    let (ty, size, stored) =
        pc::file_head(pool, &id, &path).await.map_err(AppError::internal)?.ok_or(AppError::NotFound)?;
    let etag = format!("\"{size}-{}\"", stored.timestamp());
    let mut res_headers = vec![
        (header::CONTENT_TYPE, ty.clone()),
        (header::ACCEPT_RANGES, "bytes".to_owned()),
        (header::ETAG, etag.clone()),
        (header::CACHE_CONTROL, "public, max-age=300".to_owned()),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()),
        (header::CONTENT_SECURITY_POLICY, "default-src 'none'; style-src 'unsafe-inline'; sandbox".to_owned()),
        (header::HeaderName::from_static("cross-origin-resource-policy"), "same-origin".to_owned()),
    ];
    if q.contains_key("download") || !shown_inline(&ty) {
        let name = path.rsplit('/').next().unwrap_or("file");
        res_headers.push((header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\"")));
    }
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|m| m.split(',').any(|t| t.trim() == etag))
    {
        return Ok((StatusCode::NOT_MODIFIED, header_map(&res_headers)).into_response());
    }
    let wanted = headers.get(header::RANGE).and_then(|h| h.to_str().ok());
    match byte_range(wanted, size) {
        Err(()) => {
            Ok((StatusCode::RANGE_NOT_SATISFIABLE, [(header::CONTENT_RANGE, format!("bytes */{size}"))])
                .into_response())
        }
        Ok(Some((first, last))) => {
            let data = pc::file_slice(pool, &id, &path, first, last - first + 1)
                .await
                .map_err(AppError::internal)?
                .ok_or(AppError::NotFound)?;
            res_headers.push((header::CONTENT_RANGE, format!("bytes {first}-{last}/{size}")));
            Ok((StatusCode::PARTIAL_CONTENT, header_map(&res_headers), data).into_response())
        }
        Ok(None) => {
            let (_, data) =
                rc::file_get(pool, &id, &path).await.map_err(AppError::internal)?.ok_or(AppError::NotFound)?;
            Ok((header_map(&res_headers), data).into_response())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_in_the_plugin_folder() {
        assert!(good_path("img/logo.png") && good_path("a.txt"));
        for bad in ["", "/a", "a/", "../a", "a/../b", "a//b", "A.png", "a b", "a\\b"] {
            assert!(!good_path(bad), "{bad}");
        }
    }

    #[test]
    fn ranges_are_parsed_like_the_spec_says() {
        assert_eq!(byte_range(Some("bytes=0-99"), 1000), Ok(Some((0, 99))));
        assert_eq!(byte_range(Some("bytes=900-"), 1000), Ok(Some((900, 999))));
        assert_eq!(byte_range(Some("bytes=-100"), 1000), Ok(Some((900, 999))));
        assert_eq!(byte_range(Some("bytes=990-5000"), 1000), Ok(Some((990, 999))));
        assert_eq!(byte_range(Some("bytes=1000-"), 1000), Err(()));
        assert_eq!(byte_range(Some("bytes=5-1"), 1000), Err(()));
        assert_eq!(byte_range(Some("bytes=0-1,5-9"), 1000), Ok(None), "several ranges: the whole file");
        assert_eq!(byte_range(None, 1000), Ok(None));
        assert_eq!(byte_range(Some("items=1-2"), 1000), Ok(None));
    }
}
