//! Post moderation: hiding, history and revisions, restoring, notes, reports and spammers.

use crate::forum::posting::editable_post;
use crate::forum::topics::ensure_post_visible;
use crate::forum::topics::readable_topic;
use crate::http::render::{ago, DbResult};
use crate::http::render::{layout, page};
use crate::http::viewer::CsrfForm;
use crate::http::viewer::Viewer;
use crate::http::viewer::is_staff;
use crate::moderation::review::moderated_nodes;
use crate::moderation::topics::moderated_topic;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use bf_core::AppError;
use bf_core::perms::Perms;
use bf_core::slug::{profile_path, topic_path};
use bf_db::accounts::users;
use bf_db::admin::maintenance;
use bf_db::forum::topics::TopicRow;
use bf_db::forum::{posts, topics};
use bf_db::moderation::review;
use bf_web::pages::{DeletedPage, DeletedView, HistoryPage, RevisionView, render};
use serde::Deserialize;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/post/{id}/report", post(report))
        .route("/post/{id}/hide", post(hide))
        .route("/u/{id}/notes", post(add_note))
        .route("/api/v1/admin/users/{id}/delete-spammer", post(delete_spammer))
        .route("/review/deleted", get(deletion_log))
        .route("/post/{id}/restore", post(restore))
        .route("/post/{id}/history", get(history))
        .route("/post/{id}/revisions/{n}/hide", post(hide_revision))
        .route("/post/{id}/revisions/{n}/revert", post(revert))
        .route("/post/{id}/edit-lock", post(edit_lock))
        .route("/api/v1/posts/{id}/revisions", get(revisions_api))
        .route("/post/{id}/revisions/{n}/delete", post(delete_revision))
}

#[derive(Deserialize)]
pub(crate) struct ReportForm {
    csrf: String,
    reason: String,
    #[serde(default)]
    note: String,
}

pub(crate) async fn report(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<ReportForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    crate::http::ratelimit::REPORTS_BY_USER.enforce(&format!("report:{}", user.id))?;
    let pool = state.db()?;
    let p = posts::post_ref(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    let (t, _, perms) = readable_topic(&state, &v, p.item_id).await?;
    // A post the viewer cannot see does not exist for them, and cannot be reported.
    ensure_post_visible(&p, perms, &v)?;
    let chosen = crate::moderation::report_reasons::find(&f.reason);
    let reason = chosen.key.as_str();
    let note: String = f.note.trim().chars().take(500).collect();
    if (chosen.needs_note || chosen.message_user) && note.is_empty() {
        return Err(AppError::Validation("Please say what is wrong with this post.".into()));
    }
    // A "message the author" reason files no report: the note goes to them privately.
    if chosen.message_user {
        if let Some(author) = p.author_id.filter(|a| *a != user.id.get()) {
            // This is a private message like any other: the same standing, ignore lists and limits.
            crate::community::messages::check_can_send(&v)?;
            crate::moderation::gate::account_ready(&state, user).map_err(AppError::Validation)?;
            crate::community::messages::check_recipients(&state, user.id.get(), &[author]).await?;
            let link = format!("{}{}#post-{}", state.config.base_url, topic_path(t.id, &t.slug), p.post_number);
            let subject: String = format!("About your post in \"{}\"", t.title).chars().take(150).collect();
            let body = format!("{note}\n\n{link}");
            crate::community::messages::deliver_new(&state, (user.id.get(), &user.username), &subject, &body, &[author])
                .await?;
        }
        return Ok(Redirect::to(&format!("{}#post-{}", topic_path(t.id, &t.slug), p.post_number)).into_response());
    }
    let score =
        review::report(pool, user.id.get(), &p, reason, Some(note.as_str()).filter(|n| !n.is_empty()), chosen.weight)
            .await
            .db()?;
    crate::platform::webhooks::fire(
        &state,
        "flag.created",
        json!({ "user_id": user.id.get(), "post_id": id, "topic_id": p.item_id, "reason": reason, "score": score }),
    );
    crate::community::notifications::tell_reviewers(&state, p.node_id, user.id.get()).await;
    // Enough independent reports hide the post until a moderator looks.
    if f64::from(score) >= state.settings().int("flag_hide_score") as f64 {
        let state_now: String = sqlx::query_scalar("SELECT state FROM posts WHERE id = $1")
            .bind(p.id)
            .fetch_one(pool)
            .await
            .db()?;
        if state_now == "visible" && p.post_number > 1 {
            posts::set_hidden(pool, p.id, true, Some("Hidden after reports from the community"), user.id.get())
                .await
                .db()?;
            crate::moderation::topics::refresh(&state);
            if let Some(author) = p.author_id {
                if reason == "spam" {
                    crate::moderation::spam::spam_hidden(&state, author, "Reports from the community hid a post of theirs as spam.").await;
                }
                let data =
                    json!({ "title": t.title, "url": format!("{}#post-{}", topic_path(t.id, &t.slug), p.post_number) });
                crate::community::notifications::send(&state, 0, &[author], "post_hidden", &data).await;
                // Hidden by a threshold rule, so the decision itself was automated.
                let illegal = chosen.illegal;
                crate::moderation::dsa::record(
                    &state,
                    crate::moderation::dsa::Restriction {
                        user_id: author,
                        target_type: "post",
                        target_id: p.id,
                        action: "hidden",
                        ground: if illegal { "illegal" } else { "terms" },
                        category: if illegal { "other" } else { "scope_of_platform_service" },
                        facts: "Several members reported the post, which hides it automatically until a moderator reviews it.",
                        automated_detection: false,
                        automated_decision: true,
                        notice_id: None,
                        moderator_id: None,
                    },
                )
                .await;
            }
        }
    }
    Ok(Redirect::to(&format!("{}#post-{}", topic_path(t.id, &t.slug), p.post_number)).into_response())
}

#[derive(Deserialize)]
struct HideForm {
    csrf: String,
    hidden: String,
    #[serde(default)]
    reason: String,
}

async fn hide(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<HideForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let pool = state.db()?;
    let node: i64 =
        sqlx::query_scalar("SELECT i.node_id FROM posts p JOIN items i ON i.id = p.item_id WHERE p.id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await
            .db()?
            .ok_or(AppError::NotFound)?;
    let tree = state.forum.get(pool).await?;
    if !tree.perms(&v.actor, node).has(Perms::MODERATE) {
        return Err(AppError::Forbidden);
    }
    // Pending posts go through review and deleted ones through restore, never through here.
    let current = posts::post_ref(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    if !matches!(current.state.as_str(), "visible" | "hidden") {
        return Err(AppError::Validation("Only a visible or hidden post can be changed here.".into()));
    }
    let reason: String = f.reason.trim().chars().take(200).collect();
    posts::set_hidden(pool, id, f.hidden == "1", Some(reason.as_str()).filter(|r| !r.is_empty()), user.id.get())
        .await
        .db()?;
    crate::moderation::topics::refresh(&state);
    let p = posts::post_ref(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    let t = topics::topic(pool, p.item_id).await.db()?.ok_or(AppError::NotFound)?;
    crate::platform::webhooks::moderated(
        &state,
        if f.hidden == "1" { "hide_post" } else { "show_post" },
        user.id.get(),
        "post",
        id,
        json!({ "topic_id": t.id, "post_number": p.post_number, "reason": reason }),
    )
    .await;
    // A statement of reasons when a moderator hides someone else's post.
    if f.hidden == "1"
        && current.state == "visible"
        && let Some(author) = p.author_id.filter(|a| *a != user.id)
    {
        let facts = if reason.is_empty() {
            "A moderator hid the post as against the site's terms and guidelines."
        } else {
            reason.as_str()
        };
        crate::moderation::dsa::record(
            &state,
            crate::moderation::dsa::Restriction {
                user_id: author,
                target_type: "post",
                target_id: id,
                action: "hidden",
                ground: "terms",
                category: "scope_of_platform_service",
                facts,
                automated_detection: false,
                automated_decision: false,
                notice_id: None,
                moderator_id: Some(user.id.get()),
            },
        )
        .await;
    }
    Ok(Redirect::to(&format!("{}#post-{}", topic_path(t.id, &t.slug), p.post_number)).into_response())
}

/// The `edit_history_visible_to` setting; staff always see history.
fn check_history_visible(state: &AppState, v: &Viewer, staff: bool, author: Option<i64>) -> Result<(), AppError> {
    let uid = v.user.as_ref().map(|u| u.id);
    let allowed = staff
        || match state.settings().str("edit_history_visible_to") {
            "members" => uid.is_some(),
            "author" => uid.is_some() && uid == author.map(Into::into),
            "everyone" => true,
            // "staff", and a stored value nobody recognises: the strictest choice.
            _ => false,
        };
    if allowed { Ok(()) } else { Err(AppError::Forbidden) }
}

/// The post, its topic, whether the viewer is a moderator here, and whether they may see hidden text.
async fn history_access(
    state: &AppState,
    v: &Viewer,
    id: i64,
) -> Result<(posts::PostRef, TopicRow, bool, bool), AppError> {
    let p = posts::post_ref(state.db()?, id).await.db()?.ok_or(AppError::NotFound)?;
    let (t, _, perms) = readable_topic(state, v, p.item_id).await?;
    ensure_post_visible(&p, perms, v)?;
    let staff = perms.has(Perms::MODERATE);
    check_history_visible(state, v, staff, p.author_id)?;
    let sees_hidden = is_staff(v) || (v.user.is_some() && v.user.as_ref().map(|u| u.id) == p.author_id.map(Into::into));
    Ok((p, t, staff, sees_hidden))
}

/// A post whose topic the viewer moderates, or 403/404.
async fn moderated_post(state: &AppState, v: &Viewer, id: i64) -> Result<(), AppError> {
    let p = posts::post_ref(state.db()?, id).await.db()?.ok_or(AppError::NotFound)?;
    moderated_topic(state, v, p.item_id).await.map(|_| ())
}

async fn history(State(state): State<AppState>, v: Viewer, Path(id): Path<i64>) -> Result<Response, AppError> {
    let pool = state.db()?;
    let (p, t, staff, sees_hidden) = history_access(&state, &v, id).await?;
    let revisions = bf_db::moderation::topics::revisions(pool, id).await.db()?;
    let revisions: Vec<_> = revisions
        .into_iter()
        .map(|mut r| {
            if !sees_hidden {
                r.raw = bf_cook::strip_hidden(&r.raw);
            }
            r
        })
        .collect();
    // Revision n holds the text before edit n; its diff goes to the next newer text.
    let mut newer = if sees_hidden { p.raw.clone() } else { bf_cook::strip_hidden(&p.raw) };
    let mut views = Vec::with_capacity(revisions.len());
    for r in revisions {
        // A staff-hidden revision must not show through, neither here nor in the next older diff.
        let masked = r.hidden && !staff;
        views.push(RevisionView {
            n: r.n,
            diff: if masked { String::new() } else { bf_core::diff::html(&r.raw, &newer) },
            hidden: r.hidden,
            editor: r.editor.unwrap_or_else(|| "deleted".into()),
            reason: r.reason,
            when: ago(r.created_at),
            raw: if masked { String::new() } else { r.raw.clone() },
        });
        if !masked {
            newer = r.raw;
        }
    }
    let can_revert = editable_post(&state, &v, id).await.is_ok();
    let (mut l, set) = layout(&state, &v, Some(&v.t().msg("Edit history")));
    l.noindex = true;
    let body = HistoryPage {
        layout: l,
        post_id: id,
        staff,
        can_revert,
        edit_locked: p.edit_locked,
        post_url: format!("{}#post-{}", topic_path(t.id, &t.slug), p.post_number),
        current: if sees_hidden { p.raw } else { bf_cook::strip_hidden(&p.raw) },
        revisions: views,
    };
    Ok(page(render(&body), set, false))
}

#[derive(Deserialize)]
struct OnForm {
    csrf: String,
    on: String,
}

/// Staff: hide or show one revision.
async fn hide_revision(
    State(state): State<AppState>,
    v: Viewer,
    Path((id, n)): Path<(i64, i32)>,
    Form(f): Form<OnForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let pool = state.db()?;
    moderated_post(&state, &v, id).await?;
    if !bf_db::moderation::topics::set_revision_hidden(pool, id, n, f.on == "1", user.id.get()).await.db()? {
        return Err(AppError::NotFound);
    }
    Ok(Redirect::to(&format!("/post/{id}/history")).into_response())
}

/// Revert to the text before edit `n`; the revert is itself a new revision.
async fn revert(
    State(state): State<AppState>,
    v: Viewer,
    Path((id, n)): Path<(i64, i32)>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let (p, t) = editable_post(&state, &v, id).await?;
    let user = v.require_user()?;
    let pool = state.db()?;
    if !is_staff(&v)
        && bf_db::moderation::topics::revisions(pool, id).await.db()?.iter().any(|r| r.n == n && r.hidden)
    {
        return Err(AppError::Forbidden);
    }
    let raw = bf_db::moderation::topics::revision_raw(pool, id, n).await.db()?.ok_or(AppError::NotFound)?;
    let cooked = crate::platform::plugins::cook(&state, &raw).await;
    let reason = format!("Reverted to the text before edit {n}");
    posts::edit_post(pool, id, user.id.get(), &raw, &cooked.html, bf_cook::COOK_VERSION, Some(&reason))
        .await
        .db()?;
    crate::platform::webhooks::emit(
        &state,
        "post.edited",
        json!({ "topic_id": t.id, "post_number": p.post_number, "editor": user.username, "reverted": true }),
    )
    .await;
    state.microcache.clear();
    Ok(Redirect::to(&format!("{}#post-{}", topic_path(t.id, &t.slug), p.post_number)).into_response())
}

/// Staff: lock a post against edits by its author.
async fn edit_lock(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<OnForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    moderated_post(&state, &v, id).await?;
    bf_db::moderation::topics::set_edit_locked(state.db()?, id, f.on == "1", user.id.get()).await.db()?;
    Ok(Redirect::to(&format!("/post/{id}/history")).into_response())
}

/// `GET /api/v1/posts/{id}/revisions` (API). Hidden revisions are withheld from non-staff.
async fn revisions_api(State(state): State<AppState>, v: Viewer, Path(id): Path<i64>) -> Result<Response, AppError> {
    let (_, _, staff, sees_hidden) = history_access(&state, &v, id).await?;
    let out: Vec<_> = bf_db::moderation::topics::revisions(state.db()?, id)
        .await
        .db()?
        .into_iter()
        .map(|r| {
            let hide = r.hidden && !staff;
            json!({ "n": r.n, "hidden": r.hidden, "raw": if hide { None } else if sees_hidden { Some(r.raw) } else { Some(bf_cook::strip_hidden(&r.raw)) }, "editor": r.editor, "reason": r.reason, "created_at": r.created_at })
        })
        .collect();
    Ok(Json(out).into_response())
}

async fn deletion_log(State(state): State<AppState>, v: Viewer) -> Result<Response, AppError> {
    v.require_user()?;
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    let nodes = moderated_nodes(&tree, &v.actor);
    if nodes.is_empty() {
        return Err(AppError::Forbidden);
    }
    let rows = bf_db::moderation::topics::deleted_posts(pool, &nodes, 100).await.db()?;
    let (mut l, set) = layout(&state, &v, Some(&v.t().msg("Deletion log")));
    l.noindex = true;
    let items = rows
        .into_iter()
        .map(|d| DeletedView {
            post_id: d.post_id,
            post_url: format!("{}#post-{}", topic_path(d.item_id, &d.slug), d.post_number),
            title: d.title,
            author: d.author.unwrap_or_else(|| "deleted".into()),
            deleted_by: d.deleted_by.unwrap_or_else(|| "unknown".into()),
            when: d.deleted_at.map(ago).unwrap_or_default(),
            // The query takes one character more than shown, so only a cut text gets the ellipsis.
            excerpt: match d.excerpt.char_indices().nth(300) {
                Some((at, _)) => format!("{}…", &d.excerpt[..at]),
                None => d.excerpt,
            },
        })
        .collect();
    Ok(page(render(&DeletedPage { layout: l, items }), set, false))
}

async fn restore(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let pool = state.db()?;
    let node: i64 = sqlx::query_scalar(
        "SELECT i.node_id FROM posts p JOIN items i ON i.id = p.item_id WHERE p.id = $1 AND p.state = 'deleted'",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .db()?
    .ok_or(AppError::NotFound)?;
    let tree = state.forum.get(pool).await?;
    if !tree.perms(&v.actor, node).has(Perms::MODERATE) {
        // 404 rather than 403 even in a forum the member can read: the object here is a deleted
        // post, which only staff may know about at all, so "you may not" would let a member
        // enumerate which post ids have been deleted.
        return Err(AppError::NotFound);
    }
    bf_db::moderation::topics::restore_post_override(pool, id, user.id.get(), true).await.db()?;
    crate::moderation::topics::refresh(&state);
    Ok(Redirect::to("/review/deleted").into_response())
}

#[derive(Deserialize)]
struct NoteForm {
    csrf: String,
    body: String,
}

async fn add_note(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<NoteForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let pool = state.db()?;
    if !is_staff(&v) {
        return Err(AppError::Forbidden);
    }
    let body: String = f.body.trim().chars().take(2000).collect();
    if body.is_empty() {
        return Err(AppError::Validation("The note is empty.".into()));
    }
    let p = users::profile(pool, id.into()).await.db()?.ok_or(AppError::NotFound)?;
    bf_db::moderation::topics::add_note(pool, id, user.id.get(), &body).await.db()?;
    Ok(Redirect::to(&profile_path(id, &p.username)).into_response())
}

async fn delete_spammer(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let who = crate::admin::api::admin(&state, &v, &headers, true)?;
    if who == Some(id) {
        return Err(AppError::Validation("You cannot delete yourself as a spammer.".into()));
    }
    let n = bf_db::moderation::topics::delete_spammer(state.db()?, id, who).await.db()?;
    crate::moderation::topics::refresh(&state);
    Ok(Json(json!({ "topics_touched": n })).into_response())
}

/// Permanently deletes a revision, with an audit entry.
async fn delete_revision(
    State(state): State<AppState>,
    v: Viewer,
    Path((id, n)): Path<(i64, i32)>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let pool = state.db()?;
    moderated_post(&state, &v, id).await?;
    if !maintenance::delete_revision(pool, id, n, user.id.get()).await.db()? {
        return Err(AppError::NotFound);
    }
    Ok(Redirect::to(&format!("/post/{id}/history")).into_response())
}
