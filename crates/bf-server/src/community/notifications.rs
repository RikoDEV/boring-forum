//! Sending notifications: preferences, the reader's language, read permissions and live updates
//! in one call, so a new kind only has to say who and what.

use crate::http::render::layout;
use crate::http::render::page;
use crate::http::render::unread_count;
use crate::http::render::{DbResult, ago};
use crate::http::viewer::CsrfForm;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use bf_core::AppError;
use bf_core::i18n::T;
use bf_core::perms::{Actor, Perms};
use bf_core::slug::topic_path;
use bf_web::pages::{FormPage, NotificationView, NotificationsPage, render};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/notifications", get(notifications))
        .route("/api/v1/me/notifications", get(recent_notifications))
        .route("/notifications/{id}/open", get(open_notification))
        .route("/notifications/read", post(read_all_notifications))
        .route("/unsubscribe/{token}", get(unsubscribe_form).post(unsubscribe))
        .route("/unsubscribe/{token}/topic", get(mute_topic_form).post(mute_topic))
}

/// `from` is whoever caused this (0 for the system): members ignoring them hear nothing.
pub(crate) async fn send(state: &AppState, from: i64, to: &[i64], kind: &str, data: &Value) {
    send_quoting(state, from, to, kind, data, None).await;
}

/// How long the quote of a post in its email may be.
const EXCERPT_CHARS: usize = 300;

/// The words of post `number` that the email about it may carry, when the site allows that: only
/// ever for recipients already checked to read the topic. Not kept with the notification.
pub(crate) async fn mail_quote(state: &AppState, topic: i64, number: i32) -> Option<String> {
    if !state.settings().bool("mail_post_excerpts") {
        return None;
    }
    let pool = state.pool.as_ref()?;
    let raw = bf_db::community::notifications::visible_post_raw(pool, topic, number).await.ok().flatten()?;
    let text = tokio::task::spawn_blocking(move || bf_cook::mail_excerpt(&raw, EXCERPT_CHARS)).await.ok()?;
    (!text.is_empty()).then_some(text)
}

/// [`send`], with the author's words quoted in the email: `(author, excerpt)`.
async fn send_quoting(state: &AppState, from: i64, to: &[i64], kind: &str, data: &Value, quote: Option<(&str, &str)>) {
    let Some(pool) = &state.pool else { return };
    let to: Vec<i64> = to.iter().copied().filter(|id| *id != from).collect();
    if to.is_empty() {
        return;
    }
    let by_locale = match bf_db::community::notifications::recipients_by_locale(pool, &to).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(error = %e, kind, "notification recipients failed");
            return;
        }
    };
    let site = state.settings().str("site_name").to_owned();
    let delay = state.settings().int("email_delay_minutes") as i32;
    let url = data.get("url").and_then(Value::as_str).unwrap_or("/notifications");
    let abs = if url.starts_with('/') { format!("{}{url}", state.config.base_url) } else { url.to_owned() };
    // Email keeps the title in quotes, where it stands apart from the sentence.
    let mut quoted = data.clone();
    if let Some(title) = data.get("title").and_then(Value::as_str) {
        quoted["title"] = json!(format!("\"{title}\""));
    }
    for (locale, ids) in by_locale {
        let t =
            T::new(locale.as_deref().unwrap_or(bf_core::i18n::site_language(state.settings().str("default_locale"))));
        let line = bf_core::notifications::text(&t, kind, &quoted);
        let subject = format!("{line} - {site}");
        let body = t.msgf(
            "{line}:\n\n{link}\n\nChange which emails you get under Account settings.\n\n— {site}\n",
            &[("line", &line), ("link", &abs), ("site", &site)],
        );
        let body = match quote {
            Some((author, text)) => bf_web::mail::with_quote(&body, &abs, author, text),
            None => body,
        };
        match bf_db::community::notifications::deliver(
            pool,
            &state.config.base_url,
            from,
            &ids,
            kind,
            data,
            &subject,
            &body,
            delay,
        )
        .await
        {
            Ok(app) => {
                for id in app {
                    state.hub.publish(&format!("user:{id}"), "notification", data.clone());
                }
            }
            Err(e) => tracing::error!(error = %e, kind, "notification delivery failed"),
        }
    }
}

/// Keeps only the members who can read `node`, so nobody hears about a post they cannot open.
pub(crate) async fn can_read(state: &AppState, pool: &PgPool, node: i64, ids: Vec<i64>) -> Vec<i64> {
    if ids.is_empty() {
        return ids;
    }
    let (tree, users) = match (state.forum.get(pool).await, bf_db::accounts::users::users_by_ids(pool, &ids).await) {
        (Ok(t), Ok(u)) => (t, u),
        (t, u) => {
            tracing::error!(topic_tree = t.is_ok(), users = u.is_ok(), "permission check for notifications failed");
            return vec![]; // better silent than leaking a private topic's title
        }
    };
    users
        .into_iter()
        .filter(|u| {
            let actor = Actor {
                user_id: Some(u.id),
                groups: u.groups.clone(),
                is_admin: u.is_admin,
                silenced: u.silenced,
                moderated_nodes: u.moderated_nodes.clone(),
                denied: crate::platform::plugins::denials_cached(u.id.get()),
            };
            tree.perms(&actor, node).has(Perms::READ)
        })
        .map(|u| u.id.get())
        .collect()
}

/// The notifications the viewer can still open: those about a topic in a forum they can no
/// longer read are left out (the topic's title and author would leak). One query for the page.
async fn readable_rows(
    state: &AppState,
    v: &Viewer,
    rows: Vec<bf_db::community::notifications::Row>,
) -> Result<Vec<bf_db::community::notifications::Row>, AppError> {
    let topic_of = |n: &bf_db::community::notifications::Row| {
        n.data
            .get("url")
            .and_then(Value::as_str)
            .and_then(bf_db::community::notifications::topic_in_url)
            .filter(|_| !bf_core::notifications::is_mandatory(&n.kind))
    };
    let mut ids: Vec<i64> = rows.iter().filter_map(topic_of).collect();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Ok(rows);
    }
    let pool = state.db()?;
    let nodes: std::collections::HashMap<i64, i64> =
        bf_db::community::notifications::topic_nodes(pool, &ids).await.db()?.into_iter().collect();
    let tree = state.forum.get(pool).await?;
    Ok(rows
        .into_iter()
        .filter(|n| match topic_of(n) {
            None => true,
            Some(t) => nodes.get(&t).is_some_and(|node| tree.perms(&v.actor, *node).has(Perms::READ)),
        })
        .collect())
}

/// The header badge: the unread notifications the list would show, from the newest 99. The
/// stored count is only the cheap upper bound (nothing unread means nothing to look up).
pub(crate) async fn visible_unread(state: &AppState, v: &Viewer) -> i64 {
    let Some(user) = v.user.as_ref() else { return 0 };
    if user.unread_notifications <= 0 {
        return 0;
    }
    let Ok(pool) = state.db() else { return user.unread_notifications };
    let rows = match bf_db::community::notifications::list_unread(pool, user.id.get(), 99).await {
        Ok(r) => r,
        Err(_) => return user.unread_notifications,
    };
    match readable_rows(state, v, rows).await {
        Ok(rows) => rows.len() as i64,
        Err(_) => user.unread_notifications,
    }
}

/// Drops held notification mail that the recipient may no longer read (the topic moved to a
/// forum they cannot open, or access was withdrawn) before it goes out.
async fn drop_unreadable_mail(state: &AppState) {
    let Some(pool) = &state.pool else { return };
    let held = match bf_db::community::notifications::held_mail_topics(pool, 500).await {
        Ok(h) if !h.is_empty() => h,
        Ok(_) => return,
        Err(e) => {
            tracing::warn!(error = %e, "held mail lookup failed");
            return;
        }
    };
    let mut by_node: std::collections::BTreeMap<i64, Vec<i64>> = std::collections::BTreeMap::default();
    for (_, user, _, node) in &held {
        by_node.entry(*node).or_default().push(*user);
    }
    let mut readable: std::collections::HashSet<(i64, i64)> = std::collections::HashSet::default();
    for (node, users) in by_node {
        for user in can_read(state, pool, node, users).await {
            readable.insert((node, user));
        }
    }
    let gone: Vec<i64> =
        held.iter().filter(|(_, user, _, node)| !readable.contains(&(*node, *user))).map(|h| h.0).collect();
    if !gone.is_empty()
        && let Err(e) = bf_db::community::notifications::drop_mail(pool, &gone, "skipped: no longer readable").await
    {
        tracing::warn!(error = %e, "dropping unreadable mail failed");
    }
}

/// Usernames quoted in a post: the composer's `> — @ann, [post #3](…)` signature, or BBCode's
/// `[quote=ann]` and `[quote="ann, post:3"]`.
pub(crate) fn quoted_names(raw: &str) -> Vec<String> {
    // A quote shown inside code is an example, not a quote of someone's post.
    let lower = without_code(&raw.to_lowercase());
    let mut names = Vec::new();
    for line in lower.lines() {
        if let Some(rest) = line.strip_prefix("> — @") {
            let name: String = rest.chars().take_while(|c| *c != ',' && !c.is_whitespace()).collect();
            if !name.is_empty() && !names.contains(&name) {
                names.push(name);
            }
        }
    }
    for (i, _) in lower.match_indices("[quote") {
        let rest = &lower[i + 6..];
        let Some(end) = rest.find(']') else { continue };
        let Some(eq) = rest[..end].find('=') else { continue };
        let name: String = rest[eq + 1..end]
            .trim_matches(|c: char| c == '"' || c == '\'' || c.is_whitespace())
            .chars()
            .take_while(|c| !matches!(c, ',' | '"' | '\'') && !c.is_whitespace())
            .collect();
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// `text` without fenced code, inline code and BBCode `[code]` blocks.
fn without_code(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut fence: Option<(char, usize)> = None;
    for line in text.lines() {
        let t = line.trim_start();
        let first = t.chars().next().filter(|c| matches!(c, '`' | '~'));
        let run = first.map_or(0, |c| t.chars().take_while(|x| *x == c).count());
        if let Some((c, n)) = fence {
            if first == Some(c) && run >= n && t[run..].trim().is_empty() {
                fence = None;
            }
            continue;
        }
        if run >= 3 {
            fence = first.map(|c| (c, run));
            continue;
        }
        let mut rest = line;
        while let Some(i) = rest.find('`') {
            out.push_str(&rest[..i]);
            let after = &rest[i..];
            let n = after.chars().take_while(|c| *c == '`').count();
            if let Some(j) = after[n..].find(&after[..n]) {
                rest = &after[n + j + n..];
            } else {
                out.push_str(&after[..n]);
                rest = &after[n..];
            }
        }
        out.push_str(rest);
        out.push('\n');
    }
    // BBCode code blocks, up to the closing tag or the end.
    while let Some(i) = out.find("[code") {
        let end = out[i..].find("[/code]").map_or(out.len(), |j| i + j + 7);
        out.replace_range(i..end, "");
    }
    out
}

/// Topic ids linked from a post on this site, in `/t/slug/123` or `/t/123` form.
pub(crate) fn linked_topics(raw: &str, base_url: &str) -> Vec<i64> {
    let mut ids = Vec::new();
    let raw = raw.replace(base_url, "");
    for (i, _) in raw.match_indices("/t/") {
        // Our base URL is gone by now, so a host in front of this means somebody else's site.
        if raw[..i].chars().next_back().is_some_and(|c| !(c.is_whitespace() || "([<\"'".contains(c))) {
            continue;
        }
        let tail: String =
            raw[i + 3..].chars().take_while(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '/')).collect();
        let id = tail.trim_end_matches('/').rsplit('/').next().and_then(|s| s.parse::<i64>().ok());
        if let Some(id) = id.filter(|id| !ids.contains(id)) {
            ids.push(id);
        }
    }
    ids.truncate(10);
    ids
}

/// Who hears about a new visible post. Each person gets one notification, the most personal:
/// direct reply, quote, mention, then watching, then following the author. Quoting comes before
/// mentions because the composer signs each quote with `@name`.
pub(crate) struct PostNews<'a> {
    pub author_id: i64,
    pub author: &'a str,
    pub topic_id: i64,
    pub node_id: i64,
    pub title: &'a str,
    pub number: i32,
    pub url: &'a str,
    pub mentioned: Vec<i64>,
    pub watchers: Vec<i64>,
}

pub(crate) async fn post_created(state: &AppState, n: PostNews<'_>) -> Vec<(&'static str, Vec<i64>)> {
    let Some(pool) = &state.pool else { return vec![] };
    // Who was told of what, for mail that must not repeat it.
    let mut notified: Vec<(&'static str, Vec<i64>)> = Vec::new();
    let context = bf_db::forum::reply_context::post_context(pool, n.topic_id, n.number).await;
    let (raw, replied_to) = match context {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "post lookup for notifications failed");
            (String::new(), None)
        }
    };
    let mut names = quoted_names(&raw);
    // Quotes ping people like mentions do, so the same cap applies.
    names.truncate(state.settings().int("max_mentions_per_post").max(0) as usize);
    let quoted = if names.is_empty() {
        vec![]
    } else {
        bf_core::ids::raw(&bf_db::accounts::users::user_ids_by_names(pool, &names).await.unwrap_or_default())
    };
    let followers = bf_db::community::follows::followers_of(pool, n.author_id, n.topic_id).await.unwrap_or_default();
    let mut told: Vec<i64> = vec![n.author_id];
    let group = format!("topic:{}", n.topic_id);
    let batches: [(&'static str, Vec<i64>, Value); 5] = [
        ("direct_reply", replied_to.into_iter().collect(), json!({})),
        ("quote", quoted, json!({})),
        ("mention", n.mentioned, json!({})),
        ("reply", n.watchers, json!({ "group": group, "count": 1 })),
        ("followed_post", followers, json!({ "group": format!("{group}:{}", n.author_id), "count": 1 })),
    ];
    let excerpt = mail_quote(state, n.topic_id, n.number).await;
    for (kind, ids, extra) in batches {
        let ids: Vec<i64> = ids.into_iter().filter(|i| !told.contains(i)).collect();
        // Claimed even if unreadable or switched off: a muted watcher should not hear it as a follower.
        told.extend(&ids);
        let ids = can_read(state, pool, n.node_id, ids).await;
        notified.push((kind, ids.clone()));
        let mut data = json!({ "by": n.author, "title": n.title, "url": n.url, "topic_id": n.topic_id });
        if let (Some(d), Some(e)) = (data.as_object_mut(), extra.as_object()) {
            d.extend(e.clone());
        }
        let quote = excerpt.as_deref().map(|text| (n.author, text));
        send_quoting(state, n.author_id, &ids, kind, &data, quote).await;
    }
    // Links to someone else's topic, once per target.
    for target in linked_topics(&raw, &state.config.base_url) {
        if target == n.topic_id {
            continue;
        }
        if let Ok(Some((owner, node))) = bf_db::forum::topics::topic_owner(pool, target).await
            && !told.contains(&owner)
        {
            // Told the title of the linking post, so must be able to read that one too.
            let ids = can_read(state, pool, node, vec![owner]).await;
            let ids = can_read(state, pool, n.node_id, ids).await;
            let data = json!({ "by": n.author, "title": n.title, "url": n.url });
            notified.push(("linked", ids.clone()));
            send(state, n.author_id, &ids, "linked", &data).await;
        }
    }
    notified
}

/// Global staff: something only they handle (a pending account, a member or message report),
/// folded into the same review row.
pub(crate) async fn tell_staff(state: &AppState, user: i64) {
    let Some(pool) = &state.pool else { return };
    match bf_db::community::notifications::staff(pool).await {
        Ok(ids) => {
            let data = json!({ "url": "/review", "group": "review", "count": 1 });
            send(state, user, &ids, "review_queue", &data).await;
        }
        Err(e) => tracing::warn!(error = %e, "staff lookup failed"),
    }
}

/// Moderators of `node` and all staff: a new report or queued post, folded into one row.
pub(crate) async fn tell_reviewers(state: &AppState, node: i64, from: i64) {
    let Some(pool) = &state.pool else { return };
    match bf_db::community::notifications::reviewers(pool, node).await {
        Ok(ids) => {
            let data = json!({ "url": "/review", "group": "review", "count": 1 });
            send(state, from, &ids, "review_queue", &data).await;
        }
        Err(e) => tracing::warn!(error = %e, "reviewer lookup failed"),
    }
}

/// Tells the people going to an event, or its organiser, that something changed. `kind` is
/// `event_changed`, `event_cancelled` or `event_rsvp`.
#[expect(
    clippy::too_many_arguments,
    reason = "one call site per kind of event change; a struct would only rename the arguments"
)]
pub(crate) async fn event_news(
    state: &AppState,
    from: i64,
    to: &[i64],
    kind: &str,
    topic: i64,
    slug: &str,
    title: &str,
    by: &str,
) {
    let mut data = json!({ "title": title, "by": by, "url": topic_path(topic, slug) });
    if kind == "event_rsvp" {
        // Several answers to one event fold into one unread notification.
        data["group"] = json!(format!("event:{topic}"));
        data["count"] = json!(1);
    }
    send(state, from, to, kind, &data).await;
}

/// The maintenance tick: tomorrow's events, and polls closed by any route (a moderator, a bulk
/// action or a timer).
pub(crate) async fn run_tick(state: &AppState) {
    let Some(pool) = &state.pool else { return };
    drop_unreadable_mail(state).await;
    match bf_db::community::polls::due_poll_closures(pool).await {
        Ok(rows) => {
            for (topic, slug, title, node, ids) in rows {
                let ids = can_read(state, pool, node, ids).await;
                let data = json!({ "title": title, "url": topic_path(topic, &slug) });
                send(state, 0, &ids, "poll_closed", &data).await;
            }
        }
        Err(e) => tracing::warn!(error = %e, "poll closures failed"),
    }
    if let Err(e) = bf_db::community::events::advance_repeating(pool).await {
        tracing::warn!(error = %e, "repeating events failed");
    }
    match bf_db::community::events::due_event_reminders(pool).await {
        Ok(rows) => {
            for (topic, slug, title, node, ids, starts_at) in rows {
                let ids = can_read(state, pool, node, ids).await;
                // Each member reads the start in their own time zone.
                let local = bf_db::community::events::local_times(pool, &ids, starts_at).await.unwrap_or_default();
                let mut groups: std::collections::BTreeMap<(String, String), Vec<i64>> =
                    std::collections::BTreeMap::default();
                for (id, text, zone) in local {
                    groups.entry((text, zone)).or_default().push(id);
                }
                for ((text, zone), members) in groups {
                    let data = json!({
                        "title": title,
                        "url": topic_path(topic, &slug),
                        "message": format!("{text} ({zone})"),
                    });
                    send(state, 0, &members, "event_reminder", &data).await;
                }
            }
        }
        Err(e) => tracing::warn!(error = %e, "event reminders failed"),
    }
}

/// A raised trust level, by rule or by hand.
pub(crate) async fn trust_raised(state: &AppState, user: i64, level: i16) {
    trust_notice(state, user, level, "trust_level").await;
}

/// Close to the next level: every requirement is most of the way there.
pub(crate) async fn trust_near(state: &AppState, user: i64, level: i16) {
    trust_notice(state, user, level, "trust_near").await;
}

pub(crate) async fn trust_lowered(state: &AppState, user: i64, level: i16) {
    trust_notice(state, user, level, "trust_lowered").await;
}

async fn trust_notice(state: &AppState, user: i64, level: i16, kind: &str) {
    let t = crate::http::i18n::user_t(state, user).await;
    let data = json!({ "name": crate::community::trust::level_label(state, &t, level), "url": "/trust" });
    send(state, 0, &[user], kind, &data).await;
}

pub(crate) async fn added_to_group(state: &AppState, user: i64, group: i64) {
    let Some(pool) = &state.pool else { return };
    if let Ok(Some(name)) = bf_db::community::groups::group_name(pool, group).await {
        send(state, 0, &[user], "group_added", &json!({ "name": name, "url": "/notifications" })).await;
    }
}

/// The answer to a request to join a group or club.
pub(crate) async fn request_decided(state: &AppState, user: i64, group: i64, approved: bool) {
    if approved {
        added_to_group(state, user, group).await;
    } else if let Some(pool) = &state.pool
        && let Ok(Some(name)) = bf_db::community::groups::group_name(pool, group).await
    {
        send(state, 0, &[user], "group_request_declined", &json!({ "name": name, "url": "/clubs" })).await;
    }
}

#[derive(Deserialize)]
pub(crate) struct NotifQuery {
    #[serde(default)]
    show: String,
}

/// Notification text for a stored row.
/// The newest notifications, for the header dropdown.
#[utoipa::path(get, path = "/api/v1/me/notifications", tag = "me",
    params(("show" = Option<String>, Query, description = "a notification tab")),
    responses((status = 200, body = Object, description = "the newest notifications and the unread count"), (status = 403, body = bf_core::error::Problem)))]
pub(crate) async fn recent_notifications(
    State(state): State<AppState>,
    v: Viewer,
    Query(q): Query<NotifQuery>,
) -> Result<Response, AppError> {
    let user = v.require_user()?;
    let tab = bf_core::notifications::in_tab(&q.show);
    let kinds: Option<&[&str]> = if tab.is_empty() { None } else { Some(&tab) };
    let rows = bf_db::community::notifications::list(state.db()?, user.id.get(), kinds, 15).await.db()?;
    let rows = readable_rows(&state, &v, rows).await?;
    let t = bf_core::i18n::T::new(v.locale);
    let items: Vec<_> = rows
        .iter()
        .map(|n| {
            let text = bf_core::notifications::text(&t, &n.kind, &n.data);
            let field = |k: &str| n.data.get(k).and_then(|v| v.as_str()).filter(|s| !s.is_empty());
            let count = n.data.get("count").and_then(serde_json::Value::as_i64).unwrap_or(1);
            let by = field("by").or_else(|| n.data["users"].get(0).and_then(|u| u.as_str()));
            // "ann" over the topic's title; the icon says what happened. A folded row needs its sentence.
            let (label, description) = match (by, field("title")) {
                (Some(by), Some(title)) if count <= 1 => (Some(by), title.to_owned()),
                _ => (None, text.clone()),
            };
            serde_json::json!({
                "url": format!("/notifications/{}/open", n.id),
                "text": text,
                "label": label,
                "description": description,
                "by": by,
                "when": ago(n.created_at),
                "when_at": n.created_at.to_rfc3339(),
                "read": n.read,
                "icon": bf_core::notifications::icon(&n.kind),
            })
        })
        .collect();
    Ok(axum::Json(items).into_response())
}

async fn notifications(
    State(state): State<AppState>,
    v: Viewer,
    Query(q): Query<NotifQuery>,
) -> Result<Response, AppError> {
    let user = v.require_user()?;
    let pool = state.db()?;
    let tab = bf_core::notifications::in_tab(&q.show);
    let kinds: Option<&[&str]> = if tab.is_empty() { None } else { Some(&tab) };
    let rows = bf_db::community::notifications::list(pool, user.id.get(), kinds, 100).await.db()?;
    let rows = readable_rows(&state, &v, rows).await?;
    let t = bf_core::i18n::T::new(v.locale);
    let items = rows
        .iter()
        .map(|n| NotificationView {
            text: bf_core::notifications::text(&t, &n.kind, &n.data),
            url: format!("/notifications/{}/open", n.id),
            read: n.read,
            when: ago(n.created_at),
        })
        .collect();
    let tabs = bf_core::notifications::TABS
        .iter()
        .map(|(k, label)| {
            (*label, format!("/notifications?show={k}"), q.show == *k || (q.show.is_empty() && *k == "all"))
        })
        .collect();
    let (mut l, set) = layout(&state, &v, Some(&v.t().msg("Notifications")));
    l.unread_notifications = unread_count(&state, &v).await;
    Ok(page(render(&NotificationsPage { layout: l, items, tabs }), set, false))
}

/// Opens one notification: marks it read and follows its link (local paths only).
async fn open_notification(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let user = v.require_user()?;
    let url =
        bf_db::community::notifications::open(state.db()?, user.id.get(), id).await.db()?.ok_or(AppError::NotFound)?;
    // Only local paths are followed, so stored data cannot redirect off-site. A backslash or a
    // control character is refused too: browsers treat `/\host` as protocol-relative.
    let local = |u: &String| {
        u.starts_with('/') && !u.starts_with("//") && !u.contains('\\') && !u.chars().any(char::is_control)
    };
    let url = Some(url).filter(local).unwrap_or_else(|| "/".into());
    Ok(Redirect::to(&url).into_response())
}

async fn read_all_notifications(
    State(state): State<AppState>,
    v: Viewer,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    bf_db::community::notifications::mark_all_read(state.db()?, user.id.get()).await.db()?;
    Ok(Redirect::to("/notifications").into_response())
}

async fn unsubscribe_form(
    State(state): State<AppState>,
    v: Viewer,
    Path(token): Path<String>,
) -> Result<Response, AppError> {
    let (_, kind) = bf_db::platform::mail::token_target(state.db()?, &token).await.db()?.ok_or(AppError::NotFound)?;
    if bf_core::notifications::is_mandatory(&kind) {
        return Ok(cannot_unsubscribe(&v.t()));
    }
    // The kind by the name the settings page gives it.
    let name = bf_core::notifications::find(&kind).map_or_else(|| kind.clone(), |k| v.t().msg(k.label));
    let (mut l, set) = layout(&state, &v, Some(&v.t().msg("Unsubscribe")));
    l.noindex = true;
    let body = FormPage {
        layout: l,
        heading: v.t().msg("Stop these emails?"),
        message: Some(if kind == "digest" {
            v.t().msg("You will stop getting the email digest. You can turn it back on in your account settings.")
        } else {
            v.t().msgf(
                "You will no longer get “{kind}” notifications by email. You still see them on the site.",
                &[("kind", &name)],
            )
        }),
        error: None,
        action: format!("/unsubscribe/{token}"),
        fields: vec![],
        button: v.t().msg("Unsubscribe"),
        links: vec![],
    };
    Ok(page(render(&body), set, false))
}

/// Mutes one topic from a link in its email. A page first: mail scanners follow links.
async fn mute_topic_form(
    State(state): State<AppState>,
    v: Viewer,
    Path(token): Path<String>,
) -> Result<Response, AppError> {
    bf_db::forum::topics::token_topic(state.db()?, &token).await.db()?.ok_or(AppError::NotFound)?;
    let (mut l, set) = layout(&state, &v, Some(&v.t().msg("Mute this topic")));
    l.noindex = true;
    let body = FormPage {
        layout: l,
        heading: v.t().msg("Mute this topic?"),
        message: Some(v.t().msg("You will stop getting notifications about new posts in this topic.")),
        error: None,
        action: format!("/unsubscribe/{token}/topic"),
        fields: vec![],
        button: v.t().msg("Mute"),
        links: vec![],
    };
    Ok(page(render(&body), set, false))
}

async fn mute_topic(State(state): State<AppState>, v: Viewer, Path(token): Path<String>) -> Result<Response, AppError> {
    let pool = state.db()?;
    let (user, item) = bf_db::forum::topics::token_topic(pool, &token).await.db()?.ok_or(AppError::NotFound)?;
    bf_db::community::follows::set_follow(pool, user, "item", item, bf_db::community::follows::Level::Mute)
        .await
        .db()?;
    let t = v.t();
    Ok(done_page(&t, &t.msg("Muted"), &t.msg("This topic is muted.")))
}

/// A bare confirmation page, for links followed from an email.
fn done_page(t: &T, title: &str, line: &str) -> Response {
    let html = format!(
        "<!doctype html><html lang=\"{}\"><title>{}</title><p>{}</p>",
        t.locale,
        bf_core::html::esc(title),
        bf_core::html::esc(line)
    );
    (StatusCode::OK, Html(html)).into_response()
}

/// An account notice ignores preferences: saying "unsubscribed" would not be true.
fn cannot_unsubscribe(t: &T) -> Response {
    done_page(
        t,
        &t.msg("This email cannot be turned off"),
        &t.msg("It is a notice about your account, which is always sent."),
    )
}

/// RFC 8058 one-click unsubscribe; also the confirm button on the page above.
async fn unsubscribe(
    State(state): State<AppState>,
    v: Viewer,
    Path(token): Path<String>,
) -> Result<Response, AppError> {
    let pool = state.db()?;
    let (user_id, kind) = bf_db::platform::mail::token_target(pool, &token).await.db()?.ok_or(AppError::NotFound)?;
    // Mail sent before account notices lost their link may still carry one.
    if bf_core::notifications::is_mandatory(&kind) {
        return Ok(cannot_unsubscribe(&v.t()));
    }
    bf_db::platform::mail::unsubscribe(pool, user_id, &kind).await.db()?;
    let t = v.t();
    Ok(done_page(&t, &t.msg("Unsubscribed"), &t.msg("You are unsubscribed from these emails.")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_quoted_names() {
        assert_eq!(quoted_names("[quote=Ann]hi[/quote] [quote=\"bob, post:3\"]x[/quote]"), ["ann", "bob"]);
        assert_eq!(quoted_names("[quote]anon[/quote]"), Vec::<String>::new());
        assert_eq!(quoted_names("> hi\n>\n> — @Ann, [post #2](/t/x/1#post-2)\n\nyes"), ["ann"]);
        assert_eq!(quoted_names("```\n> — @ann, x\n```\n`[quote=bob]`\n[code][quote=cy][/code]"), Vec::<String>::new());
    }

    #[test]
    fn finds_internal_links() {
        let base = "https://f.example";
        assert_eq!(linked_topics("see https://f.example/t/hello-there/12#post-3 and /t/7", base), [12, 7]);
        assert_eq!(linked_topics("https://other.example/x and /t/not-a-number", base), Vec::<i64>::new());
    }
}
