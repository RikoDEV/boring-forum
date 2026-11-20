//! JSON twins of pages: the forum tree, a topic, a post, a member and their activity, and
//! JSON routes for editing, deleting, reacting to, flagging and moving.
//!
//! The reads load the same rows, behind the same checks, as the pages. The writes call the page
//! handlers themselves (a form is built from the JSON body), so every rule, limit, notification
//! and audit entry stays in one place; only the answer differs: a redirect becomes
//! `{"ok": true, "location": ...}`. Errors are problem+json, as on the whole API.
//!
//! A browser session sends the CSRF token in `X-CSRF-Token`; an API key needs none.

use crate::community::profiles::profile_access;
use crate::forum::topics::{ensure_post_visible, readable_topic};
use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use bf_core::AppError;
use bf_core::perms::Perms;
use bf_core::slug::{forum_path, profile_path, topic_path};
use bf_db::accounts::users;
use bf_db::forum::posts;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::ToSchema;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/forums", get(forum_tree))
        .route("/api/v1/topics/{id}", get(topic_detail))
        .route("/api/v1/users/{name}", get(user_profile))
        .route("/api/v1/users/{name}/activity", get(user_activity))
        .route("/api/v1/posts/{id}", get(post_detail).put(edit_post).delete(delete_post))
        .route("/api/v1/posts/{id}/react", post(react_post))
        .route("/api/v1/posts/{id}/flag", post(flag_post))
        .route("/api/v1/topics/{id}/move", post(move_topic))
}

/// The CSRF token a session sends in a header; empty for callers that have none.
fn csrf(headers: &HeaderMap) -> String {
    crate::http::viewer::csrf_header(headers).to_owned()
}

/// A page handler's answer as a JSON answer.
async fn outcome(res: Result<Response, AppError>) -> Result<Response, AppError> {
    let res = res?;
    let status = res.status();
    if status.is_redirection() {
        let location = res.headers().get(header::LOCATION).and_then(|h| h.to_str().ok()).map(str::to_owned);
        return Ok(Json(json!({ "ok": true, "location": location })).into_response());
    }
    let json = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|c| c.starts_with("application/json") || c.starts_with("application/problem+json"));
    if status.is_success() && !json {
        // A page ("awaiting approval"): the action was taken, and says so as data.
        return Ok((status, Json(json!({ "ok": true, "pending": status == StatusCode::ACCEPTED }))).into_response());
    }
    Ok(res)
}

/// A form for a page handler, made from a JSON object the caller sent.
fn form<T: serde::de::DeserializeOwned>(fields: Value) -> Result<Form<T>, AppError> {
    serde_json::from_value(fields).map(Form).map_err(|_| AppError::BadRequest("The request body is not valid.".into()))
}

#[derive(Serialize, ToSchema)]
pub struct ForumNode {
    id: i64,
    parent_id: Option<i64>,
    /// `category`, `forum`, `link`, `archive` ...
    kind: String,
    slug: String,
    title: String,
    description_html: String,
    url: String,
    position: i32,
    topic_count: i32,
    post_count: i32,
    #[schema(value_type = Option<String>, format = DateTime)]
    last_activity_at: Option<chrono::DateTime<chrono::Utc>>,
    /// The viewer may start a topic here.
    can_create: bool,
    can_reply: bool,
    can_moderate: bool,
}

/// The forums and categories the viewer may read, flat, ordered as on the index; build the tree
/// with `parent_id`.
#[utoipa::path(get, path = "/api/v1/forums", tag = "forums",
    responses((status = 200, body = [ForumNode])))]
pub async fn forum_tree(State(state): State<AppState>, v: Viewer) -> Result<Response, AppError> {
    let tree = state.forum.get(state.db()?).await?;
    let out: Vec<ForumNode> = tree
        .nodes
        .iter()
        .filter_map(|n| {
            let perms = tree.perms(&v.actor, n.id);
            perms.has(Perms::READ).then(|| ForumNode {
                id: n.id,
                parent_id: n.parent_id,
                kind: n.kind.clone(),
                slug: n.slug.clone(),
                title: n.title.clone(),
                description_html: n.description_html.clone(),
                url: forum_path(n.id, &n.slug),
                position: n.position,
                topic_count: n.topic_count,
                post_count: n.post_count,
                last_activity_at: n.last_activity_at,
                can_create: perms.has(Perms::CREATE),
                can_reply: perms.has(Perms::REPLY),
                can_moderate: perms.has(Perms::MODERATE),
            })
        })
        .collect();
    Ok(Json(out).into_response())
}

#[derive(Serialize, ToSchema)]
pub struct PostJson {
    id: i64,
    number: i32,
    author: Option<String>,
    author_id: Option<i64>,
    html: String,
    likes: i32,
    edit_count: i32,
    /// `visible`, or `hidden` for moderators.
    state: String,
    #[schema(value_type = String, format = DateTime)]
    created_at: chrono::DateTime<chrono::Utc>,
    reply_to_number: Option<i32>,
    /// Counts by reaction, such as "♥ 2 · 😄 1".
    reactions: String,
    /// The viewer's own reaction, if signed in and reacted.
    my_reaction: Option<i64>,
}

fn post_json(p: &posts::PostRow) -> PostJson {
    PostJson {
        id: p.id,
        number: p.post_number,
        author: p.author.clone(),
        author_id: p.author_id,
        html: p.cooked.clone(),
        likes: p.like_count,
        edit_count: p.edit_count,
        state: p.state.clone(),
        created_at: p.created_at,
        reply_to_number: p.reply_to_number,
        reactions: p.reaction_summary.clone(),
        my_reaction: p.my_reaction,
    }
}

#[derive(Serialize, ToSchema)]
pub struct TopicDetail {
    id: i64,
    title: String,
    slug: String,
    url: String,
    forum_id: i64,
    author: Option<String>,
    pinned: bool,
    locked: bool,
    archived: bool,
    solved_post_number: Option<i32>,
    replies: i32,
    views: i32,
    highest_post_number: i32,
    tags: Vec<String>,
    #[schema(value_type = String, format = DateTime)]
    created_at: chrono::DateTime<chrono::Utc>,
    #[schema(value_type = String, format = DateTime)]
    last_posted_at: chrono::DateTime<chrono::Utc>,
    can_reply: bool,
    can_moderate: bool,
    /// A page of posts, from `from` on.
    posts: Vec<PostJson>,
}

#[derive(Deserialize)]
pub struct PostsQuery {
    /// First post number to return.
    #[serde(default = "one")]
    from: i32,
    /// How many posts, at most 50.
    #[serde(default = "twenty")]
    count: i64,
}

fn one() -> i32 {
    1
}

fn twenty() -> i64 {
    20
}

/// A topic and a page of its posts, for what the viewer may read.
#[utoipa::path(get, path = "/api/v1/topics/{id}", tag = "topics",
    params(("id" = i64, Path), ("from" = Option<i32>, Query, description = "first post number"), ("count" = Option<i64>, Query, description = "posts, at most 50")),
    responses((status = 200, body = TopicDetail), (status = 404, body = bf_core::error::Problem)))]
pub async fn topic_detail(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Query(q): Query<PostsQuery>,
) -> Result<Response, AppError> {
    let (t, _, perms) = readable_topic(&state, &v, id).await?;
    let viewer = v.user.as_ref().map(|u| u.id.get());
    let rows =
        posts::topic_posts(state.db()?, id, q.from.max(1), q.count.clamp(1, 50), perms.has(Perms::MODERATE), viewer)
            .await
            .db()?;
    let mut out = serde_json::to_value(TopicDetail {
        id: t.id,
        url: topic_path(t.id, &t.slug),
        title: t.title,
        slug: t.slug,
        forum_id: t.node_id,
        author: t.author,
        pinned: t.pinned,
        locked: t.locked,
        archived: t.archived,
        solved_post_number: t.solved_post_number,
        replies: t.reply_count,
        views: t.view_count,
        highest_post_number: t.highest_post_number,
        tags: t.tag_pairs.iter().filter_map(|p| p.split('|').next()).map(str::to_owned).collect(),
        created_at: t.created_at,
        last_posted_at: t.last_posted_at,
        can_reply: perms.has(Perms::REPLY) && !t.locked && !t.archived,
        can_moderate: perms.has(Perms::MODERATE),
        posts: rows.iter().map(post_json).collect(),
    })
    .map_err(AppError::internal)?;
    crate::platform::plugins::add_plugin_fields(&state, "topic", id, &mut out).await;
    Ok(Json(out).into_response())
}

/// One post, if the viewer may read its topic.
#[utoipa::path(get, path = "/api/v1/posts/{id}", tag = "posts", params(("id" = i64, Path)),
    responses((status = 200, body = PostJson), (status = 404, body = bf_core::error::Problem)))]
pub async fn post_detail(State(state): State<AppState>, v: Viewer, Path(id): Path<i64>) -> Result<Response, AppError> {
    let pool = state.db()?;
    let p = posts::post_ref(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    let (_, _, perms) = readable_topic(&state, &v, p.item_id).await?;
    ensure_post_visible(&p, perms, &v)?;
    let viewer = v.user.as_ref().map(|u| u.id.get());
    let row = posts::topic_posts(pool, p.item_id, p.post_number, 1, true, viewer)
        .await
        .db()?
        .into_iter()
        .find(|r| r.id == id)
        .ok_or(AppError::NotFound)?;
    Ok(Json(post_json(&row)).into_response())
}

#[derive(Serialize, ToSchema)]
pub struct UserJson {
    id: i64,
    username: String,
    display_name: Option<String>,
    title: Option<String>,
    group: String,
    url: String,
    about_html: Option<String>,
    location: Option<String>,
    website: Option<String>,
    status: Option<String>,
    #[schema(value_type = String, format = DateTime)]
    joined_at: chrono::DateTime<chrono::Utc>,
    /// Absent when the member hides it.
    #[schema(value_type = Option<String>, format = DateTime)]
    last_seen_at: Option<chrono::DateTime<chrono::Utc>>,
    post_count: i32,
    reputation: i64,
    likes_received: i64,
    solved_count: i64,
    cover: Option<String>,
    /// Only when the site shows trust levels.
    trust_level: Option<i16>,
}

/// A member's public profile, with the same visibility rules as their page.
#[utoipa::path(get, path = "/api/v1/users/{name}", tag = "users", params(("name" = String, Path)),
    responses((status = 200, body = UserJson), (status = 403, body = bf_core::error::Problem), (status = 404, body = bf_core::error::Problem)))]
pub async fn user_profile(
    State(state): State<AppState>,
    v: Viewer,
    Path(name): Path<String>,
) -> Result<Response, AppError> {
    let pool = state.db()?;
    let id = users::user_id_by_name(pool, &name).await.db()?.ok_or(AppError::NotFound)?;
    let p = users::profile(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    let (own, staff) = profile_access(pool, &v, id, &p.profile_visibility).await?;
    let seen = p.last_seen_at.filter(|_| !p.hide_last_seen || own || staff);
    let trust = (state.settings().bool("show_trust_levels") || own || staff).then_some(p.trust_level);
    let mut out = serde_json::to_value(UserJson {
        id: p.id.get(),
        url: profile_path(p.id.get(), &p.username),
        username: p.username,
        display_name: p.display_name,
        title: p.title,
        group: p.group_name,
        about_html: p.about_html,
        location: p.location.filter(|l| !l.is_empty()),
        website: p.website.filter(|w| !w.is_empty()),
        status: p.status_text,
        joined_at: p.joined_at,
        last_seen_at: seen,
        post_count: p.post_count,
        reputation: p.reputation,
        likes_received: p.likes_received,
        solved_count: p.solved_count,
        cover: p.cover_key.map(|k| format!("/uploads/{k}")),
        trust_level: trust,
    })
    .map_err(AppError::internal)?;
    crate::platform::plugins::add_plugin_fields(&state, "user", id.get(), &mut out).await;
    Ok(Json(out).into_response())
}

#[derive(Deserialize)]
pub struct ActivityQuery {
    /// `posts` (the default) or `topics`.
    #[serde(default)]
    kind: String,
    #[serde(default = "twenty")]
    count: i64,
}

#[derive(Serialize, ToSchema)]
pub struct ActivityItem {
    topic_id: i64,
    title: String,
    url: String,
    post_number: Option<i32>,
    /// Cooked HTML of the post; empty for topics.
    html: String,
    #[schema(value_type = String, format = DateTime)]
    created_at: chrono::DateTime<chrono::Utc>,
}

/// A member's recent posts or topics, in forums the viewer can read.
#[utoipa::path(get, path = "/api/v1/users/{name}/activity", tag = "users",
    params(("name" = String, Path), ("kind" = Option<String>, Query, description = "posts or topics"), ("count" = Option<i64>, Query)),
    responses((status = 200, body = [ActivityItem]), (status = 403, body = bf_core::error::Problem), (status = 404, body = bf_core::error::Problem)))]
pub async fn user_activity(
    State(state): State<AppState>,
    v: Viewer,
    Path(name): Path<String>,
    Query(q): Query<ActivityQuery>,
) -> Result<Response, AppError> {
    let pool = state.db()?;
    let id = users::user_id_by_name(pool, &name).await.db()?.ok_or(AppError::NotFound)?;
    let p = users::profile(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    profile_access(pool, &v, id, &p.profile_visibility).await?;
    let readable = state.forum.get(pool).await?.readable(&v.actor);
    let limit = q.count.clamp(1, 50);
    let out: Vec<ActivityItem> = if q.kind == "topics" {
        users::user_topics(pool, id, &readable, limit)
            .await
            .db()?
            .into_iter()
            .map(|(tid, slug, title, _, created_at)| ActivityItem {
                topic_id: tid,
                url: topic_path(tid, &slug),
                title,
                post_number: None,
                html: String::new(),
                created_at,
            })
            .collect()
    } else {
        users::user_posts(pool, id, &readable, limit)
            .await
            .db()?
            .into_iter()
            .map(|r| ActivityItem {
                topic_id: r.item_id,
                url: format!("{}#post-{}", topic_path(r.item_id, &r.slug), r.post_number),
                title: r.title,
                post_number: Some(r.post_number),
                html: r.cooked,
                created_at: r.created_at,
            })
            .collect()
    };
    Ok(Json(out).into_response())
}

#[derive(Deserialize, ToSchema)]
pub struct EditPost {
    /// The new Markdown source.
    body: String,
    /// The post's `edit_count` when it was loaded; a newer edit answers 409 with both versions.
    /// Left out, the edit is made on top of whatever is saved.
    base: Option<i32>,
    reason: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct Done {
    ok: bool,
    /// Where the page action would have sent a browser.
    location: Option<String>,
    /// The action waits for a moderator.
    pending: Option<bool>,
}

/// Edits a post, under the same rules as the edit form: limits, filters, grace period, conflict check.
#[utoipa::path(put, path = "/api/v1/posts/{id}", tag = "posts", params(("id" = i64, Path)), request_body = EditPost,
    responses((status = 200, body = Done), (status = 202, body = Done), (status = 403, body = bf_core::error::Problem),
        (status = 409, body = Object, description = "someone edited first: detail, base, theirs, yours"), (status = 422, body = bf_core::error::Problem)))]
pub async fn edit_post(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(b): Json<EditPost>,
) -> Result<Response, AppError> {
    let base = match b.base {
        Some(n) => n,
        None => posts::post_ref(state.db()?, id).await.db()?.ok_or(AppError::NotFound)?.edit_count,
    };
    let f = form(json!({
        "csrf": csrf(&headers), "body": b.body, "base": base.to_string(), "reason": b.reason.unwrap_or_default(),
    }))?;
    outcome(crate::forum::posting::edit(State(state), v, Path(id), f).await).await
}

/// Deletes a post, as its author or a moderator (the first post cannot be deleted alone).
#[utoipa::path(delete, path = "/api/v1/posts/{id}", tag = "posts", params(("id" = i64, Path)),
    responses((status = 200, body = Done), (status = 403, body = bf_core::error::Problem), (status = 404, body = bf_core::error::Problem)))]
pub async fn delete_post(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let f = form(json!({ "csrf": csrf(&headers) }))?;
    outcome(crate::forum::posting::delete(State(state), v, Path(id), f).await).await
}

#[derive(Deserialize, ToSchema)]
pub struct React {
    /// A reaction id from the site's reaction set; 0 removes the member's reaction.
    reaction: i64,
}

/// Sets or removes the member's reaction to a post.
#[utoipa::path(post, path = "/api/v1/posts/{id}/react", tag = "posts", params(("id" = i64, Path)), request_body = React,
    responses((status = 200, body = Done), (status = 403, body = bf_core::error::Problem), (status = 422, body = bf_core::error::Problem)))]
pub async fn react_post(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(b): Json<React>,
) -> Result<Response, AppError> {
    let f = form(json!({ "csrf": csrf(&headers), "reaction": b.reaction }))?;
    outcome(crate::forum::reactions::react(State(state), v, axum::http::HeaderMap::new(), Path(id), f).await).await
}

#[derive(Deserialize, ToSchema)]
pub struct Flag {
    /// A report reason key, as the report dialog lists them.
    reason: String,
    note: Option<String>,
}

/// Reports a post to the moderators.
#[utoipa::path(post, path = "/api/v1/posts/{id}/flag", tag = "posts", params(("id" = i64, Path)), request_body = Flag,
    responses((status = 200, body = Done), (status = 404, body = bf_core::error::Problem), (status = 422, body = bf_core::error::Problem)))]
pub async fn flag_post(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(b): Json<Flag>,
) -> Result<Response, AppError> {
    let f = form(json!({ "csrf": csrf(&headers), "reason": b.reason, "note": b.note.unwrap_or_default() }))?;
    outcome(crate::moderation::posts::report(State(state), v, Path(id), f).await).await
}

#[derive(Deserialize, ToSchema)]
pub struct Move {
    /// The destination forum.
    to: i64,
    /// Leave a link in the old forum.
    #[serde(default)]
    redirect: bool,
    /// Days the link stays; forever when left out.
    redirect_days: Option<i32>,
}

/// Moves a topic to another forum (needs moderation rights in both).
#[utoipa::path(post, path = "/api/v1/topics/{id}/move", tag = "topics", params(("id" = i64, Path)), request_body = Move,
    responses((status = 200, body = Done), (status = 403, body = bf_core::error::Problem), (status = 404, body = bf_core::error::Problem)))]
pub async fn move_topic(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(b): Json<Move>,
) -> Result<Response, AppError> {
    let f = form(json!({
        "csrf": csrf(&headers), "to": b.to, "redirect": if b.redirect { "1" } else { "" },
        "redirect_days": b.redirect_days.map(|d| d.to_string()).unwrap_or_default(),
    }))?;
    outcome(crate::moderation::topics::move_topic(State(state), v, Path(id), f).await).await
}
