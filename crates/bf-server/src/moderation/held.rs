//! Member writes held by a "require approval" watched word outside forum posts: private
//! messages, chat, statuses, profiles, signatures, topic renames, clubs and chat channels.
//! Nothing is written while it waits; approving performs the write the member asked for,
//! through the same code the original request would have used.

use crate::http::i18n::tr;
use crate::http::render::{DbResult, layout, page};
use crate::http::viewer::{Viewer, is_staff};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::post;
use axum::{Form, Json, Router};
use bf_core::AppError;
use bf_core::i18n::en;
use bf_core::perms::Perms;
use bf_db::moderation::held::{self as db, Held};
use bf_web::pages::{ErrorPage, render};
use serde::Deserialize;
use serde_json::{Value, json};

pub const HELD_TEXT: &str = en("Thanks! It will appear once a moderator approves it.");

pub fn router() -> Router<AppState> {
    Router::new().route("/review/held/{id}", post(decide))
}

/// Holds a write and tells whoever decides it: that forum's moderators for a topic rename,
/// global staff for everything else.
pub(crate) async fn hold(
    state: &AppState,
    user: i64,
    kind: &str,
    node: Option<i64>,
    payload: Value,
) -> Result<(), AppError> {
    db::hold(state.db()?, kind, user, node, &payload).await.db()?;
    match node {
        Some(n) => crate::community::notifications::tell_reviewers(state, n, user).await,
        None => crate::community::notifications::tell_staff(state, user).await,
    }
    Ok(())
}

/// The answer to a form whose write was held.
pub(crate) fn held_page(state: &AppState, v: &Viewer) -> Response {
    let (l, set) = layout(state, v, Some(&v.t().msg("Awaiting approval")));
    let p = ErrorPage { layout: l, status: 202, message: tr(HELD_TEXT) };
    page((StatusCode::ACCEPTED, render(&p)).into_response(), set, false)
}

/// The answer to an API write (chat) that was held.
pub(crate) fn held_json() -> Response {
    (StatusCode::ACCEPTED, Json(json!({ "held": true, "message": tr(HELD_TEXT) }))).into_response()
}

/// What the member wrote, in words for the queue and their notification.
pub(crate) fn label(kind: &str) -> &'static str {
    match kind {
        "conversation" => en("private message"),
        "message" => en("private message reply"),
        "chat" => en("chat message"),
        "chat_edit" => en("chat message edit"),
        "status" => en("status update"),
        "status_reply" => en("status reply"),
        "profile" => en("profile changes"),
        "signature" => en("signature"),
        "title" => en("topic title"),
        "club" => en("club"),
        "chat_channel" => en("chat channel"),
        _ => en("post"),
    }
}

/// One held item for the review page: (id, what, author, text to read, filed).
pub(crate) fn view(h: &Held) -> (i64, String, String, String, bf_web::ago::Ago) {
    let s = |k: &str| h.payload.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
    let text = match h.kind.as_str() {
        "conversation" => format!("{}\n\n{}", s("subject"), s("body")),
        "message" | "status" | "status_reply" => s("body"),
        "chat" | "chat_edit" => s("raw"),
        "signature" => s("signature"),
        "title" => s("title"),
        "club" => [s("name"), s("description")].into_iter().filter(|t| !t.trim().is_empty()).collect::<Vec<_>>().join(
            "

",
        ),
        "chat_channel" => format!("{}\n\n{}", s("name"), s("description")),
        "profile" => ["display_name", "location", "website", "about"]
            .iter()
            .filter(|k| h.payload.get(**k).is_some())
            .map(|k| format!("{}: {}", k.replace('_', " "), s(k)))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    };
    (h.id, tr(label(&h.kind)), h.author.clone(), text, crate::http::render::ago(h.created_at))
}

/// May this viewer decide this item?
///
/// NEEDS OWNER DECISION: held `conversation` and `message` items have no forum (`node_id` is
/// `None`), so any global staff member passes the `is_staff` test below and can read and decide
/// them in /review. That is intended for items a watched word caught before delivery: someone has
/// to approve or reject them, and nobody else could. It does differ from `community/messages.rs`,
/// where staff open a delivered conversation only through a participant's report. Whether
/// reviewing held private text should be narrower (for example admins only, or text hidden until
/// opened) is the owner's call; behaviour is deliberately unchanged.
async fn may_decide(state: &AppState, v: &Viewer, h: &Held) -> Result<bool, AppError> {
    if is_staff(v) {
        return Ok(true);
    }
    let Some(node) = h.node_id else { return Ok(false) };
    let tree = state.forum.get(state.db()?).await?;
    Ok(tree.perms(&v.actor, node).has(Perms::MODERATE))
}

#[derive(Deserialize)]
struct DecisionForm {
    csrf: String,
    decision: String,
    /// Sent to the member when their text is rejected.
    #[serde(default)]
    message: String,
}

async fn decide(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<DecisionForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let pool = state.db()?;
    let h = db::get(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    if !may_decide(&state, &v, &h).await? {
        return Err(AppError::Forbidden);
    }
    let approve = match f.decision.as_str() {
        "approve" => true,
        "reject" => false,
        _ => return Err(AppError::BadRequest("unknown decision".into())),
    };
    // The write is made as its author, so the author must still be allowed to write: a member who
    // was suspended, silenced, removed or un-approved while it waited does not get it published.
    if approve {
        author_still_allowed(pool, h.user_id).await?;
    }
    // Claimed first, so two moderators approving at once cannot write it twice.
    if !db::resolve(pool, id, approve, user.id.get()).await.db()? {
        return Err(AppError::NotFound);
    }
    let what = label(&h.kind);
    if approve {
        match apply(&state, &h).await {
            Ok(true) => {
                let data = json!({ "name": what, "url": "/notifications" });
                crate::community::notifications::send(&state, user.id.get(), &[h.user_id], "content_approved", &data)
                    .await;
            }
            // The rules that applied when it was written no longer let it through (the recipient
            // now ignores the author, the topic is locked, ...): it is dropped without telling anyone.
            Ok(false) => tracing::info!(held = id, "held item dropped: no longer allowed"),
            Err(e) => {
                // The conversation, channel or topic went away while it waited.
                tracing::info!(held = id, error = ?e, "held item could not be applied");
                // Not lost: it goes back to the queue, so a moderator can retry or reject it.
                db::reopen(pool, id).await.db()?;
                return Err(AppError::invalid(
                    "This {what} can no longer be published: {e}",
                    &[("what", &tr(what)), ("e", &e)],
                ));
            }
        }
    } else {
        let message: String = f.message.trim().chars().take(500).collect();
        // Nothing was published, but the member still gets a statement of reasons.
        let facts = if message.is_empty() {
            "A moderator rejected this text as against the site's terms and guidelines."
        } else {
            message.as_str()
        };
        let statement = crate::moderation::dsa::record_silent(
            &state,
            crate::moderation::dsa::Restriction {
                user_id: h.user_id,
                target_type: "account",
                target_id: h.user_id,
                action: "deleted",
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
        let url = statement.map_or_else(|| "/notifications".to_owned(), |id| format!("/statements/{id}"));
        let data = json!({ "name": what, "url": url, "message": message });
        crate::community::notifications::send(&state, user.id.get(), &[h.user_id], "content_rejected", &data).await;
    }
    state.microcache.clear();
    Ok(Redirect::to("/review").into_response())
}

/// Refuses an approval whose author can no longer write.
async fn author_still_allowed(pool: &sqlx::PgPool, user: i64) -> Result<(), AppError> {
    let u = bf_db::accounts::users::user_by_id(pool, user.into()).await.db()?;
    let Some(u) = u else {
        return Err(AppError::Validation("The author of this item no longer exists.".into()));
    };
    if u.silenced || !u.approved || bf_db::accounts::users::is_suspended(pool, user).await.db()? {
        return Err(AppError::Validation(
            "The author of this item can no longer post, so it was not published.".into(),
        ));
    }
    Ok(())
}

/// Performs the held write as its author, after checking again what the original request would
/// have checked. `Ok(false)` means the rules no longer allow it and it was dropped.
async fn apply(state: &AppState, h: &Held) -> Result<bool, AppError> {
    let pool = state.db()?;
    let author = bf_db::accounts::users::user_by_id(pool, h.user_id.into()).await.db()?.ok_or(AppError::NotFound)?;
    let staff = author.is_admin || author.groups.contains(&bf_core::ids::GroupId::MODERATORS);
    let p = &h.payload;
    let s = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
    let n = |k: &str| p.get(k).and_then(Value::as_i64).ok_or(AppError::NotFound);
    let from = (h.user_id, h.author.as_str());
    match h.kind.as_str() {
        "conversation" => {
            let to: Vec<i64> =
                p["to"].as_array().map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
            // A recipient who now ignores the sender, or takes messages from nobody, gets nothing.
            if crate::community::messages::check_recipients(state, h.user_id, &to).await.is_err() {
                return Ok(false);
            }
            crate::community::messages::deliver_new(state, from, &s("subject"), &s("body"), &to).await?;
        }
        "message" => crate::community::messages::deliver_reply(state, n("conversation")?, from, &s("body")).await?,
        "chat" => {
            let reply_to = p.get("reply_to").and_then(Value::as_i64);
            let channel = n("channel")?;
            crate::community::chat::check_can_deliver(state, channel, h.user_id).await?;
            let c = bf_db::community::chat::channel(pool, channel).await.db()?.ok_or(AppError::NotFound)?;
            if c.kind == "dm" && !staff && bf_db::community::chat::ignored_in(pool, channel, h.user_id).await.db()? {
                return Ok(false);
            }
            crate::community::chat::deliver_message(state, channel, from, &s("raw"), reply_to).await?;
        }
        "chat_edit" => {
            let m = bf_db::community::chat::message(pool, n("message")?).await.db()?.ok_or(AppError::NotFound)?;
            if m.deleted || m.user_id != Some(h.user_id) {
                return Ok(false);
            }
            let c = bf_db::community::chat::channel(pool, m.channel_id).await.db()?.ok_or(AppError::NotFound)?;
            if c.archived {
                return Err(AppError::Validation("This channel is archived.".into()));
            }
            crate::community::chat::deliver_edit(state, m.id, &s("raw")).await?;
        }
        "status" => {
            bf_db::community::statuses::create(pool, h.user_id, &s("body")).await.db()?;
        }
        "status_reply" => crate::community::statuses::deliver_reply(state, n("status")?, from, &s("body")).await?,
        "profile" => {
            let mut fields = bf_db::accounts::preferences::profile_fields(pool, h.user_id.into()).await.db()?;
            if p.get("about").is_some() {
                fields.about_md = Some(s("about"));
            }
            if p.get("display_name").is_some() {
                fields.display_name = Some(s("display_name"));
            }
            if p.get("location").is_some() {
                fields.location = Some(s("location"));
            }
            if p.get("website").is_some() {
                fields.website = Some(s("website"));
            }
            let html = fields.about_md.as_deref().map(|a| bf_cook::cook(a).html);
            bf_db::accounts::preferences::save_profile(pool, h.user_id.into(), &fields, html.as_deref()).await.db()?;
        }
        "signature" => {
            let raw = s("signature");
            let html = bf_cook::cook(&raw).html;
            bf_db::accounts::preferences::set_signature(pool, h.user_id, &raw, &html).await.db()?;
        }
        "title" => {
            let item = n("item")?;
            // As `change_title` checks: the topic exists, the author still owns it and it is open
            // (a moderator may rename a locked one), and the title still passes the title rules.
            let t = bf_db::forum::topics::topic_for(pool, item, None).await.db()?.ok_or(AppError::NotFound)?;
            let tree = state.forum.get(pool).await?;
            let moderator =
                staff || tree.perms(&crate::http::viewer::actor_for(&author), t.node_id).has(Perms::MODERATE);
            if !moderator && (t.author_id != Some(h.user_id) || t.locked) {
                return Ok(false);
            }
            crate::forum::posting::validate_title(state, &s("title")).map_err(AppError::Validation)?;
            bf_db::moderation::topics::retitle(pool, item, &s("title"), h.user_id).await.db()?;
            state.hub.publish(&format!("item:{item}"), "state", json!({ "action": "retitled" }));
            state.forum.invalidate();
        }
        "club" => {
            let club =
                crate::community::clubs::create_now(state, h.user_id, &s("name"), p["open"].as_bool().unwrap_or(true))
                    .await?;
            if !s("description").trim().is_empty() {
                crate::community::clubs::describe_now(state, club, &s("description")).await?;
            }
        }
        "chat_channel" => {
            bf_db::community::chat::create_channel(pool, &s("name"), &s("description"), "invite", &[], h.user_id)
                .await
                .db()?;
        }
        _ => return Err(AppError::NotFound),
    }
    Ok(true)
}
