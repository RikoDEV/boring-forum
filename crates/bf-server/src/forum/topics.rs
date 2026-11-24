//! Reading a topic: rendering, jumping to posts, link counts and post context.

use crate::forum::tree::Tree;
use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use bf_core::AppError;
use bf_core::perms::Perms;
use bf_core::slug::topic_path;
use bf_db::forum::topics;
use bf_db::forum::topics::TopicRow;
use serde::Deserialize;
use std::sync::Arc;

mod api;
mod jump;
mod render;

use self::api::{
    deleted_quotes, download_counts, emoji_list, link_click, links_seen, set_infinite_scroll, topic_dates, topic_links,
    topic_posts_part, topic_state,
};
use self::jump::{jump_to_post, jump_to_unread, post_context, short_link};
use self::render::{post_views, prepare_rows, render_topic};

pub(crate) use self::jump::{ensure_post_visible, post_url};

/// Guest-cacheable pages (wrapped in the microcache by `lib.rs`).
pub fn cached_pages() -> Router<AppState> {
    Router::new()
        .route("/t/{seg}", get(topic_short))
        .route("/t/{seg}/{id}", get(topic))
        .route("/t/{seg}/{id}/{n}", get(jump_to_post))
        .route("/t/{seg}/{id}/unread", get(jump_to_unread))
        .route("/p/{id}", get(short_link))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/links/click", post(link_click))
        .route("/api/v1/topics/{id}/links", get(topic_links))
        .route("/api/v1/topics/{id}/state", get(topic_state))
        .route("/api/v1/topics/{id}/dates", get(topic_dates))
        .route("/api/v1/topics/{id}/posts/{n}", get(post_context))
        .route("/api/v1/topics/{id}/posts", get(topic_posts_part))
        .route("/api/v1/uploads/counts", get(download_counts))
        .route("/api/v1/emoji", get(emoji_list))
        .route("/api/v1/quotes/deleted", get(deleted_quotes))
        .route("/api/v1/me/infinite-scroll", axum::routing::put(set_infinite_scroll))
        .route("/api/v1/links/seen", post(links_seen))
}

#[derive(Deserialize)]
struct TopicQuery {
    page: Option<i64>,
    #[serde(default)]
    edit_layout: String,
}

pub(crate) fn path_id(s: &str) -> Result<i64, AppError> {
    s.parse().ok().filter(|n| *n > 0).ok_or(AppError::NotFound)
}

async fn topic(
    state: State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path((slug, id)): Path<(String, String)>,
    Query(q): Query<TopicQuery>,
) -> Result<Response, AppError> {
    let id = path_id(&id)?;
    let page = q.page.unwrap_or(1);
    if page < 1 || q.page == Some(1) {
        return Ok(Redirect::permanent(&topic_path(id, &slug)).into_response());
    }
    render_topic(state, v, headers, slug, id, page, q.edit_layout == "1").await
}

/// `/t/{id}` or `/t/{slug}`: the newest readable topic of that name.
async fn topic_short(State(state): State<AppState>, v: Viewer, Path(seg): Path<String>) -> Result<Response, AppError> {
    let pool = state.db()?;
    let id = if let Ok(id) = seg.parse::<i64>() {
        id
    } else {
        let tree = state.forum.get(pool).await?;
        bf_db::community::directory::topic_by_slug(pool, &seg, &tree.readable(&v.actor))
            .await
            .db()?
            .ok_or(AppError::NotFound)?
    };
    Ok(Redirect::permanent(&topic_path(id, &seg)).into_response())
}

/// Loads a topic the viewer may read, or 404 (never 403, so hidden forums stay hidden).
pub(crate) async fn readable_topic(
    state: &AppState,
    v: &Viewer,
    id: i64,
) -> Result<(TopicRow, Arc<Tree>, Perms), AppError> {
    readable(state, v, id, false).await
}

/// `readable_topic` for showing the page: the author may also look at their own deleted topic. Only
/// for looking; everything that acts on a topic goes through `readable_topic` and so never reaches it.
pub(crate) async fn viewable_topic(
    state: &AppState,
    v: &Viewer,
    id: i64,
) -> Result<(TopicRow, Arc<Tree>, Perms), AppError> {
    readable(state, v, id, true).await
}

async fn readable(
    state: &AppState,
    v: &Viewer,
    id: i64,
    own_deleted: bool,
) -> Result<(TopicRow, Arc<Tree>, Perms), AppError> {
    let pool = state.db()?;
    let t = topics::topic_for(pool, id, (v.user.as_ref().map(|u| u.id)).map(bf_core::ids::UserId::get))
        .await
        .db()?
        .ok_or(AppError::NotFound)?;
    let tree = state.forum.get(pool).await?;
    let perms = tree.perms(&v.actor, t.node_id);
    let own = own_deleted
        && t.state == "deleted"
        && t.author_id.is_some()
        && t.author_id == v.user.as_ref().map(|u| u.id.get());
    let visible = t.state == "visible" || perms.has(Perms::MODERATE) || own;
    if !perms.has(Perms::READ) || !visible {
        return Err(AppError::NotFound);
    }
    Ok((t, tree, perms))
}

/// Where this reputation sits among the ranks: (number, total, medal tier, medal colour).
pub(crate) fn rank_step(spec: &str, reputation: i64) -> (usize, usize, String, i16) {
    let mut scores: Vec<i64> =
        spec.split(',').filter_map(|r| r.split_once(':')).filter_map(|(s, _)| s.trim().parse().ok()).collect();
    scores.sort_unstable();
    let total = scores.len();
    let at = scores.iter().filter(|s| **s <= reputation).count();
    let tier = match (at * 3).checked_div(total.max(1)) {
        Some(0 | 1) => "bronze",
        Some(2) => "silver",
        _ => "gold",
    };
    (at, total, tier.to_owned(), (at % 10) as i16)
}

/// The highest rank this reputation reaches, from `score:name,score:name`.
pub(crate) fn rank_name(spec: &str, reputation: i64) -> String {
    spec.split(',')
        .filter_map(|r| r.split_once(':'))
        .filter_map(|(s, n)| s.trim().parse::<i64>().ok().map(|s| (s, n.trim())))
        .filter(|(s, _)| *s <= reputation)
        .max_by_key(|(s, _)| *s)
        .map_or_else(String::new, |(_, n)| n.to_owned())
}
