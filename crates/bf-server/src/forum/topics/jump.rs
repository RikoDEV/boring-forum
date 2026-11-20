//! Linking to a post: visibility of one post, its URL, and the jump and share routes.

use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use bf_core::AppError;
use bf_core::perms::Perms;
use bf_core::slug::topic_path;
use bf_db::forum::{posts, reading};
use serde_json::json;

use super::{path_id, readable_topic};

/// A hidden or pending post exists only for its author and the forum's moderators. A topic being
/// readable says nothing about each post in it.
pub(crate) fn ensure_post_visible(p: &posts::PostRef, perms: Perms, v: &Viewer) -> Result<(), AppError> {
    let own = v.user.as_ref().is_some_and(|u| Some(u.id) == p.author_id.map(Into::into));
    if p.state == "visible" || own || perms.has(Perms::MODERATE) { Ok(()) } else { Err(AppError::NotFound) }
}

/// URL of the page that holds post `n` of a topic.
pub(crate) fn post_url(state: &AppState, id: i64, slug: &str, n: i32) -> String {
    let per_page = state.settings().int("posts_per_page") as i32;
    let page = (n.max(1) - 1) / per_page + 1;
    let base = topic_path(id, slug);
    if page <= 1 { format!("{base}#post-{n}") } else { format!("{base}?page={page}#post-{n}") }
}

/// `/t/{id}-{slug}/post/{n}`: jump to a post number.
pub(super) async fn jump_to_post(
    State(state): State<AppState>,
    v: Viewer,
    Path((_, id, n)): Path<(String, String, String)>,
) -> Result<Response, AppError> {
    let id = path_id(&id)?;
    let n = n.parse().map_err(|_| AppError::NotFound)?;
    let (t, _, _) = readable_topic(&state, &v, id).await?;
    Ok(Redirect::to(&post_url(&state, id, &t.slug, n)).into_response())
}

/// `/t/{id}-{slug}/unread`: the first post the viewer has not read.
pub(super) async fn jump_to_unread(
    State(state): State<AppState>,
    v: Viewer,
    Path((_, id)): Path<(String, String)>,
) -> Result<Response, AppError> {
    let id = path_id(&id)?;
    let (t, _, _) = readable_topic(&state, &v, id).await?;
    let read = match &v.user {
        Some(u) => reading::read_position(state.db()?, u.id.get(), id).await.db()?,
        None => 0,
    };
    let highest = posts::highest_post_number(state.db()?, id).await.db()?.unwrap_or(1);
    Ok(Redirect::to(&post_url(&state, id, &t.slug, (read + 1).min(highest))).into_response())
}

/// `/p/{post id}`: short share link for a post.
pub(super) async fn short_link(
    State(state): State<AppState>,
    v: Viewer,
    Path(post_id): Path<i64>,
) -> Result<Response, AppError> {
    let (item, n) = posts::locate_post(state.db()?, post_id).await.db()?.ok_or(AppError::NotFound)?;
    let (t, _, _) = readable_topic(&state, &v, item).await?;
    Ok(Redirect::permanent(&post_url(&state, item, &t.slug, n)).into_response())
}

/// A post's cooked HTML for "show reply context".
pub(super) async fn post_context(
    State(state): State<AppState>,
    v: Viewer,
    Path((id, n)): Path<(i64, i32)>,
) -> Result<Response, AppError> {
    readable_topic(&state, &v, id).await?;
    let (html, author) = posts::post_context(state.db()?, id, n).await.db()?.ok_or(AppError::NotFound)?;
    let mut answer = json!({ "html": html, "author": author }); // sanitized when cooked
    // Only looked up when a plugin adds fields to posts.
    if crate::platform::plugins::has_serializers()
        && let Ok(Some(post)) = bf_db::federation::post_id_at(state.db()?, id, n).await
    {
        crate::platform::plugins::add_plugin_fields(&state, "post", post, &mut answer).await;
    }
    Ok(Json(answer).into_response())
}
