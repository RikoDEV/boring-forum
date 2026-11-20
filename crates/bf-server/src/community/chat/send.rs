//! Sending and delivering messages: length and anti-spam checks, then notifying the channel.

use crate::http::render::DbResult;
use crate::http::viewer::{Viewer, is_staff};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use bf_core::AppError;
use bf_db::community::chat;
use serde::Deserialize;
use serde_json::json;

use super::{changed, viewable, writable};

#[derive(Deserialize)]
pub(super) struct SendBody {
    pub(super) raw: String,
    pub(super) reply_to: Option<i64>,
}

pub(super) fn check_length(state: &AppState, raw: &str) -> Result<(), AppError> {
    let n = raw.trim().chars().count() as i64;
    let max = state.settings().int("chat_max_message_length");
    if n == 0 || n > max {
        return Err(AppError::invalid("A message is 1 to {max} characters.", &[("max", &max)]));
    }
    Ok(())
}

/// The anti-spam checks every message passes, as posts do: returns the text to store, with
/// watched words applied. Staff skip the limits meant for newcomers, not the word list.
pub(super) async fn screen(
    state: &AppState,
    v: &Viewer,
    c: &chat::Channel,
    raw: &str,
    new: bool,
) -> Result<(String, bf_cook::Cooked, bool), AppError> {
    let user = v.require_user()?;
    let pool = state.db()?;
    let s = state.settings();
    let staff = is_staff(v);
    crate::moderation::gate::account_ready(state, user).map_err(AppError::Validation)?;
    if !staff {
        if i64::from(user.trust_level) < s.int("chat_min_trust_level") {
            return Err(crate::community::trust::level_refusal(
                state,
                &v.t(),
                user.trust_level,
                s.int("chat_min_trust_level"),
            ));
        }
        let files = crate::forum::uploads::upload_refs(raw);
        if !files.is_empty() {
            if !s.bool("chat_allow_uploads") {
                return Err(AppError::Validation("Files are not allowed in chat.".into()));
            }
            let need = s.int("chat_upload_min_trust_level").max(s.int("upload_min_trust_level"));
            if i64::from(user.trust_level) < need {
                return Err(AppError::invalid("Files in chat can be added from trust level {n}.", &[("n", &need)]));
            }
            let most = s.int("chat_max_files_per_message");
            if files.len() as i64 > most {
                return Err(AppError::invalid("A chat message can carry at most {n} files.", &[("n", &most)]));
            }
            let allowed = crate::forum::uploads::allowed_types(state, true);
            if files.iter().any(|f| !allowed.contains(&f.as_str())) {
                return Err(AppError::invalid(
                    "Chat accepts only these kinds of file: {types}.",
                    &[("types", &if allowed.is_empty() { "none".to_owned() } else { allowed.join(", ") })],
                ));
            }
        }
        if new {
            if c.kind == "dm" && chat::ignored_in(pool, c.id, user.id.get()).await.db()? {
                return Err(AppError::Validation("You cannot message that member.".into()));
            }
            if chat::sent_last_minute(pool, user.id.get()).await.db()? >= s.int("chat_messages_per_minute") {
                return Err(AppError::Validation("You are sending messages too quickly.".into()));
            }
            let wait = s.int("chat_wait_seconds");
            if wait > 0
                && let Some(since) = chat::seconds_since_last(pool, user.id.get()).await.db()?
                && since < wait
            {
                return Err(AppError::invalid("Please wait {n} more seconds.", &[("n", &(wait - since))]));
            }
            let window = s.int("chat_duplicate_seconds") as i32;
            if window > 0 && chat::is_duplicate(pool, c.id, user.id.get(), raw, window).await.db()? {
                return Err(AppError::Validation("You just sent that message.".into()));
            }
        }
    }
    // Plugins' text filters (`post.before_create`) apply to chat too; a refusal is the sender's error.
    let filtered = crate::platform::plugins::before_post(state, user.id.get(), raw).await?;
    let raw = filtered.as_str();
    let tree = state.forum.get(pool).await?;
    let (text, held) = match bf_core::watched::apply(&tree.words, raw) {
        bf_core::watched::Outcome::Blocked(word) => {
            return Err(AppError::invalid("Your message contains a blocked word: {word}", &[("word", &word)]));
        }
        // "Require approval": the message waits in the held queue (`moderation::held`).
        bf_core::watched::Outcome::Allow { text, needs_approval, .. } => (text, needs_approval && !staff),
    };
    let cook_from = text.clone();
    let cooked = tokio::task::spawn_blocking(move || bf_cook::cook(&cook_from)).await.map_err(AppError::internal)?;
    if !staff
        && i64::from(user.post_count) < s.int("new_user_posts")
        && cooked.links.len() as i64 > s.int("new_user_max_links")
    {
        return Err(AppError::invalid(
            "New members can put at most {n} links in a message.",
            &[("n", &s.int("new_user_max_links"))],
        ));
    }
    if new && !staff && crate::platform::builtin_plugins::looks_like_spam(state, user, &text).await {
        return Err(AppError::Validation("This looks like spam, so it was not sent.".into()));
    }
    Ok((text, cooked, held))
}

pub(super) async fn send(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(b): Json<SendBody>,
) -> Result<Response, AppError> {
    v.check_csrf_headers(&headers)?;
    let c = viewable(&state, &v, id).await?;
    writable(&v, &c)?;
    check_length(&state, &b.raw)?;
    let user = v.require_user()?;
    let pool = state.db()?;
    let (raw, _, held) = screen(&state, &v, &c, b.raw.trim(), true).await?;
    if held {
        // Joining still happens, as it would have with the message.
        chat::join(pool, id, user.id.get()).await.db()?;
        let payload = json!({ "channel": id, "raw": raw, "reply_to": b.reply_to });
        crate::moderation::held::hold(&state, user.id.get(), "chat", None, payload).await?;
        return Ok(crate::moderation::held::held_json());
    }
    let m = deliver_message(&state, id, (user.id.get(), &user.username), &raw, b.reply_to).await?;
    Ok((StatusCode::CREATED, Json(m)).into_response())
}

/// Whether `user` may still post in channel `id` now (a held message waited for a moderator):
/// the channel exists and is not archived, the member is neither suspended nor silenced, and
/// still belongs to it (invite-only channels and direct messages) or to an allowed group.
pub(crate) async fn check_can_deliver(state: &AppState, id: i64, user: i64) -> Result<(), AppError> {
    let pool = state.db()?;
    let c = chat::channel(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    let u = bf_db::accounts::users::user_by_id(pool, user.into()).await.db()?.ok_or(AppError::Forbidden)?;
    if c.archived {
        return Err(AppError::Validation("This channel is archived.".into()));
    }
    if u.silenced || !u.approved || bf_db::accounts::users::is_suspended(pool, user).await.db()? {
        return Err(AppError::Forbidden);
    }
    let allowed = match (c.kind.as_str(), c.access.as_str()) {
        ("dm", _) | (_, "invite") => chat::is_member(pool, id, user).await.db()?,
        (_, "groups") => {
            u.is_admin || chat::group_allowed(pool, id, &bf_core::ids::raw(&u.groups)).await.db()?
        }
        _ => true,
    };
    if allowed { Ok(()) } else { Err(AppError::Forbidden) }
}

/// Sends a message and tells the channel. Also used when a held message is approved.
pub(crate) async fn deliver_message(
    state: &AppState,
    id: i64,
    from: (i64, &str),
    raw: &str,
    reply_to: Option<i64>,
) -> Result<chat::Message, AppError> {
    let pool = state.db()?;
    let c = chat::channel(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    let from_raw = raw.to_owned();
    let cooked = tokio::task::spawn_blocking(move || bf_cook::cook(&from_raw)).await.map_err(AppError::internal)?;
    let msg_id = chat::send(pool, id, from.0, raw, &cooked.html, reply_to).await.db()?;
    let m = chat::message(pool, msg_id).await.db()?.ok_or(AppError::NotFound)?;
    state.hub.publish(&format!("chat:{id}"), "message", json!(m));
    let listeners = chat::listeners(pool, id).await.db()?;
    for l in listeners.iter().filter(|l| **l != from.0) {
        state.hub.publish(&format!("user:{l}"), "chat", json!({ "channel": id }));
    }
    notify(state, &c, from.0, from.1, &cooked.mentions, &listeners).await;
    crate::platform::webhooks::fire(
        state,
        "chat.message",
        json!({ "channel": id, "user_id": from.0, "message_id": msg_id, "kind": c.kind }),
    );
    Ok(m)
}

/// Edits a message. Also used when a held edit is approved.
pub(crate) async fn deliver_edit(state: &AppState, id: i64, raw: &str) -> Result<(), AppError> {
    let html = bf_cook::cook(raw).html;
    chat::edit(state.db()?, id, raw, &html).await.db()?;
    changed(state, id).await.map(|_| ())
}

/// Mentions reach anyone who can see the channel; a direct message reaches its other people,
/// folded per conversation until read.
pub(super) async fn notify(
    state: &AppState,
    c: &chat::Channel,
    from: i64,
    by: &str,
    mentions: &[String],
    listeners: &[i64],
) {
    let Some(pool) = &state.pool else { return };
    let url = format!("/chat/{}", c.id);
    let title = if c.kind == "dm" { "direct message".to_owned() } else { c.name.clone() };
    let max = state.settings().int("chat_max_mentions") as usize;
    let names = &mentions[..mentions.len().min(max)];
    let mut mentioned = if names.is_empty() {
        vec![]
    } else {
        bf_core::ids::raw(&bf_db::accounts::users::user_ids_by_names(pool, names).await.unwrap_or_default())
    };
    if c.kind == "dm" {
        mentioned.clear();
    } else if !mentioned.is_empty() {
        mentioned = chat::can_see(pool, c.id, &mentioned).await.unwrap_or_default();
    }
    let data = json!({ "by": by, "title": title, "url": url });
    crate::community::notifications::send(state, from, &mentioned, "chat_mention", &data).await;
    if c.kind == "dm" {
        let data = json!({ "by": by, "title": title, "url": url, "group": format!("chat:{}", c.id), "count": 1 });
        crate::community::notifications::send(state, from, listeners, "chat_message", &data).await;
    }
}
