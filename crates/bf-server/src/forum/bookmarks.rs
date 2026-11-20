//! Bookmarks and bookmark reminders.

use crate::forum::topics::ensure_post_visible;
use crate::forum::topics::readable_topic;
use crate::http::render::{ago, DbResult};
use crate::http::viewer::{CsrfForm, Viewer};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use bf_core::AppError;
use bf_core::slug::topic_path;
use bf_db::forum::{bookmarks, posts};
use serde::Deserialize;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/post/{id}/remind", post(remind))
        .route("/post/{id}/bookmark", post(bookmark))
        .route("/api/v1/me/bookmarks", get(recent_bookmarks))
}

#[derive(Deserialize)]
struct RemindForm {
    csrf: String,
    after: String,
}

async fn remind(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<RemindForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let seconds = match f.after.as_str() {
        "1min" => 60,
        "1h" => 3600,
        "1d" => 86_400,
        "1w" => 604_800,
        "1m" => 2_592_000,
        _ => return Err(AppError::Validation("Choose when to be reminded.".into())),
    };
    let pool = state.db()?;
    let p = bf_db::forum::posts::post_ref(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    let (t, _, perms) = readable_topic(&state, &v, p.item_id).await?;
    crate::forum::topics::ensure_post_visible(&p, perms, &v)?;
    bookmarks::set_reminder(pool, user.id.get(), id, seconds).await.db()?;
    Ok(Redirect::to(&crate::forum::topics::post_url(&state, t.id, &t.slug, p.post_number)).into_response())
}

/// Sends due reminders. They name no topic: the member may have lost access since setting one,
/// and the short link checks permissions when it is opened.
pub(crate) async fn send_reminders(state: &AppState) {
    let Some(pool) = &state.pool else { return };
    let due = match bookmarks::take_due_reminders(pool, 500).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e, "reminder query failed");
            return;
        }
    };
    for (user, post) in due {
        let data = json!({ "url": format!("/p/{post}") });
        crate::community::notifications::send(state, 0, &[user], "reminder", &data).await;
    }
}

/// The viewer's newest bookmarks, for the user menu.
#[utoipa::path(get, path = "/api/v1/me/bookmarks", tag = "me",
    responses((status = 200, body = Object, description = "the newest bookmarks"), (status = 403, body = bf_core::error::Problem)))]
pub(crate) async fn recent_bookmarks(State(state): State<AppState>, v: Viewer) -> Result<Response, AppError> {
    let user = v.require_user()?;
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    let rows = bookmarks::bookmarks(pool, user.id.get(), &tree.readable(&v.actor)).await.db()?;
    let items: Vec<_> = rows
        .iter()
        .take(15)
        .map(|(id, slug, title, number, at)| {
            serde_json::json!({
                "url": format!("{}#post-{number}", topic_path(*id, slug)),
                "description": title,
                "text": title,
                "when": ago(*at),
                "when_at": at.to_rfc3339(),
                "read": true,
                "icon": "bookmark",
            })
        })
        .collect();
    Ok(axum::Json(items).into_response())
}

async fn bookmark(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let pool = state.db()?;
    let p = posts::post_ref(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    let (t, _, perms) = readable_topic(&state, &v, p.item_id).await?;
    ensure_post_visible(&p, perms, &v)?;
    bookmarks::toggle_bookmark(pool, user.id.get(), id).await.db()?;
    Ok(Redirect::to(&crate::forum::topics::post_url(&state, t.id, &t.slug, p.post_number)).into_response())
}
