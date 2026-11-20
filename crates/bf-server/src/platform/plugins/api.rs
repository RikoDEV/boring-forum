//! JSON for lists, and the public metadata plugins attach to what a list shows: one query per
//! page, however many rows it has.

use crate::forum::tree::Tree;
use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use bf_core::AppError;
use bf_core::perms::Perms;
use bf_db::forum::{posts, topics};
use bf_db::platform::registry_content as rc;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;

/// Adds `meta` (the public metadata, `{plugin: {key: value}}`) to each object of `rows` that has
/// some, looking each up by its `id_field`. One query for the whole list; nothing when no
/// plugin keeps metadata on that kind.
pub(crate) async fn attach_meta(state: &AppState, target: &str, rows: &mut [Value], id_field: &str) {
    let Some(pool) = state.pool.as_ref() else { return };
    let ids: Vec<i64> = rows.iter().filter_map(|r| r.get(id_field).and_then(Value::as_i64)).collect();
    if ids.is_empty() {
        return;
    }
    let Ok(mut all): Result<HashMap<i64, Value>, _> = rc::public_meta_many(pool, target, &ids).await else { return };
    for row in rows {
        if let Some(meta) = row.get(id_field).and_then(Value::as_i64).and_then(|id| all.remove(&id)) {
            row["meta"] = meta;
        }
    }
}

fn topic_json(t: &topics::TopicRow) -> Value {
    json!({
        "id": t.id, "title": t.title, "slug": t.slug, "url": format!("/t/{}/{}", t.slug, t.id), "author": t.author,
        "forum_id": t.node_id, "pinned": t.pinned, "locked": t.locked, "replies": t.reply_count, "views": t.view_count,
        "last_posted_at": t.last_posted_at, "created_at": t.created_at,
        "tags": t.tag_pairs.iter().filter_map(|p| p.split('|').next()).collect::<Vec<_>>(),
    })
}

#[derive(Deserialize)]
pub(super) struct ListQuery {
    #[serde(default)]
    page: i64,
}

const PAGE: i64 = 30;

/// `GET /api/v1/forums/{id}/topics?page=0`: a forum's topics, newest activity first, for the
/// viewer's own access.
pub(super) async fn forum_topics(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Query(q): Query<ListQuery>,
) -> Result<Response, AppError> {
    let pool = state.db()?;
    let tree: std::sync::Arc<Tree> = state.forum.get(pool).await?;
    if tree.node(id).as_ref().is_none_or(|n| n.kind != "forum") || !tree.perms(&v.actor, id).has(Perms::READ) {
        return Err(AppError::NotFound);
    }
    let viewer = v.user.as_ref().map(|u| u.id.get());
    let rows = topics::forum_topics(
        pool,
        id,
        topics::Sort::Activity,
        topics::TopicFilter::default(),
        viewer,
        PAGE,
        q.page.max(0).saturating_mul(PAGE),
    )
    .await
    .db()?;
    let mut out: Vec<Value> = rows.iter().map(topic_json).collect();
    attach_meta(&state, "topic", &mut out, "id").await;
    Ok(Json(out).into_response())
}

#[derive(Deserialize)]
pub(super) struct PostsQuery {
    #[serde(default = "one")]
    from: i32,
    #[serde(default = "twenty")]
    count: i64,
}

fn one() -> i32 {
    1
}

fn twenty() -> i64 {
    20
}

/// `GET /api/v1/topics/{id}/post-list?from=1&count=20`: posts of a topic the viewer may read, as
/// HTML as the topic page shows it, with their public metadata.
pub(super) async fn post_list(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Query(q): Query<PostsQuery>,
) -> Result<Response, AppError> {
    let pool = state.db()?;
    let viewer = v.user.as_ref().map(|u| u.id.get());
    let t = topics::topic_for(pool, id, viewer).await.db()?.ok_or(AppError::NotFound)?;
    let tree = state.forum.get(pool).await?;
    let perms = tree.perms(&v.actor, t.node_id);
    if t.state != "visible" || !perms.has(Perms::READ) {
        return Err(AppError::NotFound);
    }
    let rows = posts::topic_posts(pool, id, q.from.max(1), q.count.clamp(1, 50), perms.has(Perms::MODERATE), viewer)
        .await
        .db()?;
    let mut out: Vec<Value> = rows
        .iter()
        .map(|p| {
            json!({ "id": p.id, "number": p.post_number, "author": p.author, "html": p.cooked, "likes": p.like_count,
                    "created_at": p.created_at, "state": p.state })
        })
        .collect();
    attach_meta(&state, "post", &mut out, "id").await;
    Ok(Json(out).into_response())
}
