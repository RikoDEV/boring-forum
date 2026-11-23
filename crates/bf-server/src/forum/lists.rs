//! Topic lists: latest, unread, top, scheduled and the activity stream.

use crate::forum::index::topic_line;
use crate::http::render::{DbResult, unread_count};
use crate::http::render::{layout, page};
use crate::http::ui::{PageQuery, simple_page};
use crate::http::viewer::{CsrfForm, Viewer};
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use bf_core::AppError;
use bf_core::html::esc;
use bf_core::perms::Perms;
use bf_core::slug::forum_path;
use bf_db::forum::{nodes, topics};
use bf_web::pages::{StreamPage, render};
use serde::Deserialize;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/top", get(top))
        .route("/latest", get(latest))
        .route("/unread", get(unread))
        .route("/c/{id}/read", post(mark_forum_read))
        .route("/read-all", post(mark_all_read))
        .route("/scheduled", get(scheduled))
}

async fn stream(state: AppState, v: Viewer, unread_only: bool, editing: bool) -> Result<Response, AppError> {
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    let uid = v.user.as_ref().map(|u| u.id);
    let topics =
        topics::latest_topics(pool, &tree.readable(&v.actor), uid.map(bf_core::ids::UserId::get), unread_only, 50)
            .await
            .db()?;
    let heading = v.t().t(if unread_only { "home.unread" } else { "nav.latest" });
    let (mut l, set) = layout(&state, &v, Some(&heading));
    l.canonical = Some(format!("{}/latest", state.config.base_url));
    l.noindex = unread_only;
    l.unread_notifications = unread_count(&state, &v).await;
    let p = StreamPage {
        layout: l,
        heading,
        topics: topics.iter().map(topic_line).collect(),
        unread_view: unread_only,
        periods: vec![],
        area: "latest",
        widgets: crate::admin::themes::page_widgets(&state, "latest").await,
        can_edit_layout: v.actor.is_admin,
        editing_layout: v.actor.is_admin && editing,
    };
    Ok(page(render(&p), set, v.user.is_none()))
}

async fn latest(State(state): State<AppState>, v: Viewer, Query(q): Query<PageQuery>) -> Result<Response, AppError> {
    stream(state, v, false, q.editing()).await
}

async fn unread(State(state): State<AppState>, v: Viewer, Query(q): Query<PageQuery>) -> Result<Response, AppError> {
    v.require_user()?;
    stream(state, v, true, q.editing()).await
}

async fn mark_forum_read(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    let node = tree.node(id).filter(|_| tree.perms(&v.actor, id).has(Perms::READ)).ok_or(AppError::NotFound)?;
    // Subforums are part of the forum the reader sees, so they are marked too.
    let mut ids = vec![id];
    let mut i = 0;
    while i < ids.len() {
        let parent = ids[i];
        ids.extend(tree.nodes.iter().filter(|n| n.parent_id == Some(parent)).map(|n| n.id));
        i += 1;
    }
    nodes::mark_nodes_read(pool, user.id.get(), &ids).await.db()?;
    Ok(Redirect::to(&forum_path(id, &node.slug)).into_response())
}

async fn mark_all_read(
    State(state): State<AppState>,
    v: Viewer,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    nodes::mark_nodes_read(pool, user.id.get(), &tree.readable(&v.actor)).await.db()?;
    Ok(Redirect::to("/").into_response())
}

#[derive(Deserialize)]
struct TopQuery {
    #[serde(default)]
    period: String,
    #[serde(default)]
    edit_layout: String,
}

async fn top(State(state): State<AppState>, v: Viewer, Query(q): Query<TopQuery>) -> Result<Response, AppError> {
    let period = topics::Period::parse(&q.period).unwrap_or(topics::Period::Week);
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    let ids = topics::top_ids(pool, &tree.readable(&v.actor), period, 50).await.db()?;
    let uid = v.user.as_ref().map(|u| u.id);
    let topics = bf_db::forum::topics::topics_by_ids(pool, &ids, uid.map(bf_core::ids::UserId::get)).await.db()?;
    let t = v.t();
    let heading = t.t("stream.top");
    let (mut l, set) = layout(&state, &v, Some(&heading));
    l.canonical = Some(format!("{}/top?period={}", state.config.base_url, period.key()));
    let body = StreamPage {
        layout: l,
        heading,
        topics: topics.iter().map(topic_line).collect(),
        unread_view: false,
        periods: ["day", "week", "month", "year", "all"]
            .into_iter()
            .map(|p| (t.t(&format!("stream.period_{p}")), format!("/top?period={p}"), p == period.key()))
            .collect(),
        area: "top",
        widgets: crate::admin::themes::page_widgets(&state, "top").await,
        can_edit_layout: v.actor.is_admin,
        editing_layout: v.actor.is_admin && q.edit_layout == "1",
    };
    Ok(page(render(&body), set, v.user.is_none()))
}

async fn scheduled(State(state): State<AppState>, v: Viewer) -> Result<Response, AppError> {
    let user = v.require_user()?;
    let rows = topics::scheduled_topics(state.db()?, user.id.get()).await.db()?;
    let items: String = rows
        .iter()
        .map(|(_, title, at)| {
            format!("<li>{} <span class=\"muted small\">{}</span></li>", esc(title), crate::http::render::ago(*at))
        })
        .collect();
    Ok(simple_page(&state, &v, &v.t().msg("Scheduled topics"), &format!("<ul class=\"activity\">{items}</ul>"), false))
}
