//! The review queue.

use crate::forum::posting::announce_post;
use crate::forum::tree::Tree;
use crate::http::render::db_err;
use crate::http::render::layout;
use crate::http::render::page;
use crate::http::render::{DbResult, ago};
use crate::http::viewer::Viewer;
use crate::http::viewer::is_staff;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use bf_core::AppError;
use bf_core::perms::{Actor, Perms};
use bf_core::slug::{profile_path, topic_path};
use bf_db::moderation::review;
use bf_web::pages::{ErrorPage, ReviewPage, ReviewView, render};
use serde::Deserialize;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/review/{id}/claim", post(claim_review))
        .route("/review", get(review))
        .route("/review/{id}", post(resolve))
        .route("/review/user/{id}", post(decide_user))
        .route("/u/{id}/report", post(report_member))
        .route("/review/member/{id}", post(close_member_report))
}

#[derive(Deserialize)]
struct ClaimForm {
    csrf: String,
    #[serde(default)]
    release: String,
}

/// Claims a review item, so two moderators do not work on the same one.
async fn claim_review(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<ClaimForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    let node = review::reviewable_node(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    if !tree.perms(&v.actor, node).has(Perms::MODERATE) {
        return Err(AppError::Forbidden);
    }
    review::claim_review(pool, id, (f.release != "1").then_some(user.id.get())).await.db()?;
    Ok(Redirect::to("/review").into_response())
}

pub(crate) fn awaiting_approval(state: &AppState, v: &Viewer) -> Response {
    let (l, set) = layout(state, v, Some(&v.t().msg("Awaiting approval")));
    let p = ErrorPage {
        layout: l,
        status: 202,
        message: v.t().msg("Thanks! Your post will appear once a moderator approves it."),
    };
    page((StatusCode::ACCEPTED, render(&p)).into_response(), set, false)
}

/// Forums where the viewer can moderate; empty for regular members.
pub(crate) fn moderated_nodes(tree: &Tree, actor: &Actor) -> Vec<i64> {
    tree.nodes.iter().filter(|n| tree.perms(actor, n.id).has(Perms::MODERATE)).map(|n| n.id).collect()
}

/// Pending review items in the forums this actor moderates, or `None` if there are none to moderate.
pub(crate) async fn review_count(state: &AppState, pool: &sqlx::PgPool, actor: &Actor) -> Option<i64> {
    let tree = state.forum.get(pool).await.ok()?;
    let nodes = moderated_nodes(&tree, actor);
    if nodes.is_empty() {
        return None;
    }
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM reviewables WHERE status = 'pending' AND node_id = ANY($1)")
            .bind(&nodes)
            .fetch_one(pool)
            .await
            .unwrap_or(0);
    // Global staff also handle pending accounts, member reports and reported conversations.
    let staff = actor.is_admin || actor.groups.contains(&bf_core::ids::GroupId::MODERATORS);
    let staff_items = if staff { review::staff_queue_count(pool).await.unwrap_or(0) } else { 0 };
    let held = bf_db::moderation::held::count(pool, staff, &nodes).await.unwrap_or(0);
    let from_plugins = if staff { crate::platform::plugins::held_for_review_count(state).await } else { 0 };
    Some(count + staff_items + held + from_plugins)
}

#[derive(Deserialize)]
struct ReviewQuery {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    forum: String,
    #[serde(default)]
    sort: String,
    /// What was just done (set by the redirect after an action).
    #[serde(default)]
    done: String,
}

async fn review(State(state): State<AppState>, v: Viewer, Query(q): Query<ReviewQuery>) -> Result<Response, AppError> {
    v.require_user()?;
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    let nodes = moderated_nodes(&tree, &v.actor);
    if nodes.is_empty() {
        return Err(AppError::Forbidden);
    }
    let forum = q.forum.parse::<i64>().ok();
    let scope: Vec<i64> = nodes.iter().copied().filter(|n| forum.is_none_or(|f| f == *n)).collect();
    let kind = match q.kind.as_str() {
        "report" => Some("report"),
        "pending_post" => Some("pending_post"),
        _ => None,
    };
    let rows = review::review_queue(pool, &scope, kind, q.sort == "age").await.db()?;
    let items = rows
        .into_iter()
        .map(|r| ReviewView {
            id: r.id,
            pending_post: r.kind == "pending_post",
            claimed_by: r.claimed_by,
            reasons: r.reasons.unwrap_or_default().join(", "),
            score: r.score,
            post_url: format!("{}#post-{}", topic_path(r.item_id, &r.slug), r.post_number),
            title: r.title,
            author: r.author.unwrap_or_else(|| "deleted".into()),
            cooked: r.cooked,
            raw: r.raw,
            when: ago(r.created_at),
        })
        .collect();
    let pending_users = if is_staff(&v) {
        bf_db::moderation::review::pending_users(pool)
            .await
            .db()?
            .into_iter()
            .map(|(id, name, email, joined)| (id, name, email, ago(joined)))
            .collect()
    } else {
        vec![]
    };
    let (member_reports, message_reports) = if is_staff(&v) {
        let members = review::open_member_reports(pool)
            .await
            .db()?
            .into_iter()
            .map(|(id, uid, name, by, reason, at, status)| {
                // A report on one status update links straight to it.
                let url = match status {
                    Some(s) => format!("{}?tab=status#status-{s}", profile_path(uid, &name)),
                    None => profile_path(uid, &name),
                };
                (id, url, name, by.unwrap_or_else(|| "deleted".into()), reason, ago(at))
            })
            .collect();
        let messages = bf_db::community::messages::open_conversation_report_count(pool).await.db()?;
        (members, messages)
    } else {
        (vec![], 0)
    };
    let held = bf_db::moderation::held::pending(pool, is_staff(&v), &nodes)
        .await
        .db()?
        .iter()
        .map(crate::moderation::held::view)
        .collect();
    let (mut l, set) = layout(&state, &v, Some(&v.t().msg("Review queue")));
    l.noindex = true;
    let forums = tree
        .nodes
        .iter()
        .filter(|n| nodes.contains(&n.id) && n.kind != "category")
        .map(|n| (n.id, n.title.clone(), forum == Some(n.id)))
        .collect();
    let chat_flags = if is_staff(&v) {
        bf_db::community::chat::flags(pool)
            .await
            .db()?
            .into_iter()
            .map(|f| {
                (
                    f.message_id,
                    f.channel_id,
                    f.author.unwrap_or_else(|| "deleted".into()),
                    f.flagged_by,
                    f.reason,
                    f.raw,
                )
            })
            .collect()
    } else {
        vec![]
    };
    Ok(page(
        render(&ReviewPage {
            chat_flags,
            layout: l,
            items,
            pending_users,
            held,
            member_reports,
            message_reports,
            forums,
            kind: q.kind,
            sort: q.sort,
            staff: is_staff(&v),
            notice: match q.done.as_str() {
                "keep" | "chat_keep" => v.t().msg("The post stays and the item is closed."),
                "remove" => v.t().msg("The post is removed."),
                "silence" => v.t().msg("The post is removed and its author is silenced."),
                "spammer" => v.t().msg("The spammer's posts are deleted and the account is suspended."),
                "approve_user" => v.t().msg("The account is approved."),
                "reject_user" => v.t().msg("The account is rejected."),
                "approve_held" => v.t().msg("The held text is approved."),
                "reject_held" => v.t().msg("The held text is rejected."),
                "closed" => v.t().msg("The report is closed."),
                "chat_remove" => v.t().msg("The message is removed from the chat."),
                _ => String::new(),
            },
        }),
        set,
        false,
    ))
}

#[derive(Deserialize)]
struct DecisionForm {
    csrf: String,
    decision: String,
    /// Sent to the author when their post is rejected or removed.
    #[serde(default)]
    message: String,
    /// "Edit, then approve": the moderator's version of the text, published instead of the original.
    #[serde(default)]
    edited: String,
}

async fn resolve(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<DecisionForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    let node = review::reviewable_node(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    if !tree.perms(&v.actor, node).has(Perms::MODERATE) {
        return Err(AppError::Forbidden);
    }
    // "silence" and "spammer" agree with the report and also act on the author (staff only).
    let decision = match f.decision.as_str() {
        "keep" => review::Decision::Keep,
        "remove" | "silence" | "spammer" => review::Decision::Remove,
        _ => return Err(AppError::BadRequest("unknown decision".into())),
    };
    let also = matches!(f.decision.as_str(), "silence" | "spammer");
    if also && !is_staff(&v) {
        return Err(AppError::Forbidden);
    }
    let author: Option<(i64, String, String, i32, i64)> = sqlx::query_as(
        "SELECT p.author_id, i.title, i.slug, p.post_number, i.id FROM reviewables r JOIN posts p ON p.id = r.target_id
         JOIN items i ON i.id = p.item_id WHERE r.id = $1 AND r.target_type = 'post' AND p.author_id IS NOT NULL",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .db()?;
    // The member the extra action is for, checked before anything is decided.
    let target = if also {
        let (author_id, ..) = author.as_ref().ok_or(AppError::NotFound)?;
        Some(crate::moderation::discipline::member(&state, &v, *author_id).await?)
    } else {
        None
    };
    let queued: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM reviewables WHERE id = $1 AND kind = 'pending_post')")
            .bind(id)
            .fetch_one(pool)
            .await
            .db()?;
    // Held again after an edit: it was announced when first published, so it is not again.
    let edited: bool =
        sqlx::query_scalar("SELECT coalesce((payload->>'edited')::boolean, false) FROM reviewables WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await
            .db()?
            .unwrap_or(false);
    let post_id: Option<i64> =
        sqlx::query_scalar("SELECT target_id FROM reviewables WHERE id = $1 AND kind = 'report'")
            .bind(id)
            .fetch_optional(pool)
            .await
            .db()?;
    let reporters = match post_id {
        Some(p) => bf_db::moderation::review::reporters(pool, p).await.db()?,
        None => vec![],
    };
    // A post that waited for approval is published as its author, who must still be allowed to
    // write: one silenced, un-approved or suspended meanwhile is not published (a moderator can
    // still reject it), as for the held member writes.
    if queued
        && decision == review::Decision::Keep
        && let Some((author_id, ..)) = &author
    {
        let u = bf_db::accounts::users::user_by_id(pool, (*author_id).into()).await.db()?;
        let allowed = match u {
            Some(u) => {
                !u.silenced && u.approved && !bf_db::accounts::users::is_suspended(pool, *author_id).await.db()?
            }
            None => false,
        };
        if !allowed {
            return Err(AppError::Validation(
                "The author of this item can no longer post, so it was not published.".into(),
            ));
        }
    }
    // "Edit, then approve": the moderator's text replaces the original, as a revision, before it goes public.
    if decision == review::Decision::Keep && !f.edited.trim().is_empty() {
        let post: Option<(i64, String)> = sqlx::query_as(
            "SELECT p.id, p.raw FROM reviewables r JOIN posts p ON p.id = r.target_id WHERE r.id = $1 AND r.target_type = 'post'",
        )
        .bind(id)
        .fetch_optional(pool)
        .await
        .db()?;
        if let Some((post_id, old)) = post
            && old.trim() != f.edited.trim()
        {
            let raw: String = f.edited.trim().chars().take(60_000).collect();
            let cooked = crate::platform::plugins::cook(&state, &raw).await;
            bf_db::forum::posts::edit_post(
                pool,
                post_id,
                user.id.get(),
                &raw,
                &cooked.html,
                bf_cook::COOK_VERSION,
                Some("Edited by a moderator before approval"),
            )
            .await
            .db()?;
        }
    }
    // A double click, or another moderator first: the item is no longer pending, which is not a fault.
    review::resolve_review(pool, id, decision, user.id.get()).await.map_err(|e| match e {
        sqlx::Error::RowNotFound => AppError::Validation("Someone has already decided this item.".into()),
        e => db_err(e),
    })?;
    if let (false, Some((_, title, _, _, _))) = (reporters.is_empty(), &author) {
        let data = json!({ "by": user.username, "title": title, "url": "/notifications" });
        crate::community::notifications::send(&state, user.id.get(), &reporters, "report_resolved", &data).await;
    }
    if let Some(target) = &target {
        let me = user.id.get();
        if f.decision == "silence" {
            let hours = state.settings().int("flag_auto_silence_hours");
            let hours = if hours > 0 { hours } else { 72 };
            crate::moderation::spam::silence_for(
                &state,
                me,
                target.id.get(),
                hours,
                "A post of theirs was reported as spam.",
                false,
            )
            .await;
        } else {
            crate::moderation::discipline::clean_up_spammer(&state, &v, target, false).await?;
        }
    }
    let message: String = f.message.trim().chars().take(500).collect();
    let post_target: i64 =
        sqlx::query_scalar("SELECT target_id FROM reviewables WHERE id = $1").bind(id).fetch_one(pool).await.db()?;
    if queued && decision == review::Decision::Remove {
        bf_db::moderation::topics::set_deleted_reason(pool, post_target, "rejected").await.db()?;
    }
    if let Some((author_id, title, slug, n, item)) = author {
        let url = format!("{}#post-{n}", topic_path(item, &slug));
        match (queued, decision) {
            (true, review::Decision::Keep) => {
                let data = json!({ "title": title, "url": url });
                crate::community::notifications::send(&state, user.id.get(), &[author_id], "post_approved", &data)
                    .await;
                if !edited {
                    announce_post(&state, item, n).await;
                }
            }
            (true, review::Decision::Remove) => {
                let url = removal_statement(&state, &v, author_id, post_target, true, &message).await;
                let data = json!({ "title": title, "url": url, "message": message });
                crate::community::notifications::send(&state, user.id.get(), &[author_id], "post_rejected", &data)
                    .await;
            }
            (false, review::Decision::Remove) => {
                let url = removal_statement(&state, &v, author_id, post_target, false, &message).await;
                let data = json!({ "by": user.username, "title": title, "url": url, "message": message });
                crate::community::notifications::send(&state, user.id.get(), &[author_id], "moderation", &data).await;
            }
            (false, review::Decision::Keep) => {}
        }
    }
    crate::moderation::topics::refresh(&state);
    let done = match f.decision.as_str() {
        d @ ("keep" | "remove" | "silence" | "spammer") => d,
        _ => "keep",
    };
    Ok(Redirect::to(&format!("/review?done={done}")).into_response())
}

/// (DSA Art. 17): removing a member's post from the queue records a statement of reasons;
/// returns the link the member's notification should point to.
async fn removal_statement(
    state: &AppState,
    v: &Viewer,
    author: i64,
    post: i64,
    never_published: bool,
    message: &str,
) -> String {
    let facts = if message.is_empty() {
        if never_published {
            "A moderator rejected the post, which was waiting for approval, as against the site's terms and guidelines."
        } else {
            "A moderator removed the post as against the site's terms and guidelines."
        }
    } else {
        message
    };
    let moderator = v.user.as_ref().map(|u| u.id.get());
    let id = crate::moderation::dsa::record_silent(
        state,
        crate::moderation::dsa::Restriction {
            user_id: author,
            target_type: "post",
            target_id: post,
            action: "deleted",
            ground: "terms",
            category: "scope_of_platform_service",
            facts,
            automated_detection: false,
            automated_decision: false,
            notice_id: None,
            moderator_id: moderator,
        },
    )
    .await;
    id.map_or_else(|| "/".to_owned(), |id| format!("/statements/{id}"))
}

#[derive(Deserialize)]
struct UserDecision {
    csrf: String,
    decision: String,
    /// On reject: ban the address so it cannot sign up again.
    #[serde(default)]
    ban_email: String,
}

async fn decide_user(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<UserDecision>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    if !is_staff(&v) {
        return Err(AppError::Forbidden);
    }
    let pool = state.db()?;
    let done = match f.decision.as_str() {
        "approve" => review::approve_user(pool, id, user.id.get()).await.db()?,
        "reject" => review::reject_user(pool, id, f.ban_email == "1", user.id.get()).await.db()?,
        _ => return Err(AppError::BadRequest("unknown decision".into())),
    };
    if !done {
        return Err(AppError::NotFound);
    }
    if f.decision == "approve" {
        let data = json!({ "url": "/" });
        crate::community::notifications::send(&state, user.id.get(), &[id], "account_approved", &data).await;
    }
    state.sessions.invalidate_all();
    state.microcache.clear();
    let done = if f.decision == "approve" { "approve_user" } else { "reject_user" };
    Ok(Redirect::to(&format!("/review?done={done}")).into_response())
}

#[derive(Deserialize)]
struct MemberReportForm {
    csrf: String,
    reason: String,
    /// One status update of theirs, when the report is about that.
    #[serde(default)]
    status: Option<i64>,
}

/// Reports a member: their profile, bio, signature or status updates.
async fn report_member(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<MemberReportForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    crate::http::ratelimit::REPORTS_BY_USER.enforce(&format!("report:{}", user.id))?;
    let pool = state.db()?;
    let p = bf_db::accounts::users::profile(pool, id.into()).await.db()?.ok_or(AppError::NotFound)?;
    let reason = f.reason.trim();
    if reason.is_empty() || reason.chars().count() > 1000 {
        return Err(AppError::Validation("Say what is wrong, in up to 1000 characters.".into()));
    }
    // The status must be this member's, or the report would point somewhere else.
    let status = match f.status {
        Some(s) => match bf_db::community::statuses::get(pool, s).await.db()? {
            Some(st) if st.user_id == id => Some(s),
            _ => return Err(AppError::NotFound),
        },
        None => None,
    };
    if review::report_member(pool, id, user.id.get(), reason, status).await.db()? {
        crate::community::notifications::tell_staff(&state, user.id.get()).await;
    }
    let back = match status {
        Some(s) => format!("{}?tab=status#status-{s}", profile_path(id, &p.username)),
        None => profile_path(id, &p.username),
    };
    Ok(Redirect::to(&back).into_response())
}

async fn close_member_report(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<ClaimForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    if !is_staff(&v) {
        return Err(AppError::Forbidden);
    }
    if !review::close_member_report(state.db()?, id, user.id.get()).await.db()? {
        return Err(AppError::NotFound);
    }
    Ok(Redirect::to("/review?done=closed").into_response())
}
