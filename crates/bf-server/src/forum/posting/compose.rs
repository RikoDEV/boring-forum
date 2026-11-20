//! The composer's helpers: preview, `@member`/`#forum` suggestions and inviting a member to a topic.

use crate::forum::topics::readable_topic;
use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Form, Json};
use bf_core::AppError;
use bf_core::perms::Perms;
use bf_core::slug::{forum_path, topic_path};
use bf_db::accounts::users;
use bf_db::community::mentions;
use bf_db::forum::tags;
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
pub(super) struct PreviewBody {
    pub(super) raw: String,
}

/// Composer preview: the same cooker as saving, so the preview is exactly what posts.
pub(super) async fn preview(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Json(b): Json<PreviewBody>,
) -> Result<Response, AppError> {
    v.require_user()?;
    v.check_csrf_headers(&headers)?;
    if b.raw.chars().count() as i64 > state.settings().int("max_post_length") {
        return Err(AppError::Validation("Post is too long".into()));
    }
    let raw = b.raw;
    let html = tokio::task::spawn_blocking(move || bf_cook::cook(&raw).html).await.map_err(AppError::internal)?;
    let mut res = Json(json!({ "html": html })).into_response();
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(res)
}

#[derive(Deserialize)]
pub(super) struct AutocompleteQuery {
    pub(super) kind: String,
    #[serde(default)]
    pub(super) q: String,
    /// The topic being replied to: its participants come first among members.
    pub(super) topic: Option<i64>,
}

/// Composer suggestions for `@member` and `#forum`. Members only; forums are
/// limited to those the viewer can read.
pub(super) async fn autocomplete(
    State(state): State<AppState>,
    v: Viewer,
    Query(q): Query<AutocompleteQuery>,
) -> Result<Response, AppError> {
    v.require_user()?;
    let pool = state.db()?;
    let out = match q.kind.as_str() {
        "user" => {
            // Participants only of a topic the viewer can read.
            let topic = match q.topic {
                Some(id) => readable_topic(&state, &v, id).await.ok().map(|_| id),
                None => None,
            };
            let me = v.user.as_ref().map_or(0, |u| u.id.get());
            let people =
                users::mention_candidates(pool, &q.q, topic, me, 8, crate::http::viewer::is_staff(&v)).await.db()?;
            let groups = if q.q.is_empty() {
                vec![]
            } else {
                mentions::mentionable_groups(pool, &bf_core::ids::raw(&v.actor.groups), &q.q, 4).await.db()?
            };
            let group = |n: String| json!({ "label": n, "group": true });
            people
                .into_iter()
                .map(|p| json!({ "label": p.username, "name": p.display_name, "in_topic": p.in_topic }))
                .chain(groups.into_iter().map(group))
                .collect::<Vec<_>>()
        }
        "tag" => tags::tags_starting(pool, &q.q, 8, &state.forum.get(pool).await?.readable(&v.actor))
            .await
            .db()?
            .into_iter()
            .map(|(n, _)| json!({ "label": n }))
            .collect(),
        "node" => {
            let tree = state.forum.get(pool).await?;
            let needle = q.q.to_lowercase();
            let forums = tree
                .nodes
                .iter()
                .filter(|n| n.kind != "category" && n.title.to_lowercase().contains(&needle))
                .filter(|n| tree.perms(&v.actor, n.id).has(Perms::VIEW))
                .take(6)
                .map(|n| json!({ "label": n.title, "url": forum_path(n.id, &n.slug) }));
            let tags = tags::tags_starting(pool, &q.q, 4, &tree.readable(&v.actor)).await.db()?;
            forums
                .chain(tags.into_iter().map(|(n, s)| json!({ "label": n, "url": format!("/tag/{s}"), "tag": true })))
                .collect()
        }
        _ => return Err(AppError::NotFound),
    };
    let mut res = Json(out).into_response();
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=30"));
    Ok(res)
}

#[derive(Deserialize)]
pub(super) struct TopicInviteForm {
    pub(super) csrf: String,
    pub(super) username: String,
}

/// Points another member at a topic. They must already be able to read it.
pub(super) async fn invite_to_topic(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<TopicInviteForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    if v.actor.silenced {
        return Err(AppError::Forbidden);
    }
    let (t, _, _) = readable_topic(&state, &v, id).await?;
    let pool = state.db()?;
    let name = f.username.trim().trim_start_matches('@').to_lowercase();
    let ids = bf_core::ids::raw(&bf_db::accounts::users::user_ids_by_names(pool, &[name]).await.db()?);
    let ids = crate::community::notifications::can_read(&state, pool, t.node_id, ids).await;
    if ids.is_empty() {
        return Err(AppError::Validation("No member by that name can read this topic.".into()));
    }
    let url = topic_path(t.id, &t.slug);
    // Invitations are mail and notifications to someone else: capped per sender, once per member and topic.
    match bf_db::forum::topics::claim_invite(pool, user.id.get(), ids[0], t.id).await.db()? {
        bf_db::forum::topics::Invite::Allowed => {}
        bf_db::forum::topics::Invite::TooMany => {
            return Err(AppError::Validation(
                "You have invited a lot of people lately. Please try again later.".into(),
            ));
        }
        bf_db::forum::topics::Invite::Already => {
            return Err(AppError::Validation("That member has already been invited to this topic.".into()));
        }
    }
    let data = serde_json::json!({ "by": user.username, "title": t.title, "url": url, "topic_id": t.id });
    crate::community::notifications::send(&state, user.id.get(), &ids, "topic_invite", &data).await;
    Ok(Redirect::to(&url).into_response())
}
