//! Permalinks (ACP → Permalinks): redirects for addresses the forum does not know, so the links
//! of a forum moved here from elsewhere keep working. An address that ends in a 404 is looked up
//! here, and when it has a permalink the visitor is sent on with a permanent redirect.

use crate::admin::api::{Admin, AdminRead, AdminWrite};
use crate::http::render::DbResult;
use crate::state::AppState;
use axum::extract::{Path, Query, Request, State};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use bf_core::AppError;
use bf_core::slug::{forum_path, profile_path, topic_path};
use bf_db::admin::permalinks::{self, Permalink};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/admin/permalinks", get(list).post(create))
        .route("/api/v1/admin/permalinks/{id}", put(update).delete(remove))
}

// --- addresses

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// An address as it is kept and compared: no scheme or host, no leading or trailing slash on the
/// path, no fragment, percent-escapes undone. The query string, if any, stays.
fn normalise(raw: &str) -> String {
    let raw = raw.split('#').next().unwrap_or("");
    let (path, query) = match raw.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (raw, None),
    };
    let path = decode(path);
    let path = path.trim_matches('/');
    match query.map(decode).filter(|q| !q.is_empty()) {
        Some(q) => format!("{path}?{q}"),
        None => path.to_owned(),
    }
}

/// The address an admin typed, ready to keep; the host of a full address must be this site's.
fn clean_url(raw: &str, site_url: &str) -> Result<String, AppError> {
    let raw = raw.trim();
    let own =
        url::Url::parse(site_url).ok().and_then(|u| u.host_str().map(str::to_ascii_lowercase)).unwrap_or_default();
    let local = if raw.starts_with("http://") || raw.starts_with("https://") {
        let u = url::Url::parse(raw).map_err(|_| AppError::Validation("That address is not valid.".into()))?;
        let host = u.host_str().unwrap_or("").to_ascii_lowercase();
        if host.trim_start_matches("www.") != own.trim_start_matches("www.") {
            return Err(AppError::Validation("A permalink is an address on this site, like c/old-forum/12.".into()));
        }
        format!("{}{}", u.path(), u.query().map(|q| format!("?{q}")).unwrap_or_default())
    } else {
        raw.to_owned()
    };
    let url = normalise(&local);
    let first = url.split(['/', '?']).next().unwrap_or("");
    if url.is_empty() || url.chars().count() > 500 || url.chars().any(char::is_control) {
        return Err(AppError::Validation("A permalink needs an address of up to 500 characters.".into()));
    }
    // These are never looked up, so a permalink there would do nothing.
    if matches!(first, "api" | "admin") {
        return Err(AppError::Validation("Permalinks do not apply to the API or the admin panel.".into()));
    }
    Ok(url)
}

/// The addresses to look up for a request: with its query string first, then without.
fn request_keys(path: &str, query: Option<&str>) -> Vec<String> {
    let bare = normalise(path);
    let mut keys = Vec::new();
    if let Some(q) = query.filter(|q| !q.is_empty()) {
        keys.push(normalise(&format!("{path}?{q}")));
    }
    keys.push(bare);
    keys
}

// --- where a permalink leads

struct Lookup {
    topics: HashMap<i64, (String, String)>,
    posts: HashMap<i64, (i64, String)>,
}

async fn lookup(state: &AppState, rows: &[Permalink]) -> Result<Lookup, AppError> {
    let ids = |kind: &str| rows.iter().filter(|r| r.kind == kind).filter_map(|r| r.target_id).collect::<Vec<_>>();
    let pool = state.db()?;
    Ok(Lookup {
        topics: permalinks::topics(pool, &ids("topic"))
            .await
            .db()?
            .into_iter()
            .map(|(id, slug, title)| (id, (slug, title)))
            .collect(),
        posts: permalinks::posts(pool, &ids("post"))
            .await
            .db()?
            .into_iter()
            .map(|(id, topic, title)| (id, (topic, title)))
            .collect(),
    })
}

/// Where a permalink sends a visitor, and a name for it; None when what it points at is gone.
async fn destination(state: &AppState, p: &Permalink, known: &Lookup) -> Result<Option<(String, String)>, AppError> {
    let id = p.target_id.unwrap_or(0);
    let text = p.target.clone().unwrap_or_default();
    Ok(match p.kind.as_str() {
        "forum" => {
            let tree = state.forum.get(state.db()?).await?;
            tree.nodes.iter().find(|n| n.id == id).map(|n| (forum_path(n.id, &n.slug), n.title.clone()))
        }
        "topic" => known.topics.get(&id).map(|(slug, title)| (topic_path(id, slug), title.clone())),
        "post" => known.posts.get(&id).map(|(_, title)| (format!("/p/{id}"), title.clone())),
        "tag" => permalinks::tag_exists(state.db()?, &text).await.db()?.then(|| (format!("/tag/{text}"), text.clone())),
        "member" => permalinks::member_name(state.db()?, &text)
            .await
            .db()?
            .map(|name| (profile_path(0, &name), format!("@{name}"))),
        "url" => Some((text.clone(), text)),
        _ => None,
    })
}

/// Outside the page cache: an address that ended in a 404 and has a permalink is redirected for good.
pub(crate) async fn layer(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let browsing = matches!(*req.method(), Method::GET | Method::HEAD);
    let path = req.uri().path().to_owned();
    let query = req.uri().query().map(str::to_owned);
    let res = next.run(req).await;
    if res.status() != StatusCode::NOT_FOUND || !browsing || path.starts_with("/api/") || path.starts_with("/admin") {
        return res;
    }
    let Some(db) = &state.pool else { return res };
    let Ok(Some(p)) = permalinks::find(db, &request_keys(&path, query.as_deref())).await else { return res };
    let Ok(known) = lookup(&state, std::slice::from_ref(&p)).await else { return res };
    match destination(&state, &p, &known).await {
        // A permalink that points at its own address would go round for ever.
        Ok(Some((to, _))) if normalise(&to) != normalise(&path) => match axum::http::HeaderValue::from_str(&to) {
            // 301, as a move between forums is announced to search engines.
            Ok(location) => (StatusCode::MOVED_PERMANENTLY, [(axum::http::header::LOCATION, location)]).into_response(),
            Err(_) => res,
        },
        _ => res,
    }
}

// --- the admin API

#[derive(Deserialize)]
struct SearchQuery {
    #[serde(default)]
    q: String,
}

async fn list(
    State(state): State<AppState>,
    Admin(..): AdminRead,
    Query(q): Query<SearchQuery>,
) -> Result<Response, AppError> {
    let rows = permalinks::list(state.db()?, q.q.trim(), 500).await.db()?;
    let known = lookup(&state, &rows).await?;
    let mut out = Vec::with_capacity(rows.len());
    for p in &rows {
        let dest = destination(&state, p, &known).await?;
        out.push(json!({
            "id": p.id, "url": p.url, "kind": p.kind, "target_id": p.target_id, "target": p.target,
            "destination": dest.as_ref().map(|d| &d.0), "label": dest.as_ref().map(|d| &d.1), "missing": dest.is_none(),
        }));
    }
    Ok(Json(json!({ "permalinks": out, "total": permalinks::count(state.db()?).await.db()? })).into_response())
}

#[derive(Deserialize)]
struct Body {
    url: String,
    kind: String,
    target_id: Option<i64>,
    target: Option<String>,
}

/// The target of a permalink, checked against what exists: (id, text).
async fn target(state: &AppState, b: &Body, url: &str) -> Result<(Option<i64>, Option<String>), AppError> {
    let pool = state.db()?;
    let id = b.target_id.filter(|i| *i > 0);
    let text = b.target.as_deref().map(str::trim).filter(|t| !t.is_empty());
    let missing = || AppError::Validation("What the permalink points at does not exist.".into());
    match b.kind.as_str() {
        "forum" => {
            let id = id.ok_or_else(missing)?;
            let tree = state.forum.get(pool).await?;
            tree.nodes.iter().any(|n| n.id == id).then_some((Some(id), None)).ok_or_else(missing)
        }
        "topic" => {
            let id = id.ok_or_else(missing)?;
            if permalinks::topics(pool, &[id]).await.db()?.is_empty() { Err(missing()) } else { Ok((Some(id), None)) }
        }
        "post" => {
            let id = id.ok_or_else(missing)?;
            if permalinks::posts(pool, &[id]).await.db()?.is_empty() { Err(missing()) } else { Ok((Some(id), None)) }
        }
        "tag" => {
            // A tag by its name (what the picker offers) or its address name.
            let slug = permalinks::tag_slug(pool, text.ok_or_else(missing)?.trim_start_matches('#'))
                .await
                .db()?
                .ok_or_else(missing)?;
            Ok((None, Some(slug)))
        }
        "member" => {
            let name = permalinks::member_name(pool, text.ok_or_else(missing)?.trim_start_matches('@'))
                .await
                .db()?
                .ok_or_else(missing)?;
            Ok((None, Some(name)))
        }
        "url" => {
            let to = text.ok_or_else(|| AppError::Validation("Give the address to send visitors to.".into()))?;
            let ok_path = to.starts_with('/') && !to.starts_with("//");
            let ok_web =
                url::Url::parse(to).is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some());
            if to.chars().count() > 2000
                || to.chars().any(|c| c.is_control() || c == '"' || c == '<')
                || !(ok_path || ok_web)
            {
                return Err(AppError::Validation(
                    "Send visitors to an address on this site, like /c/forum/1, or to a web address.".into(),
                ));
            }
            if ok_path && normalise(to) == url {
                return Err(AppError::Validation("A permalink cannot point at its own address.".into()));
            }
            Ok((None, Some(to.to_owned())))
        }
        _ => Err(AppError::Validation("Choose what the permalink points at.".into())),
    }
}

async fn audit(state: &AppState, who: Option<i64>, action: &str, url: &str) -> Result<(), AppError> {
    crate::admin::api::audit_log(state.db()?, who, action, &format!("permalink:{url}"), json!({})).await
}

async fn create(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Json(b): Json<Body>,
) -> Result<Response, AppError> {
    let url = clean_url(&b.url, &state.config.base_url)?;
    if permalinks::url_taken(state.db()?, &url, None).await.db()? {
        return Err(AppError::Validation("That address already has a permalink.".into()));
    }
    let (id, text) = target(&state, &b, &url).await?;
    let made = permalinks::insert(state.db()?, &url, &b.kind, id, text.as_deref()).await.db()?;
    audit(&state, who, "permalink.add", &url).await?;
    Ok((StatusCode::CREATED, Json(made)).into_response())
}

async fn update(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path(id): Path<i64>,
    Json(b): Json<Body>,
) -> Result<Response, AppError> {
    let url = clean_url(&b.url, &state.config.base_url)?;
    if permalinks::url_taken(state.db()?, &url, Some(id)).await.db()? {
        return Err(AppError::Validation("That address already has a permalink.".into()));
    }
    let (target_id, text) = target(&state, &b, &url).await?;
    permalinks::update(state.db()?, id, &url, &b.kind, target_id, text.as_deref()).await.found()?;
    audit(&state, who, "permalink.update", &url).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn remove(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    permalinks::delete(state.db()?, id).await.found()?;
    audit(&state, who, "permalink.delete", &id.to_string()).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_kept_without_slashes_hosts_and_escapes() {
        assert_eq!(normalise("/c/forum/kosz/50/"), "c/forum/kosz/50");
        assert_eq!(normalise("c/caf%C3%A9"), "c/café");
        assert_eq!(normalise("/viewtopic.php?t=12&p=3#top"), "viewtopic.php?t=12&p=3");
        assert_eq!(normalise("/a/?"), "a");
        assert_eq!(clean_url("https://www.forum.example/c/old/12/", "https://forum.example").unwrap(), "c/old/12");
        assert!(clean_url("https://elsewhere.example/c/old", "https://forum.example").is_err());
        assert!(clean_url("", "https://forum.example").is_err());
        assert!(clean_url("/api/v1/x", "https://forum.example").is_err());
        assert!(clean_url("admin/forums", "https://forum.example").is_err());
    }

    #[test]
    fn a_request_looks_up_its_query_string_first() {
        assert_eq!(request_keys("/viewtopic.php", Some("t=12")), vec!["viewtopic.php?t=12", "viewtopic.php"]);
        assert_eq!(request_keys("/c/old/12/", None), vec!["c/old/12"]);
    }
}
