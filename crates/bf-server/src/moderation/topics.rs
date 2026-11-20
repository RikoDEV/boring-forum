//! Topic moderation: move, merge, split, copy, dates and bulk actions.

use crate::forum::posting::validate_title;
use crate::forum::topics::readable_topic;
use crate::forum::tree::Tree;
use crate::http::render::{db_err, DbResult};
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::post;
use axum::{Form, Router};
use bf_core::AppError;
use bf_core::perms::Perms;
use bf_core::slug::{forum_path, slugify, topic_path};
use bf_db::forum::topics::{TopicFlag, TopicRow};
use bf_db::forum::{posts, topics};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/t/{id}/mod", post(moderate))
        .route("/c/{id}/bulk", post(bulk))
        .route("/t/{id}/move", post(move_topic))
        .route("/t/{id}/merge", post(merge_topic))
        .route("/t/{id}/split", post(split_topic))
        .route("/t/{id}/copy", post(copy_topic))
        .route("/t/{id}/date", post(topic_date))
}

/// The flag an action name switches, and to which value.
fn topic_flag(action: &str) -> Option<(TopicFlag, bool)> {
    let name = action.strip_prefix("un").unwrap_or(action);
    let flag = [
        (TopicFlag::Pinned, "pin"),
        (TopicFlag::Locked, "lock"),
        (TopicFlag::Listed, "list"),
        (TopicFlag::Archived, "archive"),
        (TopicFlag::Featured, "feature"),
    ]
    .into_iter()
    .find(|(_, n)| *n == name)?
    .0;
    Some((flag, !action.starts_with("un")))
}

/// Drops the cached forum tree and pages after a change.
pub(crate) fn refresh(state: &AppState) {
    state.forum.invalidate();
    state.microcache.clear();
}

#[derive(Deserialize)]
struct ModForm {
    csrf: String,
    action: String,
    #[serde(default)]
    reason: String,
}

async fn moderate(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<ModForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let (t, _, perms) = readable_topic(&state, &v, id).await?;
    if !perms.has(Perms::MODERATE) {
        return Err(AppError::Forbidden);
    }
    let (flag, on) = topic_flag(&f.action).ok_or_else(|| AppError::BadRequest("unknown action".into()))?;
    topics::set_topic_flag(state.db()?, id, flag, on, user.id.get()).await.db()?;
    let reason: String = f.reason.trim().chars().take(200).collect();
    if !reason.is_empty() {
        posts::add_action_reason(state.db()?, id, &reason).await.db()?;
    }
    crate::platform::webhooks::moderated(&state, &f.action, user.id.get(), "topic", id, json!({ "reason": reason }))
        .await;
    state.hub.publish(&format!("item:{id}"), "state", json!({ "action": f.action }));
    state.microcache.clear();
    Ok(Redirect::to(&topic_path(id, &t.slug)).into_response())
}

/// Loads a topic the viewer moderates, or 403/404.
pub(crate) async fn moderated_topic(state: &AppState, v: &Viewer, id: i64) -> Result<(TopicRow, Arc<Tree>), AppError> {
    let (t, tree, perms) = readable_topic(state, v, id).await?;
    if !perms.has(Perms::MODERATE) {
        return Err(AppError::Forbidden);
    }
    Ok((t, tree))
}

/// May this viewer make `new_author` the author of a topic? Putting someone's name on a text is
/// for global staff, not for the moderator of one forum (or a club's owner); and only an
/// administrator may put an administrator's or another moderator's name on one.
pub(crate) async fn may_change_author(state: &AppState, v: &Viewer, new_author: i64) -> Result<(), AppError> {
    let me = v.require_user()?;
    if !crate::http::viewer::is_staff(v) {
        return Err(AppError::Forbidden);
    }
    let target =
        bf_db::accounts::users::user_by_id(state.db()?, new_author.into()).await.db()?.ok_or(AppError::NotFound)?;
    if !v.actor.is_admin
        && target.id != me.id
        && (target.is_admin || target.groups.contains(&bf_core::ids::GroupId::MODERATORS))
    {
        return Err(AppError::Validation(
            "Only an administrator can make an administrator or a moderator the author.".into(),
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
pub(crate) struct MoveForm {
    csrf: String,
    to: i64,
    #[serde(default)]
    redirect: String,
    #[serde(default)]
    redirect_days: String,
}

pub(crate) async fn move_topic(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<MoveForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let (t, tree) = moderated_topic(&state, &v, id).await?;
    // Moving needs moderation rights at the destination as well.
    if !tree.node(f.to).is_some_and(|n| n.kind == "forum") || !tree.perms(&v.actor, f.to).has(Perms::MODERATE) {
        return Err(AppError::Forbidden);
    }
    let from = t.node_id;
    bf_db::moderation::topics::move_topic(state.db()?, id, f.to, user.id.get()).await.db()?;
    // Optionally leave a link in the old forum, for good or for some days.
    if f.redirect == "1" {
        let days = f.redirect_days.trim().parse::<i32>().ok().filter(|d| (1..=3650).contains(d));
        bf_db::forum::topics::add_redirect(state.db()?, from, id, days).await.db()?;
    }
    if let Some(author) = sqlx::query_scalar::<_, Option<i64>>("SELECT author_id FROM items WHERE id = $1")
        .bind(id)
        .fetch_one(state.db()?)
        .await
        .db()?
        .filter(|a| *a != user.id)
    {
        // Told only if they can read the forum it now sits in.
        if !crate::community::notifications::can_read(&state, state.db()?, f.to, vec![author]).await.is_empty() {
            let data = json!({ "by": user.username, "title": t.title, "url": topic_path(id, &t.slug) });
            crate::community::notifications::send(&state, user.id.get(), &[author], "moved", &data).await;
        }
    }
    crate::platform::webhooks::emit(&state, "topic.moved", json!({ "topic_id": id, "from": from, "to": f.to })).await;
    crate::platform::webhooks::moderated(&state, "move", user.id.get(), "topic", id, json!({ "from": from, "to": f.to }))
        .await;
    refresh(&state);
    Ok(Redirect::to(&topic_path(id, &t.slug)).into_response())
}

#[derive(Deserialize)]
struct MergeForm {
    csrf: String,
    into: i64,
}

async fn merge_topic(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<MergeForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    if f.into == id {
        return Err(AppError::Validation("A topic cannot be merged into itself.".into()));
    }
    moderated_topic(&state, &v, id).await?;
    let (target, _) = moderated_topic(&state, &v, f.into).await?;
    if target.state != "visible" {
        return Err(AppError::Validation("A topic can only be merged into a visible topic.".into()));
    }
    bf_db::moderation::topics::merge_topics(state.db()?, id, f.into, user.id.get()).await.db()?;
    refresh(&state);
    Ok(Redirect::to(&topic_path(target.id, &target.slug)).into_response())
}

async fn split_topic(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(fields): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let get = |k: &str| fields.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str()).unwrap_or_default();
    v.check_csrf(get("csrf"))?;
    let user = v.require_user()?;
    moderated_topic(&state, &v, id).await?;
    let posts: Vec<i64> = fields.iter().filter(|(n, _)| n == "post").filter_map(|(_, v)| v.parse().ok()).collect();
    if posts.is_empty() {
        return Err(AppError::Validation("Select at least one reply to split.".into()));
    }
    let title = get("title").trim();
    validate_title(&state, title).map_err(AppError::Validation)?;
    let title = crate::platform::plugins::filter_text(&state, "item.before_create", "title", title).await?;
    let title = title.as_str();
    let slug = slugify(title);
    let new_id =
        match bf_db::moderation::topics::split_posts(state.db()?, id, &posts, title, &slug, user.id.get()).await {
            Ok(n) => n,
            Err(sqlx::Error::RowNotFound) => {
                return Err(AppError::Validation("None of the selected posts can be split.".into()));
            }
            Err(e) => return Err(db_err(e)),
        };
    crate::platform::webhooks::moderated(&state, "split", user.id.get(), "topic", id, json!({ "to": new_id })).await;
    refresh(&state);
    Ok(Redirect::to(&topic_path(new_id, &slug)).into_response())
}

/// Bulk actions on a forum's topic list: lock, unlock, pin, unpin, delete, move, archive,
/// unarchive, add tags and change the owner.
async fn bulk(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(fields): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let get = |k: &str| fields.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str()).unwrap_or_default();
    v.check_csrf(get("csrf"))?;
    let user = v.require_user()?;
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    let node = tree.node(id).ok_or(AppError::NotFound)?;
    if !tree.perms(&v.actor, id).has(Perms::MODERATE) {
        return Err(AppError::Forbidden);
    }
    let topics: Vec<i64> = fields.iter().filter(|(n, _)| n == "topic").filter_map(|(_, v)| v.parse().ok()).collect();
    // Only topics that really are in this forum.
    let topics: Vec<(i64, String, String, Option<i64>)> =
        sqlx::query_as("SELECT id, title, slug, author_id FROM items WHERE id = ANY($1) AND node_id = $2")
            .bind(&topics)
            .bind(id)
            .fetch_all(pool)
            .await
            .db()?;
    // Whatever is wrong with the request as a whole is refused before any topic is touched.
    let action = get("action");
    if !matches!(action, "lock" | "unlock" | "pin" | "unpin" | "delete" | "move" | "archive" | "unarchive" | "tag" | "owner") {
        return Err(AppError::BadRequest("unknown action".into()));
    }
    let to: Option<i64> = if action == "move" {
        let to: i64 = get("to").parse().map_err(|_| AppError::Validation("Choose a forum to move to.".into()))?;
        if !tree.node(to).is_some_and(|n| n.kind == "forum") || !tree.perms(&v.actor, to).has(Perms::MODERATE) {
            return Err(AppError::Forbidden);
        }
        Some(to)
    } else {
        None
    };
    let new_tags = if action == "tag" {
        let csv = get("tags").trim();
        if csv.is_empty() {
            return Err(AppError::Validation("Enter at least one tag.".into()));
        }
        Some(csv.to_owned())
    } else {
        None
    };
    let new_owner: Option<i64> = if action == "owner" {
        let name = get("owner").trim().trim_start_matches('@');
        let found = bf_db::accounts::users::user_id_by_name(pool, name).await.db()?;
        let owner = found.ok_or_else(|| AppError::Validation("There is no member with that username.".into()))?.get();
        may_change_author(&state, &v, owner).await?;
        Some(owner)
    } else {
        None
    };
    let mut failed = 0_usize;
    for (t, title, slug, author) in &topics {
        // Tags are added to the ones the topic has, so they go through the same rules as an edit.
        if let Some(csv) = &new_tags {
            let have = bf_db::forum::tags::tags_of(pool, &[*t]).await.db()?;
            let mut all: Vec<String> = have.into_iter().map(|(_, name, _)| name).collect();
            all.push(csv.clone());
            match crate::forum::posting::apply_tags(&state, &v.actor, *t, &all.join(","), true).await {
                Ok(()) => crate::platform::webhooks::moderated(&state, "tag", user.id.get(), "topic", *t, json!({ "tags": csv })).await,
                Err(e) => {
                    tracing::warn!(topic = *t, error = ?e, "bulk tag failed for one topic");
                    failed += 1;
                }
            }
            continue;
        }
        let done: Result<(), sqlx::Error> = async {
            if let Some((flag, on)) = topic_flag(action) {
                topics::set_topic_flag(pool, *t, flag, on, user.id.get()).await?;
                return Ok(());
            }
            match (action, to) {
                ("owner", _) => {
                    if let Some(owner) = new_owner {
                        bf_db::accounts::users::change_author(pool, *t, owner, user.id.get()).await?;
                    }
                }
                ("delete", _) => {
                    let first: Option<i64> =
                        sqlx::query_scalar("SELECT id FROM posts WHERE item_id = $1 AND post_number = 1")
                            .bind(t)
                            .fetch_optional(pool)
                            .await?;
                    match first {
                        Some(first) => posts::soft_delete_post(pool, first, user.id.get()).await?,
                        None => return Err(sqlx::Error::RowNotFound),
                    }
                }
                ("move", Some(to)) => {
                    bf_db::moderation::topics::move_topic(pool, *t, to, user.id.get()).await?;
                    // The same side effects as moving one topic: a link left behind, and the author told.
                    if get("redirect") == "1" {
                        let days = get("redirect_days").trim().parse::<i32>().ok().filter(|d| (1..=3650).contains(d));
                        topics::add_redirect(pool, id, *t, days).await?;
                    }
                }
                _ => {}
            }
            Ok(())
        }
        .await;
        match done {
            Ok(()) => {
                let me = user.id.get();
                match (action, to) {
                    ("delete", _) => {
                        // The topic's author gets a statement of reasons.
                        if let Some(author) = author.filter(|a| *a != me) {
                            let first: Option<i64> =
                                sqlx::query_scalar("SELECT id FROM posts WHERE item_id = $1 AND post_number = 1")
                                    .bind(t)
                                    .fetch_optional(pool)
                                    .await
                                    .ok()
                                    .flatten();
                            if let Some(first) = first {
                                let sid = crate::moderation::dsa::record_silent(
                                    &state,
                                    crate::moderation::dsa::Restriction {
                                        user_id: author,
                                        target_type: "post",
                                        target_id: first,
                                        action: "deleted",
                                        ground: "terms",
                                        category: "scope_of_platform_service",
                                        facts: "A moderator removed the topic as against the site's terms and guidelines.",
                                        automated_detection: false,
                                        automated_decision: false,
                                        notice_id: None,
                                        moderator_id: Some(me),
                                    },
                                )
                                .await;
                                let url = sid.map_or_else(|| "/notifications".to_owned(), |id| format!("/statements/{id}"));
                                let data = json!({ "by": user.username, "title": title, "url": url, "message": "" });
                                crate::community::notifications::send(&state, me, &[author], "moderation", &data).await;
                            }
                        }
                        crate::platform::webhooks::emit(&state, "topic.deleted", json!({ "topic_id": *t, "actor": me }))
                            .await;
                        crate::platform::webhooks::moderated(&state, "delete_topic", me, "topic", *t, json!({})).await;
                    }
                    ("move", Some(dest)) => {
                        crate::platform::webhooks::emit(
                            &state,
                            "topic.moved",
                            json!({ "topic_id": *t, "from": id, "to": dest }),
                        )
                        .await;
                        crate::platform::webhooks::moderated(&state, "move", me, "topic", *t, json!({ "from": id, "to": dest }))
                            .await;
                    }
                    _ => crate::platform::webhooks::moderated(&state, action, me, "topic", *t, json!({})).await,
                }
                if let (Some(author), Some(dest)) = (author.filter(|a| *a != user.id.get()), to)
                    && !crate::community::notifications::can_read(&state, pool, dest, vec![author]).await.is_empty()
                {
                    let data = json!({ "by": user.username, "title": title, "url": topic_path(*t, slug) });
                    crate::community::notifications::send(&state, user.id.get(), &[author], "moved", &data).await;
                }
            }
            Err(e) => {
                tracing::warn!(topic = *t, action, error = %e, "bulk action failed for one topic");
                failed += 1;
            }
        }
    }
    refresh(&state);
    if failed > 0 {
        return Err(AppError::invalid(
            "{n} of the selected topics could not be changed; the others were.",
            &[("n", &failed)],
        ));
    }
    Ok(Redirect::to(&forum_path(id, &node.slug)).into_response())
}

#[derive(Deserialize)]
struct CopyForm {
    csrf: String,
    title: String,
    to: i64,
}

async fn copy_topic(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<CopyForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let (_, tree) = moderated_topic(&state, &v, id).await?;
    if !tree.node(f.to).is_some_and(|n| n.kind == "forum") || !tree.perms(&v.actor, f.to).has(Perms::MODERATE) {
        return Err(AppError::Forbidden);
    }
    let title = f.title.trim();
    if title.is_empty() {
        return Err(AppError::Validation("The copy needs a title.".into()));
    }
    let title = crate::platform::plugins::filter_text(&state, "item.before_create", "title", title).await?;
    let title = title.as_str();
    let slug = slugify(title);
    let new_id = topics::copy_topic(state.db()?, id, f.to, title, &slug, user.id.get()).await.db()?;
    let payload = json!({ "topic_id": new_id, "post_number": 1, "title": title, "node_id": f.to, "copied_from": id });
    crate::platform::webhooks::emit(&state, "topic.created", payload).await;
    crate::platform::webhooks::moderated(&state, "copy", user.id.get(), "topic", id, json!({ "to": new_id })).await;
    refresh(&state);
    Ok(Redirect::to(&topic_path(new_id, &slug)).into_response())
}

#[derive(Deserialize)]
struct DateForm {
    csrf: String,
    date: String,
}

async fn topic_date(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<DateForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let (t, _) = moderated_topic(&state, &v, id).await?;
    let day = chrono::NaiveDate::parse_from_str(f.date.trim(), "%Y-%m-%d")
        .map_err(|_| AppError::Validation("Use a date like 2026-01-31.".into()))?;
    #[expect(clippy::expect_used, reason = "12:00:00 is a valid time of day")]
    let at = day.and_hms_opt(12, 0, 0).expect("valid time").and_utc();
    topics::set_topic_date(state.db()?, id, at, user.id.get()).await.db()?;
    state.microcache.clear();
    Ok(Redirect::to(&topic_path(id, &t.slug)).into_response())
}
