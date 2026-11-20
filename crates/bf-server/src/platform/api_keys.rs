//! API keys.

use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Json, Router};
use bf_core::AppError;
use bf_db::platform::api;
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/me", get(me))
        .route("/api/v1/me/api-keys", get(list_keys).post(create_key))
        .route("/api/v1/me/api-keys/{id}", delete(delete_key))
}

fn check_write(v: &Viewer, headers: &HeaderMap) -> Result<(), AppError> {
    v.check_csrf_headers(headers)
}

/// The caller, for checking an API key.
#[utoipa::path(get, path = "/api/v1/me", tag = "me",
    responses((status = 200, body = Object, description = "id, username, scope and the resource and plugin scopes of the key"), (status = 403, body = bf_core::error::Problem)))]
pub async fn me(State(state): State<AppState>, v: Viewer) -> Result<Response, AppError> {
    let u = v.require_user()?;
    let mut answer = json!({ "id": u.id, "username": u.username, "scope": v.api_scope, "scopes": v.plugin_scopes });
    crate::platform::plugins::add_plugin_fields(&state, "user", u.id.get(), &mut answer).await;
    Ok(Json(answer).into_response())
}

/// What a key can reach, besides `read`/`write`/`admin`: `read:<resource>` and `write:<resource>`
/// narrow a key to those parts of the API (`write` implies `read` of the same resource).
pub const RESOURCES: [&str; 9] =
    ["topics", "posts", "users", "search", "notifications", "messages", "chat", "uploads", "bookmarks"];

fn is_resource_scope(s: &str) -> bool {
    s.split_once(':').is_some_and(|(mode, r)| matches!(mode, "read" | "write") && RESOURCES.contains(&r))
}

/// The resource a request path belongs to, for the HTML routes and the JSON API alike.
pub(crate) fn resource_of(path: &str) -> Option<&'static str> {
    let rest = path.strip_prefix("/api/v1").unwrap_or(path).trim_start_matches('/');
    let mut seg = rest.split('/');
    let first = seg.next().unwrap_or_default();
    let second = seg.next().unwrap_or_default();
    let last = rest.rsplit('/').next().unwrap_or_default();
    Some(match first {
        // Replying is writing a post, though the route hangs off the topic.
        "t" if last == "reply" => "posts",
        "t" | "c" | "topics" | "forums" | "tag" | "tags" | "latest" | "top" | "unread" => "topics",
        "post" | "posts" => "posts",
        "u" | "users" | "g" => "users",
        "search" => "search",
        "notifications" => "notifications",
        "me" if second == "notifications" => "notifications",
        "me" if second == "messages" => "messages",
        "me" if second == "bookmarks" => "bookmarks",
        "messages" => "messages",
        "chat" => "chat",
        "uploads" => "uploads",
        _ => return None,
    })
}

/// Whether a key with these scopes may make this request. A key with no resource scopes is not
/// narrowed. A narrowed key may still ask who it is, read the public settings and call plugin
/// routes (a plugin checks its own scopes); everything else needs a matching resource scope.
pub fn resource_allowed(held: &[String], method: &axum::http::Method, path: &str) -> bool {
    let narrowed: Vec<&str> = held.iter().map(String::as_str).filter(|s| is_resource_scope(s)).collect();
    if narrowed.is_empty() {
        return true;
    }
    if path == "/api/v1/me" || path == "/api/v1/settings" || path.starts_with("/api/p/") {
        return true;
    }
    let Some(resource) = resource_of(path) else { return false };
    let reads = matches!(*method, axum::http::Method::GET | axum::http::Method::HEAD);
    let write = format!("write:{resource}");
    let read = format!("read:{resource}");
    narrowed.iter().any(|s| *s == write || (reads && *s == read))
}

#[derive(Deserialize, ToSchema)]
pub struct NewKey {
    name: String,
    /// `read`, `write`, or `admin` (administrators only).
    scope: String,
    expires_days: Option<i32>,
    /// `read:topics`, `write:posts`, ...: limits the key to these parts of the API. Resources:
    /// topics, posts, users, search, notifications, messages, chat, uploads, bookmarks.
    #[serde(default)]
    resources: Vec<String>,
    /// Scopes plugins declare (`plugin:<id>:<key>`), as listed in Settings.
    #[serde(default)]
    plugin_scopes: Vec<String>,
}

#[derive(Serialize, ToSchema)]
pub struct CreatedKey {
    id: i64,
    /// Shown once; only its digest is kept.
    token: String,
    /// Resource and plugin scopes the key holds.
    scopes: Vec<String>,
}

/// An admin key skips the sudo and CSRF checks a session needs, so it is short-lived.
const ADMIN_KEY_MAX_DAYS: i32 = 30;

fn key_lifetime(scope: &str, asked: Option<i32>) -> i32 {
    let days = api::key_days(asked);
    if scope == "admin" { days.min(ADMIN_KEY_MAX_DAYS) } else { days }
}

/// Creates a key; the token is shown once and only its digest is kept. Besides the base scope a
/// key can be limited to resources (`read:topics`) and given plugin scopes.
#[utoipa::path(post, path = "/api/v1/me/api-keys", tag = "me", request_body = NewKey,
    responses((status = 201, body = CreatedKey), (status = 403, body = bf_core::error::Problem), (status = 422, body = bf_core::error::Problem)))]
pub async fn create_key(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Json(b): Json<NewKey>,
) -> Result<Response, AppError> {
    let u = v.require_user()?;
    check_write(&v, &headers)?;
    // A key can never grant more than the session that made it.
    if v.api_scope.is_some() {
        return Err(AppError::Forbidden);
    }
    v.require_sudo()?;
    let name = b.name.trim();
    let allowed = matches!(b.scope.as_str(), "read" | "write") || (b.scope == "admin" && u.is_admin && !u.needs_2fa);
    if name.is_empty() || name.len() > 80 || !allowed {
        return Err(AppError::Validation("Give the key a name and a scope you hold: read, write or admin.".into()));
    }
    // An admin key is for the admin API as a whole, and a read key cannot hold a write scope.
    if b.resources.iter().any(|r| !is_resource_scope(r))
        || (!b.resources.is_empty() && b.scope == "admin")
        || (b.scope == "read" && b.resources.iter().any(|r| r.starts_with("write:")))
    {
        return Err(AppError::Validation("Unknown or unusable resource scope for this key.".into()));
    }
    let mut extra: Vec<String> = b.resources.clone();
    if !b.plugin_scopes.is_empty() {
        let held = bf_db::platform::registry_content::existing_scopes(state.db()?, &b.plugin_scopes).await.db()?;
        if held.len() != b.plugin_scopes.len() {
            return Err(AppError::Validation("Unknown plugin scope.".into()));
        }
        extra.extend(held);
    }
    extra.sort();
    extra.dedup();
    let token = format!("bf_{}", bf_core::auth::random_token());
    let id = api::create_key(
        state.db()?,
        u.id.get(),
        name,
        &bf_core::auth::token_digest(&token),
        &b.scope,
        key_lifetime(&b.scope, b.expires_days),
    )
    .await
    .db()?;
    // Resource and plugin scopes share one column; resource scopes never start with `plugin:`.
    if !extra.is_empty() {
        api::set_key_scopes(state.db()?, id, &extra).await.db()?;
    }
    Ok((StatusCode::CREATED, Json(CreatedKey { id, token, scopes: extra })).into_response())
}

#[utoipa::path(get, path = "/api/v1/me/api-keys", tag = "me",
    responses((status = 200, body = [Object], description = "The member's keys: id, name, scope, last_used_at, expires_at, plugin_scopes")))]
pub async fn list_keys(State(state): State<AppState>, v: Viewer) -> Result<Response, AppError> {
    let u = v.require_user()?;
    Ok(Json(api::keys(state.db()?, u.id.get()).await.db()?).into_response())
}

#[utoipa::path(delete, path = "/api/v1/me/api-keys/{id}", tag = "me", params(("id" = i64, Path)),
    responses((status = 204), (status = 404, body = bf_core::error::Problem)))]
pub async fn delete_key(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let u = v.require_user()?;
    check_write(&v, &headers)?;
    if !api::delete_key(state.db()?, u.id.get(), id).await.db()? {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    use super::key_lifetime;

    #[test]
    fn admin_keys_are_short_lived() {
        assert_eq!(key_lifetime("admin", None), 30);
        assert_eq!(key_lifetime("admin", Some(365)), 30);
        assert_eq!(key_lifetime("admin", Some(7)), 7);
        assert_eq!(key_lifetime("write", None), 90);
        assert_eq!(key_lifetime("read", Some(365)), 365);
    }

    #[test]
    fn resource_scopes_narrow_a_key() {
        use axum::http::Method;
        let held = vec!["read:topics".to_owned(), "write:posts".to_owned()];
        assert!(super::resource_allowed(&[], &Method::DELETE, "/post/1/delete"));
        assert!(super::resource_allowed(&held, &Method::GET, "/api/v1/topics/3"));
        assert!(!super::resource_allowed(&held, &Method::POST, "/t/3/move"));
        assert!(super::resource_allowed(&held, &Method::POST, "/t/3/reply"));
        assert!(super::resource_allowed(&held, &Method::PUT, "/api/v1/posts/3"));
        assert!(!super::resource_allowed(&held, &Method::GET, "/api/v1/users/ann"));
        assert!(super::resource_allowed(&held, &Method::GET, "/api/v1/me"));
        assert!(!super::resource_allowed(&held, &Method::GET, "/api/v1/admin/settings"));
    }
}
