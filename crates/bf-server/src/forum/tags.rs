//! Tag pages, tag editing, tag groups and muted tags.

use crate::admin::api::admin;
use crate::forum::index::topic_line;
use crate::forum::posting::apply_tags;
use crate::forum::posting::clean_tag;
use crate::forum::topics::readable_topic;
use crate::http::render::page;
use crate::http::render::unread_count;
use crate::http::render::{DbResult, layout};
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post, put};
use axum::{Form, Json, Router};
use bf_core::AppError;
use bf_core::perms::Perms;
use bf_core::slug::topic_path;
use bf_db::forum::tags;
use bf_web::pages::{StreamPage, render};
use serde::Deserialize;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/admin/tag-groups", get(list_groups).post(create_group))
        .route("/api/v1/admin/tag-groups/{id}", put(update_group).delete(delete_group))
        .route("/t/{id}/tags", post(edit_tags))
        .route("/settings/muted-tags", post(save_muted_tags))
}

pub fn cached_pages() -> Router<AppState> {
    Router::new().route("/tag/{slug}", get(tag_page))
}

/// Topic tag rules from tag groups. Staff follow them too, so forums stay consistent.
pub(crate) async fn check_tag_rules(state: &AppState, item: i64, slugs: &[String]) -> Result<(), AppError> {
    match tags::tag_rule_problem(state.db()?, item, slugs).await.db()? {
        Some(problem) => Err(AppError::Validation(problem)),
        None => Ok(()),
    }
}

fn check_group(g: &tags::TagGroup) -> Result<(), AppError> {
    if g.name.trim().is_empty() || g.name.chars().count() > 60 {
        return Err(AppError::Validation("A tag group needs a name of up to 60 characters.".into()));
    }
    Ok(())
}

async fn list_groups(State(state): State<AppState>, v: Viewer, headers: HeaderMap) -> Result<Response, AppError> {
    admin(&state, &v, &headers, false)?;
    Ok(Json(tags::tag_groups(state.db()?).await.db()?).into_response())
}

async fn create_group(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Json(g): Json<tags::TagGroup>,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, true)?;
    check_group(&g)?;
    let id = tags::save_tag_group(state.db()?, 0, &g).await.db()?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))).into_response())
}

async fn update_group(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(g): Json<tags::TagGroup>,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, true)?;
    check_group(&g)?;
    if id <= 0 || tags::save_tag_group(state.db()?, id, &g).await.db()?.is_none() {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn delete_group(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, true)?;
    if !tags::delete_tag_group(state.db()?, id).await.db()? {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize)]
struct TagsForm {
    csrf: String,
    #[serde(default)]
    tags: String,
}

/// `POST /t/{id}/tags`: by the topic's author or a moderator.
async fn edit_tags(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<TagsForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let (t, tree, perms) = readable_topic(&state, &v, id).await?;
    let moderator = perms.has(Perms::MODERATE);
    let archived_forum = tree.node(t.node_id).is_some_and(|n| n.kind == "archive");
    if !moderator
        && (t.author_id != Some(user.id.get()) || t.locked || t.archived || archived_forum || v.actor.silenced)
    {
        return Err(AppError::Forbidden);
    }
    crate::moderation::gate::account_ready(&state, user).map_err(AppError::Validation)?;
    // A forum that requires a tag does not let a topic end up with none.
    if tree.node(t.node_id).is_some_and(|n| n.require_tag)
        && !f.tags.split(',').any(|x| crate::forum::posting::clean_tag(&state, x).is_some())
    {
        return Err(AppError::Validation("Topics in this forum need at least one tag.".into()));
    }
    apply_tags(&state, &v.actor, id, &f.tags, moderator).await?;
    Ok(Redirect::to(&topic_path(t.id, &t.slug)).into_response())
}

/// `GET /tag/{slug}`: topics with a tag, in forums the viewer can read.
async fn tag_page(
    State(state): State<AppState>,
    v: Viewer,
    Path(slug): Path<String>,
    crate::http::ui::EditLayout(edit): crate::http::ui::EditLayout,
) -> Result<Response, AppError> {
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    let readable = tree.readable(&v.actor);
    let name = tags::tag_by_slug(pool, &slug, &readable).await.db()?.ok_or(AppError::NotFound)?;
    let uid = v.user.as_ref().map(|u| u.id);
    let topics = tags::tag_topics(pool, &slug, &readable, uid.map(bf_core::ids::UserId::get), 50).await.db()?;
    let heading = v.t().f("stream.tag", &[("name", name.as_str())]);
    let (mut l, set) = layout(&state, &v, Some(&heading));
    l.feed = Some(format!("/tag/{slug}/feed.rss"));
    l.canonical = Some(format!("{}/tag/{slug}", state.config.base_url));
    l.unread_notifications = unread_count(&state, &v).await;
    let p = StreamPage {
        layout: l,
        heading,
        topics: topics.iter().map(topic_line).collect(),
        unread_view: false,
        periods: vec![],
        // All tag pages share one layout.
        area: "tag",
        widgets: crate::admin::themes::page_widgets(&state, "tag").await,
        can_edit_layout: v.actor.is_admin,
        editing_layout: v.actor.is_admin && edit,
    };
    Ok(page(render(&p), set, v.user.is_none()))
}

#[derive(Deserialize)]
struct MutedTagsForm {
    csrf: String,
    #[serde(default)]
    tags: String,
}

async fn save_muted_tags(
    State(state): State<AppState>,
    v: Viewer,
    Form(f): Form<MutedTagsForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let slugs: Vec<String> = f.tags.split(',').filter_map(|t| clean_tag(&state, t)).map(|(_, s)| s).collect();
    tags::set_muted_tags(state.db()?, user.id.get(), &slugs).await.db()?;
    Ok(Redirect::to("/settings").into_response())
}
