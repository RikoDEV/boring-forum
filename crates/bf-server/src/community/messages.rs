//! Private messages: conversations with several people, read receipts, archive, search.
//! A conversation someone is not part of answers 404, so its existence is not revealed.

use crate::http::i18n::{tr, trh};
use crate::http::render::{DbResult, ago, layout, page};
use crate::http::ui::hidden_csrf;
use crate::http::ui::simple_page;
use crate::http::ui::{Shell, begin, finish, panel};
use crate::http::viewer::{CsrfForm, Viewer, is_staff};
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use bf_core::AppError;
use bf_core::html::esc;
use bf_db::admin::audit;
use bf_db::community::messages;
use bf_web::pages::{ConversationPage, InboxLine, InboxPage, MessageView, ParticipantView, render};
use serde::Deserialize;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/me/messages", get(recent))
        .route("/messages", get(inbox).post(start))
        .route("/messages/{id}", get(conversation))
        .route("/messages/{id}/reply", post(reply))
        .route("/messages/{id}/add", post(add))
        .route("/messages/{id}/remove", post(remove))
        .route("/messages/{id}/leave", post(leave))
        .route("/messages/{id}/archive", post(archive))
        .route("/messages/{id}/unread", post(unread))
        .route("/messages/{id}/folder", post(set_folder))
        .route("/messages/{id}/typing", post(typing))
        .route("/messages/{id}/report", post(report_conversation))
        .route("/review/messages", get(message_reports))
        .route("/review/messages/{id}", get(read_reported).post(resolve_reported))
}

const MAX_BODY: usize = 20_000;

const MAX_SUBJECT: usize = 150;

#[derive(Deserialize, Default)]
struct InboxQuery {
    #[serde(default)]
    folder: String,
    #[serde(default)]
    q: String,
    #[serde(default)]
    to: String,
}

fn first_name(others: &str) -> String {
    others.split(", ").next().unwrap_or_default().to_owned()
}

/// One line of plain text for lists, from the rendered message so no Markdown shows.
fn preview_of(cooked: Option<&str>) -> String {
    bf_seo::excerpt(cooked.unwrap_or_default(), 160)
}

/// The newest conversations, for the header dropdown.
#[utoipa::path(get, path = "/api/v1/me/messages", tag = "me",
    responses((status = 200, body = Object, description = "the newest conversations"), (status = 403, body = bf_core::error::Problem)))]
pub(crate) async fn recent(State(state): State<AppState>, v: Viewer) -> Result<Response, AppError> {
    let user = v.require_user()?;
    let rows = messages::inbox(state.db()?, user.id.get(), false, "", "").await.db()?;
    let items: Vec<_> = rows
        .iter()
        .take(10)
        .map(|r| {
            serde_json::json!({
                "url": format!("/messages/{}", r.id),
                "title": r.subject,
                "text": preview_of(r.preview.as_deref()),
                "when": ago(r.last_message_at),
                "when_at": r.last_message_at.to_rfc3339(),
                "read": !r.unread,
                "avatar": first_name(&r.others),
            })
        })
        .collect();
    Ok(axum::Json(items).into_response())
}

async fn inbox(State(state): State<AppState>, v: Viewer, Query(q): Query<InboxQuery>) -> Result<Response, AppError> {
    let user = v.require_user()?;
    let archived = q.folder == "archive";
    let custom = if archived { "" } else { q.folder.as_str() };
    let term = q.q.trim();
    let term = if term.is_empty() {
        String::new()
    } else {
        crate::platform::plugins::filter_text(&state, "search.query", "q", term).await?
    };
    let rows = messages::inbox(state.db()?, user.id.get(), archived, &term, custom).await.db()?;
    let folders = bf_db::community::messages::folders(state.db()?, user.id.get()).await.db()?;
    let (mut l, set) = layout(&state, &v, Some(&v.t().msg("Messages")));
    l.noindex = true;
    let new_blocked = crate::community::trust::needs_text(&state, &v.t(), &v, "pm_min_trust_level");
    let body = InboxPage {
        new_blocked,
        layout: l,
        archived,
        folders,
        current_folder: custom.to_owned(),
        query: q.q,
        to: q.to,
        lines: rows
            .into_iter()
            .map(|r| InboxLine {
                people: r
                    .people
                    .iter()
                    .filter_map(|p| p.split_once('|'))
                    .filter_map(|(id, name)| {
                        Some((bf_core::slug::profile_path(id.parse().ok()?, name), name.to_owned()))
                    })
                    .collect(),
                preview: preview_of(r.preview.as_deref()),
                avatar: first_name(&r.others),
                count: r.message_count,
                url: format!("/messages/{}", r.id),
                subject: r.subject,
                others: r.others,
                when: ago(r.last_message_at),
                unread: r.unread,
            })
            .collect(),
    };
    Ok(page(render(&body), set, false))
}

fn check_body(body: &str) -> Result<(), AppError> {
    if body.trim().is_empty() || body.chars().count() > MAX_BODY {
        return Err(AppError::invalid("A message must be 1 to {MAX_BODY} characters.", &[("MAX_BODY", &MAX_BODY)]));
    }
    Ok(())
}

/// Silenced members cannot message anyone.
pub(crate) fn check_can_send(v: &Viewer) -> Result<(), AppError> {
    if v.actor.silenced {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

/// Turns names into people who can be messaged. The same answer covers unknown names and people
/// ignoring the sender, so this cannot be used to find out who ignores you.
async fn recipients(state: &AppState, sender: i64, names: &str) -> Result<Vec<i64>, AppError> {
    let pool = state.db()?;
    let (groups, lower): (Vec<String>, Vec<String>) =
        names.split(',').map(|n| n.trim().to_lowercase()).filter(|n| !n.is_empty()).partition(|n| n.starts_with('@'));
    let refused = || AppError::Validation("One of those members cannot be messaged.".into());
    // `name@server`: someone on another server (direct messages), found by WebFinger
    // the first time and messaged through their shadow account here.
    let remote_name = |n: &str| n.split_once('@').is_some_and(|(u, h)| !u.is_empty() && h.contains('.'));
    let (remote, local): (Vec<String>, Vec<String>) = lower.into_iter().partition(|n| remote_name(n));
    let mut ids = bf_core::ids::raw(&bf_db::accounts::users::user_ids_by_names(pool, &local).await.db()?);
    if ids.len() != local.len() {
        return Err(refused());
    }
    for name in &remote {
        if !crate::federation::enabled(state) || !state.settings().bool("federation_dms") {
            return Err(refused());
        }
        let known = bf_db::accounts::users::user_ids_by_names(pool, std::slice::from_ref(name)).await.db()?;
        let id = if let Some(id) = known.first() {
            id.get()
        } else {
            let actor = crate::federation::extended::resolve(state, name)
                .await
                .map_err(|e| AppError::invalid("Could not find {name}: {e}", &[("name", &name), ("e", &e)]))?;
            bf_db::federation::ensure_shadow_user(pool, &actor).await.db()?
        };
        ids.push(id);
    }
    // `@group` is a shared inbox: its members all join; `@staff` is the moderators' inbox.
    let cap = state.settings().int("pm_max_participants");
    for g in &groups {
        let members = bf_db::community::groups::group_members(pool, &g[1..], cap).await.db()?;
        if members.is_empty() {
            return Err(refused());
        }
        // Anyone can reach staff. A group's inbox is for its own members and administrators,
        // or a single message could reach every member of a large group.
        let admin = bf_db::accounts::users::user_by_id(pool, sender.into()).await.db()?.is_some_and(|u| u.is_admin);
        // Everyone in the group counts here, not just those within the participant cap.
        let is_member = bf_db::community::groups::group_members(pool, &g[1..], i64::MAX).await.db()?.contains(&sender);
        if g != "@staff" && !admin && !is_member {
            return Err(refused());
        }
        ids.extend(members);
    }
    ids.sort_unstable();
    ids.dedup();
    ids.retain(|i| *i != sender);
    // "Anyone can reach staff": one moderator's ignore list only drops that moderator.
    if groups.iter().any(|g| g == "@staff") {
        let ignoring = messages::ignoring(pool, sender, &ids).await.db()?;
        ids.retain(|i| !ignoring.contains(i));
    }
    check_recipients(state, sender, &ids).await?;
    Ok(ids)
}

/// The recipients' side of a message from `sender`: refused when one of `to` takes messages from
/// nobody or only from people they follow (staff excepted), or ignores the sender. The same
/// answer as for an unknown name, so this tells nothing about who set what. Also the check for a
/// message that arrives from another server.
pub(crate) async fn check_recipients(state: &AppState, sender: i64, to: &[i64]) -> Result<(), AppError> {
    let pool = state.db()?;
    if !messages::restricting(pool, sender, to).await.db()?.is_empty()
        || !messages::ignoring(pool, sender, to).await.db()?.is_empty()
    {
        return Err(AppError::Validation("One of those members cannot be messaged.".into()));
    }
    Ok(())
}

/// What a member may still do in private messages this hour.
struct SenderLimits {
    /// Conversations started and people added to them, per hour.
    per_hour: i64,
    /// People in one conversation, the sender included.
    max_people: i64,
    /// How much of `per_hour` is used.
    used: i64,
}

/// People each member added to conversations in the last hour. They count toward `pm_per_hour`
/// like conversations started, so adding and removing people is no way around it. Kept in
/// memory, per server.
static ADDED: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<i64, Vec<std::time::Instant>>>> =
    std::sync::LazyLock::new(Default::default);

/// Records `adding` more people for this member and returns their count for the last hour.
fn added_last_hour(user: i64, adding: usize) -> i64 {
    let hour = std::time::Duration::from_secs(3600);
    let now = std::time::Instant::now();
    let mut map = ADDED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if map.len() > 10_000 {
        map.retain(|_, times| times.iter().any(|t| now.duration_since(*t) < hour));
    }
    let times = map.entry(user).or_default();
    times.retain(|t| now.duration_since(*t) < hour);
    times.extend(std::iter::repeat_n(now, adding));
    let n = times.len() as i64;
    if n == 0 {
        map.remove(&user);
    }
    n
}

/// The sender's side of starting a conversation with `names` or adding them to one: enough
/// standing to write to them, and the limits that apply. A group's limit, where every group of
/// the member has one, replaces the site's.
async fn check_sender(state: &AppState, v: &Viewer, names: &str) -> Result<SenderLimits, AppError> {
    let user = v.require_user()?;
    // Unverified, unapproved, terms not accepted, 2FA not set up: no writing to members either.
    crate::moderation::gate::account_ready(state, user).map_err(AppError::Validation)?;
    let pool = state.db()?;
    let s = state.settings();
    // Writing to the moderators' inbox is always open; other members need some standing first.
    let only_staff =
        names.split(',').map(str::trim).filter(|n| !n.is_empty()).all(|n| n.eq_ignore_ascii_case("@staff"));
    if !is_staff(v) && !only_staff && i64::from(user.trust_level) < s.int("pm_min_trust_level") {
        return Err(crate::community::trust::level_refusal(
            state,
            &v.t(),
            user.trust_level,
            s.int("pm_min_trust_level"),
        ));
    }
    let limits = bf_db::accounts::limits::limits(pool, &user.groups).await.db()?;
    Ok(SenderLimits {
        per_hour: limits.pm_per_hour.unwrap_or_else(|| s.int("pm_per_hour")),
        max_people: limits.pm_max_participants.unwrap_or_else(|| s.int("pm_max_participants")),
        used: messages::started_last_hour(pool, user.id.get()).await.db()? + added_last_hour(user.id.get(), 0),
    })
}

/// Tells the other people in a conversation, in each one's language by email and in the app.
async fn tell(state: &AppState, from: (i64, &str), id: i64, subject: &str, to: &[i64]) {
    // The text stays out of email, which is not private; the link needs a sign-in. The
    // conversation id lets the email be answered by replying to it.
    let data = json!({ "by": from.1, "title": subject, "url": format!("/messages/{id}"), "conversation_id": id });
    crate::community::notifications::send(state, from.0, to, "message", &data).await;
}

#[derive(Deserialize)]
struct StartForm {
    csrf: String,
    to: String,
    subject: String,
    body: String,
}

async fn start(State(state): State<AppState>, v: Viewer, Form(f): Form<StartForm>) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    check_can_send(&v)?;
    check_body(&f.body)?;
    let subject = f.subject.trim();
    if subject.is_empty() || subject.chars().count() > MAX_SUBJECT {
        return Err(AppError::invalid(
            "A subject must be 1 to {MAX_SUBJECT} characters.",
            &[("MAX_SUBJECT", &MAX_SUBJECT)],
        ));
    }
    let limits = check_sender(&state, &v, &f.to).await?;
    if !user.is_admin && limits.used >= limits.per_hour {
        return Err(AppError::Validation("You have started too many conversations. Try again later.".into()));
    }
    let to = recipients(&state, user.id.get(), &f.to).await?;
    if to.is_empty() {
        return Err(AppError::Validation("Name at least one other member.".into()));
    }
    if !user.is_admin && to.len() as i64 + 1 > limits.max_people {
        return Err(AppError::Validation("That is too many people for one conversation.".into()));
    }
    let subject = crate::moderation::gate::words(&state, &v, subject).await?;
    let (body, _) = crate::moderation::gate::check_post(&state, &v, &f.body).await?;
    if subject.held || body.held {
        let payload = json!({ "subject": subject.text, "body": body.text, "to": to });
        crate::moderation::held::hold(&state, user.id.get(), "conversation", None, payload).await?;
        return Ok(crate::moderation::held::held_page(&state, &v));
    }
    let cap = (!user.is_admin).then_some(limits.per_hour);
    let id = deliver_new_capped(&state, (user.id.get(), &user.username), &subject.text, &body.text, &to, cap).await?;
    Ok(Redirect::to(&format!("/messages/{id}")).into_response())
}

/// Starts a conversation and tells its people. Also used when a held one is approved.
pub(crate) async fn deliver_new(
    state: &AppState,
    from: (i64, &str),
    subject: &str,
    body: &str,
    to: &[i64],
) -> Result<i64, AppError> {
    deliver_new_capped(state, from, subject, body, to, None).await
}

/// [`deliver_new`] for a sender held to `per_hour` new conversations. The limit is checked again
/// where the conversation is written, under a per-member lock, so parallel requests cannot pass it.
async fn deliver_new_capped(
    state: &AppState,
    from: (i64, &str),
    subject: &str,
    body: &str,
    to: &[i64],
    per_hour: Option<i64>,
) -> Result<i64, AppError> {
    let cooked = bf_cook::cook(body);
    let cap = per_hour.map(|n| (n, added_last_hour(from.0, 0)));
    let Some(id) = messages::create_capped(state.db()?, from.0, subject, body, &cooked.html, to, cap).await.db()?
    else {
        return Err(AppError::Validation("You have started too many conversations. Try again later.".into()));
    };
    tell(state, from, id, subject, to).await;
    crate::federation::extended::outbound_dm(state, id, from.0).await;
    crate::platform::webhooks::fire(
        state,
        "message.created",
        json!({ "from_id": from.0, "to_ids": to, "conversation_id": id, "new": true }),
    );
    Ok(id)
}

async fn member_head(state: &AppState, id: i64, user: i64) -> Result<messages::Head, AppError> {
    messages::participant_of(state.db()?, id, user).await.db()?.ok_or(AppError::NotFound)
}

async fn conversation(State(state): State<AppState>, v: Viewer, Path(id): Path<i64>) -> Result<Response, AppError> {
    let user = v.require_user()?;
    let head = member_head(&state, id, user.id.get()).await?;
    let pool = state.db()?;
    let messages = messages::messages(pool, id).await.db()?;
    let people = messages::participants(pool, id).await.db()?;
    messages::mark_read(pool, id, user.id.get()).await.db()?;
    let last = messages.last().map_or(0, |m| m.id);
    let (mut l, set) = layout(&state, &v, Some(&head.subject));
    l.noindex = true;
    let body = ConversationPage {
        layout: l,
        id,
        subject: head.subject,
        archived: head.archived,
        can_manage: head.starter_id == Some(user.id.get()),
        participants: people
            .into_iter()
            .map(|p| ParticipantView {
                url: bf_core::slug::profile_path(p.user_id, &p.username),
                is_self: p.user_id == user.id,
                seen_latest: p.last_read_id >= last,
                username: p.username,
            })
            .collect(),
        messages: messages
            .into_iter()
            .map(|m| MessageView {
                author: m.author.unwrap_or_else(|| "deleted member".into()),
                when: ago(m.created_at),
                cooked: m.cooked,
                new: m.id > head.last_read_id,
            })
            .collect(),
    };
    Ok(page(render(&body), set, false))
}

#[derive(Deserialize)]
struct ReplyForm {
    csrf: String,
    body: String,
}

async fn reply(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<ReplyForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    check_can_send(&v)?;
    check_body(&f.body)?;
    member_head(&state, id, user.id.get()).await?;
    // Starting conversations has `pm_per_hour`; replies wait like posts do.
    let flood = state.settings().int("flood_wait_seconds");
    if flood > 0
        && !is_staff(&v)
        && let Some(since) = messages::seconds_since_last_message(state.db()?, user.id.get()).await.db()?
        && since < flood
    {
        return Err(AppError::invalid("Please wait {n} more seconds.", &[("n", &(flood - since))]));
    }
    let (body, _) = crate::moderation::gate::check_post(&state, &v, &f.body).await?;
    if body.held {
        crate::moderation::held::hold(
            &state,
            user.id.get(),
            "message",
            None,
            json!({ "conversation": id, "body": body.text }),
        )
        .await?;
        return Ok(crate::moderation::held::held_page(&state, &v));
    }
    deliver_reply(&state, id, (user.id.get(), &user.username), &body.text).await?;
    Ok(Redirect::to(&format!("/messages/{id}")).into_response())
}

/// Adds a reply from someone still in the conversation and tells the others. Also used when a
/// held reply is approved.
pub(crate) async fn deliver_reply(state: &AppState, id: i64, from: (i64, &str), body: &str) -> Result<(), AppError> {
    let head = member_head(state, id, from.0).await?;
    let pool = state.db()?;
    let cooked = bf_cook::cook(body);
    messages::reply(pool, id, from.0, body, &cooked.html).await.db()?;
    // Anyone with the conversation open sees the reply without reloading.
    state.hub.publish(&format!("conversation:{id}"), "message", json!({ "by": from.1 }));
    let others: Vec<i64> =
        messages::participants(pool, id).await.db()?.into_iter().map(|p| p.user_id).filter(|u| *u != from.0).collect();
    tell(state, from, id, &head.subject, &others).await;
    crate::federation::extended::outbound_dm(state, id, from.0).await;
    crate::platform::webhooks::fire(
        state,
        "message.created",
        json!({ "from_id": from.0, "to_ids": others, "conversation_id": id, "new": false }),
    );
    Ok(())
}

#[derive(Deserialize)]
struct PersonForm {
    csrf: String,
    username: String,
}

/// Only the starter adds or removes people.
async fn managed_head(state: &AppState, v: &Viewer, id: i64, csrf: &str) -> Result<messages::Head, AppError> {
    v.check_csrf(csrf)?;
    let user = v.require_user()?;
    let head = member_head(state, id, user.id.get()).await?;
    if head.starter_id != Some(user.id.get()) {
        return Err(AppError::Forbidden);
    }
    Ok(head)
}

async fn add(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<PersonForm>,
) -> Result<Response, AppError> {
    let head = managed_head(&state, &v, id, &f.csrf).await?;
    check_can_send(&v)?;
    let user = v.require_user()?;
    // The same standing and limits as starting a conversation with these people.
    let limits = check_sender(&state, &v, &f.username).await?;
    let to = recipients(&state, user.id.get(), &f.username).await?;
    let pool = state.db()?;
    let count = messages::participants(pool, id).await.db()?.len();
    if !user.is_admin && (count + to.len()) as i64 > limits.max_people {
        return Err(AppError::Validation("That is too many people for one conversation.".into()));
    }
    if !user.is_admin && limits.used + to.len() as i64 > limits.per_hour {
        return Err(AppError::Validation("You have added too many people to conversations. Try again later.".into()));
    }
    for person in &to {
        messages::add_participant(pool, id, *person).await.db()?;
    }
    added_last_hour(user.id.get(), to.len());
    tell(&state, (user.id.get(), &user.username), id, &head.subject, &to).await;
    Ok(Redirect::to(&format!("/messages/{id}")).into_response())
}

async fn remove(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<PersonForm>,
) -> Result<Response, AppError> {
    managed_head(&state, &v, id, &f.csrf).await?;
    let pool = state.db()?;
    let ids = bf_db::accounts::users::user_ids_by_names(pool, &[f.username.trim().to_lowercase()]).await.db()?;
    for person in ids.into_iter().filter(|p| Some(*p) != v.user.as_ref().map(|u| u.id)) {
        messages::remove_participant(pool, id, person.get()).await.db()?;
    }
    Ok(Redirect::to(&format!("/messages/{id}")).into_response())
}

async fn leave(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    member_head(&state, id, user.id.get()).await?;
    messages::leave(state.db()?, id, user.id.get()).await.db()?;
    Ok(Redirect::to("/messages").into_response())
}

#[derive(Deserialize)]
struct ArchiveForm {
    csrf: String,
    #[serde(default)]
    archived: String,
}

async fn archive(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<ArchiveForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    member_head(&state, id, user.id.get()).await?;
    messages::set_archived(state.db()?, id, user.id.get(), f.archived == "1").await.db()?;
    Ok(Redirect::to("/messages").into_response())
}

async fn unread(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    member_head(&state, id, user.id.get()).await?;
    messages::mark_unread(state.db()?, id, user.id.get()).await.db()?;
    Ok(Redirect::to("/messages").into_response())
}

#[derive(Deserialize)]
struct FolderForm {
    csrf: String,
    #[serde(default)]
    folder: String,
}

async fn set_folder(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<FolderForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let uid = v.require_user()?.id.get();
    member_head(&state, id, uid).await?;
    let folder = f.folder.trim();
    if folder.chars().count() > 40 || folder.chars().any(char::is_control) || folder == "archive" {
        return Err(AppError::Validation("Choose a folder name of up to 40 characters.".into()));
    }
    messages::set_folder(state.db()?, id, uid, folder).await.db()?;
    Ok(Redirect::to(&format!("/messages/{id}")).into_response())
}

async fn typing(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    member_head(&state, id, user.id.get()).await?;
    state.hub.publish(&format!("conversation:{id}"), "typing", json!({ "by": user.username }));
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize)]
struct ReasonForm {
    csrf: String,
    reason: String,
}

/// A participant's report is the only way staff can ever open a conversation.
async fn report_conversation(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<ReasonForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let reporter = v.require_user()?.id.get();
    crate::http::ratelimit::REPORTS_BY_USER.enforce(&format!("report:{reporter}"))?;
    member_head(&state, id, reporter).await?;
    let reason = f.reason.trim();
    if reason.is_empty() || reason.chars().count() > 1000 {
        return Err(AppError::Validation("Say what is wrong, in up to 1000 characters.".into()));
    }
    messages::report_conversation(state.db()?, id, reporter, reason).await.db()?;
    crate::community::notifications::tell_staff(&state, reporter).await;
    Ok(Redirect::to(&format!("/messages/{id}")).into_response())
}

fn staff_only(v: &Viewer) -> Result<i64, AppError> {
    let user = v.require_user()?;
    if is_staff(v) { Ok(user.id.get()) } else { Err(AppError::Forbidden) }
}

async fn message_reports(State(state): State<AppState>, v: Viewer) -> Result<Response, AppError> {
    staff_only(&v)?;
    let rows = messages::open_conversation_reports(state.db()?).await.db()?;
    let items: String = rows
        .iter()
        .map(|r| {
            format!(
                "<li><a href=\"/review/messages/{}\">{}</a> <span class=\"muted small\">{}</span><p class=\"small\">{}</p></li>",
                r.id,
                trh("Report by {name}").replace("{name}", &esc(&r.reporter.clone().unwrap_or_else(|| tr("deleted member")))),
                ago(r.created_at),
                esc(&r.reason)
            )
        })
        .collect();
    Ok(simple_page(&state, &v, &v.t().msg("Reported messages"), &format!("<ul class=\"activity\">{items}</ul>"), false))
}

/// Opening one is written to the staff log, and works only while the report is open.
async fn read_reported(State(state): State<AppState>, v: Viewer, Path(id): Path<i64>) -> Result<Response, AppError> {
    let who = staff_only(&v)?;
    let pool = state.db()?;
    let r = messages::conversation_report(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    audit::audit(pool, who, "conversation.read_reported", &r.conversation_id.to_string()).await.db()?;
    let messages = bf_db::community::messages::messages(pool, r.conversation_id).await.db()?;
    let ctx = begin(&state, &v);
    let mut html = format!("<p class=\"small\">{}</p>", trh("Reason: {reason}").replace("{reason}", &esc(&r.reason)));
    for m in &messages {
        html.push_str(&format!(
            "<article class=\"panel post\"><p class=\"small\"><strong>{}</strong> {}</p><div class=\"cooked\">{}</div></article>",
            esc(m.author.as_deref().unwrap_or("deleted member")),
            ago(m.created_at),
            m.cooked
        ));
    }
    html.push_str(&format!(
        "<form method=\"post\" action=\"/review/messages/{id}\">{}<button class=\"btn\">{tr1}</button></form>",
        hidden_csrf(&ctx.token),
        tr1 = trh("Close report")
    ));
    let shell = Shell { title: v.t().msg("Reported conversation"), noindex: true, ..Shell::default() };
    Ok(finish(&state, &v, ctx, shell, panel(&html)))
}

async fn resolve_reported(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let who = staff_only(&v)?;
    if !messages::resolve_conversation_report(state.db()?, id, who).await.db()? {
        return Err(AppError::NotFound);
    }
    Ok(Redirect::to("/review/messages").into_response())
}
