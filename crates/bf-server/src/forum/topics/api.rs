//! The small JSON and beacon endpoints a topic page calls: link clicks, post fragments, live state, counts and suggestions.

use crate::http::render::DbResult;
use crate::http::render::layout;
use crate::http::render::page;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bf_core::AppError;
use bf_core::perms::Perms;
use bf_core::slug::topic_path;
use bf_db::accounts::users;
use bf_db::admin::emoji;
use bf_db::forum::{links, posts, reading};
use bf_web::pages::TopicPostsPart;
use serde::Deserialize;
use serde_json::json;

use super::{post_views, prepare_rows, readable_topic};

#[derive(Deserialize)]
pub(super) struct ClickBody {
    pub(super) post: i64,
    pub(super) url: String,
}

/// Link click counts, sent by a beacon. Only links stored for the post are counted.
///
/// Cross-site forgery is already refused by the origin check. This also requires the
/// browser's `Sec-Fetch-Site`, which `navigator.sendBeacon` always sends, so a header-less
/// scripted POST cannot quietly inflate counts. Counts are best-effort anyway: only
/// links already stored for the post are incremented, and the request is rate-limited per IP.
pub(super) async fn link_click(
    State(state): State<AppState>,
    v: Viewer,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, AppError> {
    let same_site = headers
        .get("sec-fetch-site")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| matches!(s, "same-origin" | "same-site" | "none"));
    if !same_site {
        return Err(AppError::Forbidden);
    }
    let b: ClickBody = serde_json::from_slice(&body).map_err(|_| AppError::BadRequest("bad click".into()))?;
    // Only a link in a post the visitor can read counts.
    let p = posts::post_ref(state.db()?, b.post).await.db()?.ok_or(AppError::NotFound)?;
    let (_, _, perms) = readable_topic(&state, &v, p.item_id).await?;
    super::ensure_post_visible(&p, perms, &v)?;
    links::count_link_click(state.db()?, b.post, &b.url).await.db()?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub(super) async fn topic_links(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    readable_topic(&state, &v, id).await?;
    let rows = links::link_clicks(state.db()?, id).await.db()?;
    let out: Vec<_> = rows.into_iter().map(|(p, u, c)| json!({ "post": p, "url": u, "clicks": c })).collect();
    Ok(Json(out).into_response())
}

#[derive(Deserialize)]
pub(super) struct PostsPartQuery {
    pub(super) from: i32,
    #[serde(default = "one")]
    pub(super) count: i64,
    /// The reader has the page in front of them: the posts count as read.
    #[serde(default)]
    pub(super) seen: bool,
}

pub(super) fn one() -> i64 {
    1
}

/// `?from=12&count=3`: posts 12 to 14 as HTML, as the topic page renders them for this viewer.
/// A post they may not see is left out, so an open page drops a post deleted under it.
pub(super) async fn topic_posts_part(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Query(q): Query<PostsPartQuery>,
) -> Result<Response, AppError> {
    let (t, tree, perms) = readable_topic(&state, &v, id).await?;
    let pool = state.db()?;
    let moderator = perms.has(Perms::MODERATE);
    let qa = tree.node(t.node_id).is_some_and(|n| n.mode == "qa");
    let uid = v.user.as_ref().map(|u| u.id);
    let mut rows = posts::topic_posts(
        pool,
        id,
        q.from.max(1),
        q.count.clamp(1, 50),
        moderator,
        uid.map(bf_core::ids::UserId::get),
    )
    .await
    .db()?;
    prepare_rows(&state, &v, id, moderator, &mut rows).await?;
    if let (true, Some(u), Some(last)) = (q.seen, &v.user, rows.last())
        && t.last_read.is_none_or(|r| r < last.post_number)
    {
        reading::mark_read(pool, u.id.get(), id, last.post_number).await.db()?;
    }
    let mut title = t.title.clone();
    let posts = post_views(&state, &v, &t, moderator, qa, &rows, &mut title).await?;
    let (layout, set) = layout(&state, &v, None);
    let part = TopicPostsPart {
        layout,
        posts,
        id,
        solved: t.solved_post_number,
        can_solve: state.settings().bool("solved_enabled")
            && (moderator || (uid.is_some() && uid == t.author_id.map(Into::into))),
        can_reply: perms.has(Perms::REPLY) && ((!t.locked && !t.archived) || moderator),
        can_moderate: moderator,
    };
    let html = bf_web::pages::render_string(&part).map_err(AppError::internal)?;
    let res = ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response();
    Ok(page(res, set, false))
}

/// Polling fallback for live updates: the newest post number of a readable topic.
pub(super) async fn topic_state(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let (_, _, perms) = readable_topic(&state, &v, id).await?;
    // Pending, hidden and deleted posts do not show in the count to anyone who cannot see them.
    let n = if perms.has(Perms::MODERATE) {
        posts::highest_post_number(state.db()?, id).await.db()?
    } else {
        posts::highest_visible_post_number(state.db()?, id).await.db()?
    }
    .ok_or(AppError::NotFound)?;
    let mut answer = json!({ "highest_post_number": n });
    if let Some(extra) = crate::platform::plugins::extra_fields(&state, "topic_state", id).await {
        answer["plugins"] = extra;
    }
    let mut res = Json(answer).into_response();
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(res)
}

#[derive(Deserialize)]
pub(super) struct CountsQuery {
    #[serde(default)]
    pub(super) keys: String,
}

#[derive(Deserialize)]
pub(super) struct QuoteRefs {
    #[serde(default)]
    pub(super) refs: String,
}

/// `?refs=12:3,12:7`: which quoted posts are gone, so quotes can show "[deleted]".
pub(super) async fn deleted_quotes(
    State(state): State<AppState>,
    v: Viewer,
    Query(q): Query<QuoteRefs>,
) -> Result<Response, AppError> {
    let pairs: Vec<(i64, i32)> = q
        .refs
        .split(',')
        .take(100)
        .filter_map(|r| r.split_once(':'))
        .filter_map(|(t, n)| Some((t.parse().ok()?, n.parse().ok()?)))
        .collect();
    // Only topics the visitor can read: otherwise this tells whether a post in a private forum is gone.
    let mut readable = std::collections::HashMap::new();
    let mut allowed = Vec::new();
    for (item, number) in pairs {
        if !readable.contains_key(&item) {
            let ok = readable.len() < 20 && readable_topic(&state, &v, item).await.is_ok();
            readable.insert(item, ok);
        }
        if readable[&item] {
            allowed.push((item, number));
        }
    }
    let (items, numbers): (Vec<i64>, Vec<i32>) = allowed.into_iter().unzip();
    let gone = posts::deleted_refs(state.db()?, &items, &numbers).await.db()?;
    Ok(Json(gone.into_iter().map(|(t, n)| format!("{t}:{n}")).collect::<Vec<_>>()).into_response())
}

/// Download counts for the attachment cards on a page.
pub(super) async fn download_counts(
    State(state): State<AppState>,
    Query(q): Query<CountsQuery>,
) -> Result<Response, AppError> {
    let keys: Vec<String> = q.keys.split(',').take(100).map(str::to_owned).collect();
    let rows = links::download_counts(state.db()?, &keys).await.db()?;
    Ok(Json(rows.into_iter().map(|(k, n)| json!({ "key": k, "downloads": n })).collect::<Vec<_>>()).into_response())
}

pub(super) async fn emoji_list(State(state): State<AppState>) -> Result<Response, AppError> {
    let rows = emoji::custom_emoji(state.db()?).await.db()?;
    let mut list: Vec<_> = rows.into_iter().map(|(n, u)| json!({ "name": n, "url": u })).collect();
    // Emoji and stickers from plugins, grouped by their set.
    for (name, url, set, label) in crate::platform::plugins::emoji_entries() {
        if !list.iter().any(|e| e["name"] == name.as_str()) {
            list.push(json!({ "name": name, "url": url, "set": set, "label": label }));
        }
    }
    let mut res = Json(list).into_response();
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=60"));
    Ok(res)
}

/// Spaced (post number, date) points for the timeline scrubber's label.
pub(super) async fn topic_dates(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    readable_topic(&state, &v, id).await?;
    let rows = reading::topic_dates(state.db()?, id).await.db()?;
    let out: Vec<_> = rows.into_iter().map(|(n, at)| json!([n, at.format("%Y-%m-%d").to_string()])).collect();
    Ok(Json(out).into_response())
}

#[derive(Deserialize)]
pub(super) struct InfiniteBody {
    pub(super) on: bool,
}

pub(super) async fn set_infinite_scroll(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Json(b): Json<InfiniteBody>,
) -> Result<Response, AppError> {
    let user = v.require_user()?;
    v.check_csrf_headers(&headers)?;
    users::set_infinite_scroll(state.db()?, user.id, b.on).await.db()?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize)]
pub(super) struct LinksBody {
    pub(super) urls: Vec<String>,
}

/// Duplicate-link warning: where these links were already posted. Advisory only.
pub(super) async fn links_seen(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Json(b): Json<LinksBody>,
) -> Result<Response, AppError> {
    v.require_user()?;
    v.check_csrf_headers(&headers)?;
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    let urls: Vec<String> = b.urls.into_iter().take(20).collect();
    let rows = links::links_seen(pool, &urls, &tree.readable(&v.actor)).await.db()?;
    let out: Vec<_> = rows
        .into_iter()
        .map(|(url, id, slug, title, n)| json!({ "url": url, "title": title, "topic_url": format!("{}#post-{n}", topic_path(id, &slug)) }))
        .collect();
    Ok(Json(out).into_response())
}
