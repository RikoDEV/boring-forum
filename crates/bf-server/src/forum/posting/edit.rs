//! Editing, deleting and reading the source of a post, and changing its author.

use crate::forum::topics::ensure_post_visible;
use crate::forum::topics::post_url;
use crate::forum::topics::readable_topic;
use crate::http::i18n::tr;
use crate::http::render::DbResult;
use crate::http::viewer::{CsrfForm, Viewer};
use crate::moderation::review::awaiting_approval;
use crate::moderation::topics::moderated_topic;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Form, Json};
use bf_core::AppError;
use bf_core::perms::Perms;
use bf_core::slug::topic_path;
use bf_db::accounts::users;
use bf_db::forum::topics::TopicRow;
use bf_db::forum::{links, posts};
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;

use super::{Activity, BodyForm, check_activity_limits, check_content_rules, check_image_limit, screen, validate_body};

/// Loads a post the viewer may edit or delete: its author (topic not locked) or a moderator.
pub(crate) async fn editable_post(
    state: &AppState,
    v: &Viewer,
    post_id: i64,
) -> Result<(posts::PostRef, TopicRow), AppError> {
    post_access(state, v, post_id, true).await
}

/// `editable_post`, with `wiki` saying whether trusted members may act on a wiki post. Deleting
/// is not an edit: it is the author's or a moderator's alone.
async fn post_access(
    state: &AppState,
    v: &Viewer,
    post_id: i64,
    wiki: bool,
) -> Result<(posts::PostRef, TopicRow), AppError> {
    let user = v.require_user()?;
    let p = posts::post_ref(state.db()?, post_id).await.db()?.ok_or(AppError::NotFound)?;
    let (t, _, perms) = readable_topic(state, v, p.item_id).await?;
    // An archive forum is closed to its members' own edits too, as it is to replies.
    let archive_forum = state.forum.get(state.db()?).await?.node(t.node_id).is_some_and(|n| n.kind == "archive");
    let group = bf_db::accounts::limits::limits(state.db()?, &user.groups).await.db()?;
    let limit = group.edit_minutes.unwrap_or_else(|| state.settings().int("edit_time_limit_minutes"));
    let in_window = within_edit_window(p.created_at, Utc::now(), limit);
    let own = p.author_id == Some(user.id.get())
        && !t.locked
        && !t.archived
        && !archive_forum
        && perms.has(Perms::REPLY)
        && !v.actor.silenced
        && !p.edit_locked
        && in_window;
    if !perms.has(Perms::MODERATE) && p.state != "visible" {
        // A post a moderator hid, or one still waiting for approval, is out of its author's hands.
        return Err(AppError::Forbidden);
    }
    if !(own || perms.has(Perms::MODERATE)) {
        if !wiki {
            return Err(AppError::Forbidden);
        }
        // A wiki post can be edited by anyone trusted enough, under the same topic rules.
        let trusted = i64::from(user.trust_level) >= state.settings().int("wiki_min_trust_level");
        // The edit form shows the source, so a post with hidden blocks is not open to everyone.
        let wiki = trusted
            && !t.locked
            && !t.archived
            && !v.actor.silenced
            && !p.edit_locked
            && !p.raw.contains("[hide]")
            && !p.raw.contains("```hide");
        if !wiki || !bf_db::forum::tools::is_wiki(state.db()?, post_id).await.db()? {
            return Err(AppError::Forbidden);
        }
    }
    Ok((p, t))
}

/// What the editor needs to open inside a post. Only for someone who may edit it: it is the source.
pub(super) async fn post_source(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let (p, _) = editable_post(&state, &v, id).await?;
    let mut res = Json(json!({
        "raw": p.raw,
        "base": p.edit_count,
        "reason_required": state.settings().bool("require_edit_reason"),
    }))
    .into_response();
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    Ok(res)
}

pub(crate) async fn edit(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<BodyForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let (p, t) = editable_post(&state, &v, id).await?;
    validate_body(&state, &f.body).map_err(AppError::Validation)?;
    let user = v.require_user()?;
    // An edit is held to the same rules as a new post: blocked words, plugin filters and limits.
    let tree = state.forum.get(state.db()?).await?;
    let screened =
        screen(&state, &tree, user, &v.actor, t.node_id, &[&f.body], Some(&p.raw), 0).map_err(AppError::Validation)?;
    let filtered = crate::platform::plugins::before_post(&state, user.id.get(), &screened.texts[0]).await?;
    // Replace rules and plugin filters can lengthen the text: the limit applies to what is stored.
    validate_body(&state, &filtered).map_err(AppError::Validation)?;
    let cooked = crate::platform::plugins::cook(&state, &filtered).await;
    let s = state.settings();
    if !user.is_admin
        && !tree.perms(&v.actor, t.node_id).has(Perms::MODERATE)
        && i64::from(user.post_count) < s.int("new_user_posts")
        && cooked.links.len() as i64 > s.int("new_user_max_links")
    {
        return Err(AppError::invalid(
            "New members can include at most {n} links in a post.",
            &[("n", &s.int("new_user_max_links"))],
        ));
    }
    check_image_limit(&state, user, &cooked.html).await?;
    check_content_rules(&state, user, &filtered, &cooked.html)?;
    check_activity_limits(&state, user, Activity::Edit).await?;
    let reason = Some(f.reason.trim()).filter(|r| !r.is_empty());
    // Someone else saved an edit since this form was loaded: do not overwrite it.
    let base: i32 = f.base.parse().map_err(|_| AppError::BadRequest("The edit form is out of date.".into()))?;
    if base != p.edit_count {
        return edit_conflict(&state, id, base, &p, &f.body).await;
    }
    let grace = chrono::Duration::seconds(state.settings().int("edit_grace_seconds"));
    let in_grace = p.author_id == Some(user.id.get())
        && p.edit_count == 0
        && reason.is_none()
        && Utc::now() - p.created_at < grace;
    if !in_grace && reason.is_none() && state.settings().bool("require_edit_reason") {
        return Err(AppError::Validation("Please give a reason for this edit.".into()));
    }
    if in_grace {
        posts::replace_post_text(state.db()?, id, &filtered, &cooked.html, bf_cook::COOK_VERSION)
            .await
            .db()?;
    } else {
        // The base is compared again under the row lock: two saves from one base cannot both win.
        let saved = posts::edit_post_if(
            state.db()?,
            id,
            user.id.get(),
            &filtered,
            &cooked.html,
            bf_cook::COOK_VERSION,
            reason,
            Some(base),
        )
        .await
        .db()?;
        if !saved {
            let now = posts::post_ref(state.db()?, id).await.db()?.ok_or(AppError::NotFound)?;
            return edit_conflict(&state, id, base, &now, &f.body).await;
        }
    }
    links::record_links(state.db()?, t.id, p.post_number, &cooked.links).await.db()?;
    // The edit added something that needs approval: the post waits for a moderator again.
    if screened.pending && posts::requeue_post(state.db()?, p.id).await.db()? {
        crate::moderation::topics::refresh(&state);
        state.hub.publish(&format!("item:{}", p.item_id), "edit", json!({ "post_number": p.post_number }));
        crate::community::notifications::tell_reviewers(&state, t.node_id, user.id.get()).await;
        return Ok(awaiting_approval(&state, &v));
    }
    if let Some(author) = p.author_id.filter(|a| *a != user.id) {
        // Only an author who can still open the topic is told its title and address.
        let readable = crate::community::notifications::can_read(&state, state.db()?, t.node_id, vec![author]).await;
        if !readable.is_empty() {
            let data = json!({ "by": user.username, "title": t.title, "url": format!("{}#post-{}", topic_path(t.id, &t.slug), p.post_number) });
            crate::community::notifications::send(&state, user.id.get(), &[author], "edited", &data).await;
        }
    }
    crate::platform::webhooks::emit(
        &state,
        "post.edited",
        json!({ "topic_id": t.id, "post_number": p.post_number, "editor": user.username }),
    )
    .await;
    state.hub.publish(&format!("item:{}", p.item_id), "edit", json!({ "post_number": p.post_number }));
    state.microcache.clear();
    Ok(Redirect::to(&post_url(&state, t.id, &t.slug, p.post_number)).into_response())
}

/// The 409 answer with both versions, so the editor can merge them.
async fn edit_conflict(
    state: &AppState,
    id: i64,
    base: i32,
    p: &posts::PostRef,
    body: &str,
) -> Result<Response, AppError> {
    let msg = tr(
        "Someone edited this post while you were editing. Compare the changes below, merge them into your text and save again.",
    );
    // Revision n holds the text before edit n, so the form's base text is revision base + 1.
    let base_text = bf_db::moderation::topics::revisions(state.db()?, id)
        .await
        .db()?
        .into_iter()
        .find(|r| r.n == base + 1 && !r.hidden)
        .map_or_else(|| p.raw.clone(), |r| r.raw);
    let answer = json!({
        "detail": msg,
        "base": p.edit_count,
        "theirs": bf_core::diff::html(&base_text, &p.raw),
        "yours": bf_core::diff::html(&base_text, body),
    });
    Ok((StatusCode::CONFLICT, Json(answer)).into_response())
}

pub(crate) async fn delete(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let (p, t) = post_access(&state, &v, id, false).await?;
    if p.post_number == 1 {
        return Err(AppError::BadRequest("The first post cannot be deleted on its own.".into()));
    }
    let user = v.require_user()?;
    let pool = state.db()?;
    posts::soft_delete_post(pool, id, user.id.get()).await.db()?;
    bf_db::admin::audit::record(pool, bf_db::admin::audit::Entry::new("post.delete").by(user.id.get()).on(format!("post:{id}")))
        .await
        .db()?;
    state.microcache.clear();
    // Open copies of the topic fetch the post again, and so drop it.
    state.hub.publish(&format!("item:{}", t.id), "edit", json!({ "post_number": p.post_number }));
    // Staff removing someone else's post owe them a statement of reasons.
    let staff = readable_topic(&state, &v, p.item_id).await?.2.has(Perms::MODERATE);
    if let Some(author) = p.author_id.filter(|a| staff && *a != user.id) {
        crate::moderation::dsa::record(
            &state,
            crate::moderation::dsa::Restriction {
                user_id: author,
                target_type: "post",
                target_id: id,
                action: "deleted",
                ground: "terms",
                category: "scope_of_platform_service",
                facts: "A moderator removed the post as against the site's terms and guidelines.",
                automated_detection: false,
                automated_decision: false,
                notice_id: None,
                moderator_id: Some(user.id.get()),
            },
        )
        .await;
    }
    crate::federation::post_deleted(&state, id, p.author_id).await;
    crate::platform::webhooks::emit(
        &state,
        "post.deleted",
        json!({ "topic_id": t.id, "post_number": p.post_number, "actor": user.id }),
    )
    .await;
    if staff && p.author_id != Some(user.id.get()) {
        crate::platform::webhooks::moderated(
            &state,
            "delete_post",
            user.id.get(),
            "post",
            id,
            json!({ "topic_id": t.id, "post_number": p.post_number }),
        )
        .await;
    }
    Ok(Redirect::to(&topic_path(t.id, &t.slug)).into_response())
}

/// Markdown source of a readable post, for quote-replies and "view source".
pub(super) async fn post_raw(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let pool = state.db()?;
    let p = posts::post_ref(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    let (t, _, perms) = readable_topic(&state, &v, p.item_id).await?;
    ensure_post_visible(&p, perms, &v)?;
    // Quoting must not hand out what the page would not show.
    let viewer = v.user.as_ref().map(|u| u.id);
    let replied = match viewer {
        Some(u) => bf_db::community::onboarding::has_posted(pool, p.item_id, u.get()).await.db()?,
        None => false,
    };
    let source = if perms.has(Perms::MODERATE) || replied || (viewer.is_some() && viewer == p.author_id.map(Into::into))
    {
        p.raw.clone()
    } else {
        bf_cook::strip_hidden(&p.raw)
    };
    let author = match p.author_id {
        Some(a) => users::profile(pool, a.into()).await.db()?.map(|u| u.username),
        None => None,
    };
    let mut res = Json(json!({
        "raw": source,
        "author": author,
        "post_number": p.post_number,
        "url": format!("{}#post-{}", topic_path(t.id, &t.slug), p.post_number),
    }))
    .into_response();
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    Ok(res)
}

#[derive(Deserialize)]
pub(super) struct AuthorForm {
    pub(super) csrf: String,
    pub(super) username: String,
}

pub(super) async fn change_author(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<AuthorForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let (t, _) = moderated_topic(&state, &v, id).await?;
    let pool = state.db()?;
    let to = users::user_id_by_name(pool, f.username.trim())
        .await
        .db()?
        .ok_or_else(|| AppError::Validation("No member has that username.".into()))?;
    // Site-wide staff only, and an administrator's or moderator's name only by an administrator.
    crate::moderation::topics::may_change_author(&state, &v, to.get()).await?;
    users::change_author(pool, id, to.get(), user.id.get()).await.db()?;
    state.microcache.clear();
    Ok(Redirect::to(&topic_path(id, &t.slug)).into_response())
}

/// Whether a post made at `created` can still be edited at `now`; `limit` is minutes, 0 for no
/// limit. A group limit is an admin-typed number, and a huge one must not overflow `chrono`'s
/// duration (which panics): it just means "always".
fn within_edit_window(created: chrono::DateTime<Utc>, now: chrono::DateTime<Utc>, limit: i64) -> bool {
    limit == 0 || chrono::Duration::try_minutes(limit).is_none_or(|window| now - created < window)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_huge_group_limit_means_always_not_a_panic() {
        let now = Utc::now();
        let old = now - chrono::Duration::days(3650);
        assert!(within_edit_window(old, now, i64::MAX));
        assert!(within_edit_window(old, now, 0));
        assert!(!within_edit_window(old, now, 60));
        assert!(within_edit_window(now, now, 60));
    }
}
