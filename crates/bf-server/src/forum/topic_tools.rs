//! Topic timers, slow mode, wiki posts, saved replies and reply by email.

use crate::forum::topics::readable_topic;
use crate::http::i18n::trh;
use crate::http::render::DbResult;
use crate::http::viewer::{Viewer, is_staff};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post, put};
use axum::{Form, Json, Router};
use bf_core::AppError;
use bf_core::perms::Perms;
use bf_core::slug::topic_path;
use bf_db::forum::tools;
use serde::Deserialize;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/t/{id}/timer", post(set_timer))
        .route("/t/{id}/slow", post(set_slow))
        .route("/post/{id}/wiki", post(toggle_wiki))
        .route("/post/{id}/note", post(add_note))
        .route("/post/{id}/note/{note}/delete", post(delete_note))
        .route("/api/v1/saved-replies", get(list_saved).post(create_saved))
        .route("/api/v1/saved-replies/{id}", put(update_saved).delete(delete_saved))
        .route("/api/v1/webhooks/inbound-mail", post(inbound_mail))
        .route("/settings/mail-key", post(new_mail_key))
}

async fn moderated_topic(state: &AppState, v: &Viewer, id: i64) -> Result<bf_db::forum::topics::TopicRow, AppError> {
    let (t, _, perms) = readable_topic(state, v, id).await?;
    if !perms.has(Perms::MODERATE) {
        return Err(AppError::Forbidden);
    }
    Ok(t)
}

#[derive(Deserialize)]
struct TimerForm {
    csrf: String,
    action: String,
    #[serde(default)]
    hours: i32,
    #[serde(default)]
    minutes: i32,
}

/// `action` "none" clears the timer.
async fn set_timer(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<TimerForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let t = moderated_topic(&state, &v, id).await?;
    let pool = state.db()?;
    let total = i64::from(f.hours) * 60 + i64::from(f.minutes);
    match f.action.as_str() {
        "none" => tools::clear_timer(pool, id).await.db()?,
        "close" | "open" | "bump" | "delete" if (1..=5_256_000).contains(&total) => {
            let by = v.require_user()?.id;
            tools::set_timer(pool, id, &f.action, total as i32, by.get()).await.db()?;
        }
        _ => return Err(AppError::Validation("Choose an action and a time of 1 minute to 10 years.".into())),
    }
    Ok(Redirect::to(&topic_path(t.id, &t.slug)).into_response())
}

/// Runs from the maintenance tick.
pub(crate) async fn run_timers(state: &AppState) {
    let Some(pool) = &state.pool else { return };
    match tools::close_inactive(pool, state.settings().int("auto_close_topics_post_count")).await {
        Ok(closed) if !closed.is_empty() => {
            for item in &closed {
                state.hub.publish(&format!("item:{item}"), "state", serde_json::json!({ "action": "closed" }));
            }
            state.forum.invalidate();
            state.microcache.clear();
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "closing inactive topics failed"),
    }
    match tools::run_due_timers(pool).await {
        Ok((0, _)) => {}
        Ok((_, published)) => {
            state.forum.invalidate();
            state.microcache.clear();
            for item in published {
                crate::forum::posting::announce_post(state, item, 1).await;
            }
        }
        Err(e) => tracing::warn!(error = %e, "topic timers failed"),
    }
}

#[derive(Deserialize)]
struct SlowForm {
    csrf: String,
    seconds: i32,
}

async fn set_slow(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<SlowForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let t = moderated_topic(&state, &v, id).await?;
    if !(0..=86_400).contains(&f.seconds) {
        return Err(AppError::Validation("Slow mode is 0 seconds to one day.".into()));
    }
    tools::set_slow_mode(state.db()?, id, f.seconds).await.db()?;
    state.microcache.clear();
    Ok(Redirect::to(&topic_path(t.id, &t.slug)).into_response())
}

/// Slow mode, checked before a reply. Moderators are not limited.
pub(crate) async fn check_slow_mode(state: &AppState, topic: i64, user: i64, moderator: bool) -> Result<(), AppError> {
    if moderator {
        return Ok(());
    }
    let wait = tools::slow_mode_wait(state.db()?, topic, user).await.db()?;
    match wait {
        Some(s) if s > 0 => {
            Err(AppError::invalid("This topic is in slow mode. Try again in {s} seconds.", &[("s", &s)]))
        }
        _ => Ok(()),
    }
}

async fn toggle_wiki(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<crate::http::viewer::CsrfForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let pool = state.db()?;
    let p = bf_db::forum::posts::post_ref(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    let t = moderated_topic(&state, &v, p.item_id).await?;
    tools::toggle_wiki(pool, id).await.db()?;
    state.microcache.clear();
    Ok(Redirect::to(&format!("{}#post-{}", topic_path(t.id, &t.slug), p.post_number)).into_response())
}

#[derive(Deserialize)]
struct NoteForm {
    csrf: String,
    body: String,
}

/// A note on a post, for staff only. It is kept apart from the post, so its author cannot see,
/// edit or remove it; only staff who moderate the forum can.
async fn add_note(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<NoteForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let pool = state.db()?;
    let p = bf_db::forum::posts::post_ref(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    let t = moderated_topic(&state, &v, p.item_id).await?;
    let body = f.body.trim();
    if body.is_empty() || body.chars().count() > 2000 {
        return Err(AppError::Validation("A staff note needs some text, up to 2000 characters.".into()));
    }
    let me = v.require_user()?.id.get();
    bf_db::moderation::topics::add_post_note(pool, id, me, body).await.db()?;
    Ok(Redirect::to(&format!("{}#post-{}", topic_path(t.id, &t.slug), p.post_number)).into_response())
}

async fn delete_note(
    State(state): State<AppState>,
    v: Viewer,
    Path((id, note)): Path<(i64, i64)>,
    Form(f): Form<crate::http::viewer::CsrfForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let pool = state.db()?;
    let p = bf_db::forum::posts::post_ref(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    let t = moderated_topic(&state, &v, p.item_id).await?;
    let me = v.require_user()?.id.get();
    if !bf_db::moderation::topics::delete_post_note(pool, id, note, me).await.db()? {
        return Err(AppError::NotFound);
    }
    Ok(Redirect::to(&format!("{}#post-{}", topic_path(t.id, &t.slug), p.post_number)).into_response())
}

fn staff_only(v: &Viewer) -> Result<(), AppError> {
    v.require_user()?;
    if is_staff(v) { Ok(()) } else { Err(AppError::Forbidden) }
}

async fn list_saved(State(state): State<AppState>, v: Viewer) -> Result<Response, AppError> {
    staff_only(&v)?;
    Ok(Json(tools::saved_replies(state.db()?).await.db()?).into_response())
}

#[derive(Deserialize)]
struct SavedBody {
    title: String,
    body: String,
}

fn check_saved(b: &SavedBody) -> Result<(), AppError> {
    if b.title.trim().is_empty()
        || b.title.chars().count() > 60
        || b.body.trim().is_empty()
        || b.body.chars().count() > 10_000
    {
        return Err(AppError::Validation("A saved reply needs a short title and some text.".into()));
    }
    Ok(())
}

async fn create_saved(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Json(b): Json<SavedBody>,
) -> Result<Response, AppError> {
    v.check_csrf_headers(&headers)?;
    staff_only(&v)?;
    check_saved(&b)?;
    let id = tools::create_saved_reply(state.db()?, &b.title, &b.body).await.db()?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))).into_response())
}

async fn update_saved(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(b): Json<SavedBody>,
) -> Result<Response, AppError> {
    v.check_csrf_headers(&headers)?;
    staff_only(&v)?;
    check_saved(&b)?;
    if !tools::update_saved_reply(state.db()?, id, &b.title, &b.body).await.db()? {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn delete_saved(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    v.check_csrf_headers(&headers)?;
    staff_only(&v)?;
    if !tools::delete_saved_reply(state.db()?, id).await.db()? {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// What the person typed: everything before the quoted original.
pub(crate) fn strip_quoted(text: &str) -> String {
    let mut kept = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        let wrote = t.starts_with("On ") && t.ends_with("wrote:");
        if wrote || t == "--" || t.starts_with("-----Original Message") {
            break;
        }
        if !t.starts_with('>') {
            kept.push(line);
        }
    }
    kept.join("\n").trim().to_owned()
}

/// The key in `reply+KEY@domain`.
pub(crate) fn reply_key(to: &str) -> Option<&str> {
    let key = to.trim().trim_start_matches('<').strip_prefix("reply+")?.split('@').next()?;
    (!key.is_empty() && key.len() <= 100).then_some(key)
}

/// The key and forum in `post+KEY+FORUM@domain`.
pub(crate) fn post_address(to: &str) -> Option<(&str, i64)> {
    let local = to.trim().trim_start_matches('<').strip_prefix("post+")?.split('@').next()?;
    let (key, node) = local.rsplit_once('+')?;
    (!key.is_empty() && key.len() <= 100).then_some(())?;
    Some((key, node.parse().ok()?))
}

/// Shows or replaces the member's private address for posting by email.
async fn new_mail_key(
    State(state): State<AppState>,
    v: Viewer,
    Form(f): Form<crate::http::viewer::CsrfForm>,
) -> Result<Response, AppError> {
    v.check_session_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let key = bf_db::forum::tools::new_mail_key(state.db()?, user.id.get()).await.db()?;
    let domain = std::env::var("BF_REPLY_DOMAIN").unwrap_or_else(|_| "your-mail-domain".into());
    let html = format!(
        "<p>{tr1} <code>post+{key}+FORUM@{domain}</code>{tr2}</p>",
        tr1 = trh("Send a mail to"),
        tr2 = trh(
            ", with FORUM as the forum's number in its address. The subject becomes the title. Anyone with this address can post as you, so keep it private. Asking again replaces it."
        )
    );
    Ok(crate::http::ui::simple_page(&state, &v, &v.t().msg("Post by email"), &html, false))
}

#[derive(Deserialize)]
struct InboundMail {
    to: String,
    #[serde(default)]
    text: String,
    /// Used when the mail has no plain-text part.
    #[serde(default)]
    html: String,
    #[serde(default)]
    subject: String,
}

/// A mail provider's inbound webhook, with `Authorization: Bearer $BF_INBOUND_TOKEN`. The address
/// carries a key made for one member and topic; all the usual posting rules then apply. A mail
/// we will not post still answers 200, so the provider does not retry it.
async fn inbound_mail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(m): Json<InboundMail>,
) -> Result<Response, AppError> {
    let expected = std::env::var("BF_INBOUND_TOKEN").ok().filter(|t| t.len() >= 32).ok_or(AppError::NotFound)?;
    let given =
        headers.get(header::AUTHORIZATION).and_then(|h| h.to_str().ok()).and_then(|h| h.strip_prefix("Bearer "));
    if !given.is_some_and(|g| crate::admin::api::constant_time_eq(g.as_bytes(), expected.as_bytes())) {
        return Err(AppError::Forbidden);
    }
    process_inbound(&state, std::slice::from_ref(&m.to), &m.subject, &m.text, &m.html, &[]).await;
    Ok(StatusCode::OK.into_response())
}

/// One incoming mail, from the webhook or the mailbox: the first address that is one of ours
/// decides what it is (a new topic, a reply to a topic, a reply to a private message). The text
/// is the plain part, or the HTML part with its tags removed. A mail that cannot be posted is
/// answered to the member who owns the key, at their own address, with the reason.
pub(crate) async fn process_inbound(
    state: &AppState,
    to: &[String],
    subject: &str,
    text: &str,
    html: &str,
    files: &[(String, Vec<u8>)],
) {
    let Some(pool) = &state.pool else { return };
    let body = strip_quoted(&if text.trim().is_empty() { html_to_text(html) } else { text.to_owned() });
    for addr in to {
        let (user, outcome) = if let Some((key, node)) = post_address(addr) {
            let Ok(Some(user)) = bf_db::forum::tools::mail_key_owner(pool, key).await else { continue };
            let outcome = match attach_files(state, user, &body, files).await {
                Ok(body) => crate::forum::posting::topic_by_email(state, user, node, subject, &body).await,
                Err(e) => Err(e),
            };
            (user, outcome)
        } else if let Some(key) = reply_key(addr) {
            if let Ok(Some((user, item))) = tools::reply_target(pool, key).await {
                let outcome = match attach_files(state, user, &body, files).await {
                    Ok(body) => crate::forum::posting::reply_by_email(state, user, item, &body).await,
                    Err(e) => Err(e),
                };
                (user, outcome)
            } else if let Ok(Some((user, conversation))) = tools::conversation_target(pool, key).await {
                let outcome = match attach_files(state, user, &body, files).await {
                    Ok(body) => message_by_email(state, user, conversation, &body).await,
                    Err(e) => Err(e),
                };
                (user, outcome)
            } else {
                continue;
            }
        } else {
            continue;
        };
        if let Err(e) = outcome {
            tracing::info!(user, error = %e, "inbound mail refused");
            bounce(state, user, subject, &e).await;
        }
        return;
    }
}

/// Stores a mail's attachments through the site's upload pipeline, as the member's own uploads
/// (the same trust, size, type, scan, metadata and daily-quota rules as the upload form), and
/// returns the body with their links appended. Any file refused refuses the whole mail.
async fn attach_files(
    state: &AppState,
    user: i64,
    body: &str,
    files: &[(String, Vec<u8>)],
) -> Result<String, AppError> {
    use crate::forum::uploads::{Accept, allowed_types, sniff, store};
    if files.is_empty() {
        return Ok(body.to_owned());
    }
    let pool = state.db()?;
    let u = bf_db::accounts::users::user_by_id(pool, user.into()).await.db()?.ok_or(AppError::NotFound)?;
    if u.silenced {
        return Err(AppError::Forbidden);
    }
    crate::moderation::gate::account_ready(state, &u).map_err(AppError::Validation)?;
    crate::http::ratelimit::UPLOADS_BY_USER.enforce(&format!("upload:{}", u.id))?;
    if files.len() > 10 {
        return Err(AppError::Validation("Files cannot be added here.".into()));
    }
    let s = state.settings();
    let staff = u.is_admin || u.groups.contains(&bf_core::ids::GroupId::MODERATORS);
    let need = s.int("upload_min_trust_level");
    if !staff && i64::from(u.trust_level) < need {
        return Err(AppError::invalid("Files can be added from trust level {n}.", &[("n", &need)]));
    }
    let group = bf_db::accounts::limits::limits(pool, &u.groups).await.db()?;
    let max_kb = group.max_upload_kb.unwrap_or_else(|| s.int("max_upload_kb"));
    let allowed = allowed_types(state, false);
    let mut out = body.trim_end().to_owned();
    for (filename, bytes) in files {
        if bytes.len() as i64 > max_kb * 1024 {
            return Err(AppError::invalid("Files can be at most {max_kb} KB.", &[("max_kb", &max_kb)]));
        }
        if let Some((_, ext, _)) = sniff(bytes, filename)
            && !allowed.contains(&ext)
        {
            return Err(if allowed.is_empty() {
                AppError::Validation("Files cannot be added here.".into())
            } else {
                AppError::invalid(
                    "Only these kinds of file can be added here: {types}.",
                    &[("types", &allowed.join(", "))],
                )
            });
        }
        let stored = store(state, u.id.get(), filename, bytes, Accept::Files, max_kb).await?;
        let up = &stored.upload;
        let url = format!("/uploads/{}", up.storage_key);
        let label = up.filename.replace(['[', ']'], "");
        let size = up.width.zip(up.height).map(|(w, h)| format!("#{w}x{h}")).unwrap_or_default();
        out.push_str("\n\n");
        if stored.is_image {
            out.push_str(&format!("![{label}]({url}{size})"));
        } else {
            out.push_str(&format!("[{label} ({})]({url})", bf_core::html::human_size(up.size)));
        }
    }
    Ok(out)
}

/// A reply to a private message that arrived by email. The member must still be in the
/// conversation and able to write; the length limit of the site's own form applies.
async fn message_by_email(state: &AppState, user: i64, conversation: i64, body: &str) -> Result<(), AppError> {
    let pool = state.db()?;
    let u = bf_db::accounts::users::user_by_id(pool, user.into()).await.db()?.ok_or(AppError::NotFound)?;
    if bf_db::accounts::users::is_suspended(pool, user).await.db()? || !u.approved || u.legal_pending || u.silenced {
        return Err(AppError::Forbidden);
    }
    let body = body.trim();
    if body.is_empty() || body.chars().count() > 20_000 {
        return Err(AppError::Validation("A message must be 1 to 20000 characters.".into()));
    }
    // The same gate the site's reply form applies: account ready (2FA, email verification, terms),
    // the flood wait, plugin filters, watched words and the new-member link limit.
    let staff = u.is_admin || u.groups.contains(&bf_core::ids::GroupId::MODERATORS);
    crate::moderation::gate::account_ready(state, &u).map_err(AppError::Validation)?;
    bf_db::community::messages::participant_of(pool, conversation, user).await.db()?.ok_or(AppError::NotFound)?;
    let flood = state.settings().int("flood_wait_seconds");
    if flood > 0
        && !staff
        && let Some(since) = bf_db::community::messages::seconds_since_last_message(pool, user).await.db()?
        && since < flood
    {
        return Err(AppError::invalid("Please wait {n} more seconds.", &[("n", &(flood - since))]));
    }
    let body = crate::platform::plugins::before_post(state, user, body).await?;
    let (text, held) = if staff {
        (body, false)
    } else {
        let tree = state.forum.get(pool).await?;
        match bf_core::watched::apply(&tree.words, &body) {
            bf_core::watched::Outcome::Blocked(word) => {
                return Err(AppError::invalid("That text contains a blocked word: {word}", &[("word", &word)]));
            }
            bf_core::watched::Outcome::Allow { text, needs_approval, .. } => (text, needs_approval),
        }
    };
    if !staff {
        let from = text.clone();
        let cooked = tokio::task::spawn_blocking(move || bf_cook::cook(&from)).await.map_err(AppError::internal)?;
        let s = state.settings();
        if i64::from(u.post_count) < s.int("new_user_posts") && cooked.links.len() as i64 > s.int("new_user_max_links")
        {
            return Err(AppError::invalid(
                "New members can include at most {n} links here.",
                &[("n", &s.int("new_user_max_links"))],
            ));
        }
    }
    if held {
        let payload = serde_json::json!({ "conversation": conversation, "body": text });
        return crate::moderation::held::hold(state, user, "message", None, payload).await;
    }
    crate::community::messages::deliver_reply(state, conversation, (user, &u.username), &text).await
}

/// Tells a member, at their own address, that the mail they sent was not posted.
async fn bounce(state: &AppState, user: i64, subject: &str, why: &AppError) {
    let Some(pool) = &state.pool else { return };
    let Ok(Some(email)) =
        sqlx::query_scalar::<_, String>("SELECT email FROM users WHERE id = $1").bind(user).fetch_optional(pool).await
    else {
        return;
    };
    let t = crate::http::i18n::user_t(state, user).await;
    let site = state.settings().str("site_name").to_owned();
    let reason = match why {
        AppError::Validation(m) => t.msg(m),
        AppError::Forbidden => t.msg("You are not allowed to post this here."),
        AppError::NotFound => t.msg("The topic or forum could not be found."),
        _ => t.msg("Something went wrong on our side. Try again later."),
    };
    let body = t.msgf(
        "Your email “{subject}” to {site} was not posted.\n\nReason: {reason}\n\n— {site}\n",
        &[("subject", subject), ("site", &site), ("reason", &reason)],
    );
    if let Err(e) = bf_db::platform::mail::queue_mail(
        pool,
        &email,
        &t.msgf("Your email to {site} was not posted", &[("site", &site)]),
        &body,
    )
    .await
    {
        tracing::warn!(error = %e, "bounce not queued");
    }
}

/// The text of an HTML mail: tags dropped, line breaks kept at block ends, common entities read.
pub(crate) fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut tag = String::new();
    let mut in_tag = false;
    let mut skip = false;
    for c in html.chars() {
        match c {
            '<' => {
                in_tag = true;
                tag.clear();
            }
            '>' if in_tag => {
                in_tag = false;
                let name = tag
                    .trim_start_matches('/')
                    .split(|c: char| c.is_whitespace() || c == '/')
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if matches!(name.as_str(), "script" | "style") {
                    skip = !tag.starts_with('/');
                } else if matches!(name.as_str(), "br" | "p" | "div" | "li" | "tr" | "h1" | "h2" | "h3" | "blockquote")
                {
                    out.push('\n');
                }
            }
            _ if in_tag => tag.push(c),
            _ if !skip => out.push(c),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_text_is_removed() {
        assert_eq!(strip_quoted("Thanks!\n\nOn Mon, Ann wrote:\n> old"), "Thanks!");
        assert_eq!(strip_quoted("> old\nNew text\n-- \nsig"), "New text");
    }

    #[test]
    fn html_mail_becomes_text() {
        assert_eq!(
            html_to_text("<p>Hi&nbsp;there</p><style>x{}</style><div>a &amp; b<br>c</div>"),
            "
Hi there

a & b
c"
        );
    }

    #[test]
    fn post_addresses_name_key_and_forum() {
        assert_eq!(post_address("post+abc+12@x.example"), Some(("abc", 12)));
        assert_eq!(post_address("post+abc@x.example"), None);
        assert_eq!(post_address("reply+abc+12@x.example"), None);
    }

    #[test]
    fn keys_come_only_from_reply_addresses() {
        assert_eq!(reply_key("reply+abc@x.example"), Some("abc"));
        assert_eq!(reply_key("<reply+abc@x.example>"), Some("abc"));
        assert_eq!(reply_key("other@x.example"), None);
        assert_eq!(reply_key("reply+@x.example"), None);
    }
}
