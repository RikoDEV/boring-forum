//! More operations behind `bf.request`: members, settings, mail, realtime, schedules, new topics
//! and outbound requests. Each is gated by a capability (checked in `host.rs`) and, where it
//! reaches people or other servers, by a per-plugin rate limit (settings `plugin_*_per_*`).

use crate::http::render::pool;
use crate::state::AppState;
use bf_core::i18n::en;
use bf_core::perms::{Actor, Perms};
use bf_db::platform::plugins;
use serde_json::{Value, json};

use super::stats::allow;
use super::{MAX_VALUE, http_allowed};

const MAX_EMAIL_TEXT: usize = 5000;
const MAX_EVENT_DATA: usize = 4096;
const MAX_HTTP_ANSWER: usize = 256 * 1024;
const MAX_HEADERS: usize = 20;

pub(super) fn failed<E>(_: E) -> String {
    en("The operation failed.").to_owned()
}

/// A list field of the request (`user_ids`, `keys`); empty when absent or of the wrong type.
pub(super) fn list<T: serde::de::DeserializeOwned>(req: &Value, field: &str) -> Vec<T> {
    req.get(field).and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
}

/// `user.get`: what a profile page shows to anyone. Never the email, the address or the settings.
async fn user_get(state: &AppState, req: &Value) -> Result<Value, String> {
    let pool = pool(state).map_err(failed)?;
    let id = req.get("user_id").and_then(Value::as_i64).unwrap_or_default();
    let row: Option<(i64, String, Option<String>, i16, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as("SELECT id, username, display_name, trust_level, joined_at FROM users WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await
            .map_err(failed)?;
    let (id, username, display_name, trust_level, joined_at) = row.ok_or(en("No such member."))?;
    let groups: Vec<(i64, String)> = sqlx::query_as(
        "SELECT g.id, g.name FROM groups g WHERE g.id = (SELECT primary_group_id FROM users WHERE id = $1)
            OR g.id IN (SELECT group_id FROM user_secondary_groups WHERE user_id = $1) ORDER BY g.id",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .map_err(failed)?;
    Ok(json!({
        "id": id, "username": username, "display_name": display_name, "trust_level": trust_level,
        "created_at": joined_at, "groups": groups.into_iter().map(|(id, name)| json!({ "id": id, "name": name })).collect::<Vec<_>>(),
    }))
}

/// `user.can`: whether a member holds a permission on a forum.
async fn user_can(state: &AppState, plugin: &str, req: &Value) -> Result<Value, String> {
    let pool = pool(state).map_err(failed)?;
    let id = req.get("user_id").and_then(Value::as_i64).unwrap_or_default();
    let node = req.get("node_id").and_then(Value::as_i64).unwrap_or_default();
    let perm = req.get("perm").and_then(Value::as_str).unwrap_or_default();
    // The plugin's own permission: the groups the administrator chose.
    if let Some(held) = super::registry::holds(state, plugin, perm, id).await {
        return Ok(json!({ "allowed": held }));
    }
    let bit = match perm {
        "view" => Perms::VIEW,
        "read" => Perms::READ,
        "create" => Perms::CREATE,
        "reply" => Perms::REPLY,
        "attach" => Perms::ATTACH,
        "download" => Perms::DOWNLOAD,
        "moderate" => Perms::MODERATE,
        _ => return Err(en("Use perm view, read, create, reply, attach, download, moderate or one of your own permission keys.").into()),
    };
    let u = bf_db::accounts::users::users_by_ids(pool, &[id])
        .await
        .map_err(failed)?
        .into_iter()
        .next()
        .ok_or(en("No such member."))?;
    let tree = state.forum.get(pool).await.map_err(failed)?;
    if tree.node(node).is_none() {
        return Err(en("No such forum.").into());
    }
    let actor = Actor {
        user_id: Some(u.id),
        groups: u.groups.clone(),
        is_admin: u.is_admin,
        silenced: u.silenced,
        moderated_nodes: u.moderated_nodes.clone(),
        denied: super::gate::denials(state, u.id.get(), &u.groups).await,
    };
    let actor = if u.approved { actor } else { crate::http::viewer::pending_actor(&u) };
    Ok(json!({ "allowed": tree.perms(&actor, node).has(bit) }))
}

/// `settings.get`: the site settings that are public already (the ones sent to guests' browsers),
/// one by `key` or all of them. Secrets and staff-only settings are not reachable.
fn settings_get(state: &AppState, plugin: &str, req: &Value) -> Result<Value, String> {
    let public = state.settings().public_values();
    match req.get("key").and_then(Value::as_str) {
        // A setting the plugin declared itself (`plugin:<id>:<key>`) is readable by it alone.
        Some(key) => {
            let own = key.starts_with(&format!("plugin:{plugin}:"));
            let found = if own { state.settings().get(key).cloned() } else { public.get(key).cloned() };
            found.map(|v| json!({ "value": v })).ok_or(en("No such setting.").into())
        }
        None => Ok(json!({ "settings": public })),
    }
}

/// A message of the plugin's own catalog for this locale, then its base language, then English.
pub(super) fn catalog_text(catalogs: &Value, locale: &str, key: &str) -> Option<String> {
    let base = locale.split(['-', '_']).next().unwrap_or(locale);
    [locale, base, "en"]
        .iter()
        .find_map(|l| catalogs.get(*l)?.get(key)?.as_str())
        .map(str::to_owned)
}

/// `{name}` placeholders filled from `vars`.
pub(super) fn fill(text: &str, vars: Option<&Value>) -> String {
    let mut out = text.to_owned();
    for (k, v) in vars.and_then(Value::as_object).into_iter().flatten() {
        let value = v.as_str().map_or_else(|| v.to_string(), str::to_owned);
        out = out.replace(&format!("{{{k}}}"), &value);
    }
    out
}

/// `email.send`: to a member by id, in their language, to the address they verified. The text is
/// plain; `subject_key` and `text_key` read the plugin's catalog in the recipient's language.
async fn email_send(state: &AppState, plugin: &str, catalogs: &Value, req: &Value) -> Result<Value, String> {
    let pool = pool(state).map_err(failed)?;
    let id = req.get("user_id").and_then(Value::as_i64).unwrap_or_default();
    if !allow(plugin, "email", 3600, state.settings().int("plugin_emails_per_hour"), 1) {
        return Err(en("This plugin has sent too much email in the last hour.").into());
    }
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT email, username FROM users WHERE id = $1 AND email_verified_at IS NOT NULL AND NOT email_bad",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(failed)?;
    let (to, username) = row.ok_or(en("That member cannot receive email."))?;
    let t = crate::http::i18n::user_t(state, id).await;
    let locale = t.locale.to_owned();
    let pick = |plain: &str, key: &str| -> String {
        let text = req
            .get(key)
            .and_then(Value::as_str)
            .and_then(|k| catalog_text(catalogs, &locale, k))
            .or_else(|| req.get(plain).and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_default();
        fill(&text, req.get("vars"))
    };
    let subject = pick("subject", "subject_key");
    let body = pick("text", "text_key");
    if subject.trim().is_empty() || subject.chars().count() > 150 || subject.contains(['\r', '\n']) {
        return Err(en("The subject must be 1 to 150 characters on one line.").into());
    }
    if body.trim().is_empty() || body.chars().count() > MAX_EMAIL_TEXT {
        return Err(en("The text must be 1 to 5000 characters.").into());
    }
    let site = state.settings().str("site_name").to_owned();
    let full = t.msgf(
        "Hi {name},\n\n{text}\n\n— {site} (sent by the plugin {plugin})\n",
        &[("name", &username), ("text", &body), ("site", &site), ("plugin", plugin)],
    );
    bf_db::platform::mail::queue_mail(pool, &to, &subject, &full).await.map_err(failed)?;
    Ok(json!({ "queued": true }))
}

/// `realtime.push`: an event on each member's own live channel (`user`), named
/// `plugin.<id>.<event>` so it cannot be taken for one of the site's.
fn realtime_push(state: &AppState, plugin: &str, req: &Value) -> Result<Value, String> {
    let users: Vec<i64> = list(req, "user_ids");
    let event = req.get("event").and_then(Value::as_str).unwrap_or_default();
    let data = req.get("data").cloned().unwrap_or_else(|| json!({}));
    let good = !event.is_empty()
        && event.len() <= 40
        && event.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'));
    if users.is_empty() || users.len() > 50 || !good || data.to_string().len() > MAX_EVENT_DATA {
        return Err(en("Send 1 to 50 user_ids, an event name and up to 4 KB of data.").into());
    }
    if !allow(plugin, "realtime", 60, 600, i64::try_from(users.len()).unwrap_or(50)) {
        return Err(en("This plugin has pushed too many events in the last minute.").into());
    }
    for u in &users {
        state.hub.publish(&format!("user:{u}"), &format!("plugin.{plugin}.{event}"), data.clone());
    }
    Ok(json!({ "sent": users.len() }))
}

/// `schedule`: a recurring job. `{"name", "every_minutes": 5..43200, "payload"}`, or
/// `{"name", "cancel": true}`. The plugin's `job.run` hook gets the payload plus `"schedule": name`.
async fn schedule(state: &AppState, plugin: &str, req: &Value) -> Result<Value, String> {
    let pool = pool(state).map_err(failed)?;
    let name = req.get("name").and_then(Value::as_str).unwrap_or_default();
    if name.is_empty() || name.len() > 40 || !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_')) {
        return Err(en("Name the schedule with lowercase letters, digits, dashes and underscores.").into());
    }
    if req.get("cancel").and_then(Value::as_bool).unwrap_or(false) {
        return Ok(json!({ "cancelled": plugins::cancel_schedule(pool, plugin, name).await.map_err(failed)? }));
    }
    let every = req.get("every_minutes").and_then(Value::as_i64).unwrap_or_default();
    if !(5..=43_200).contains(&every) {
        return Err(en("every_minutes must be between 5 and 43200.").into());
    }
    let payload = req.get("payload").cloned().unwrap_or_else(|| json!({}));
    if payload.to_string().len() > MAX_VALUE {
        return Err(en("The payload is too large.").into());
    }
    let max = state.settings().int("plugin_max_schedules");
    let every = i32::try_from(every).unwrap_or(60);
    if plugins::set_schedule(pool, plugin, name, every, &payload, max).await.map_err(failed)? {
        Ok(json!({ "scheduled": true }))
    } else {
        Err(en("This plugin has too many schedules.").into())
    }
}

/// `content.create_topic`: a topic by the plugin's own account, in a forum that account may post
/// in, with the same hourly limit as replies. Plugin hooks do not run for it.
async fn create_topic(state: &AppState, plugin: &str, req: &Value) -> Result<Value, String> {
    let pool = pool(state).map_err(failed)?;
    let node = req.get("node_id").and_then(Value::as_i64).unwrap_or_default();
    let title = req.get("title").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
    let body = req.get("body").and_then(Value::as_str).unwrap_or_default();
    if title.chars().count() < 3 || title.chars().count() > 200 || body.trim().is_empty() || body.chars().count() > 20_000 {
        return Err(en("The title must be 3 to 200 characters and the body 1 to 20000.").into());
    }
    let bot = plugins::bot_user(pool, plugin).await.map_err(failed)?.ok_or(en("This plugin has no account."))?;
    if plugins::posts_last_hour(pool, bot).await.map_err(failed)? >= state.settings().int("plugin_posts_per_hour") {
        return Err(en("This plugin has posted too much in the last hour.").into());
    }
    let tree = state.forum.get(pool).await.map_err(failed)?;
    let node_row = tree.node(node).ok_or(en("No such forum."))?;
    let actor = Actor { user_id: Some(bot.into()), groups: vec![2.into()], ..Default::default() };
    if node_row.kind == "archive" || !tree.perms(&actor, node).has(Perms::CREATE) {
        return Err(en("This plugin may not post in that forum.").into());
    }
    let cooked = super::cook(state, body).await;
    let post = bf_db::forum::posts::NewPost {
        author_id: bot,
        raw: body,
        cooked: &cooked.html,
        cooked_version: bf_cook::COOK_VERSION,
        ip: None,
        pending: false,
    };
    let slug = bf_core::slug::slugify(&title);
    let topic = bf_db::forum::posts::create_topic(pool, node, &title, &slug, &post).await.map_err(failed)?;
    bf_db::forum::links::record_links(pool, topic, 1, &cooked.links).await.map_err(failed)?;
    state.forum.invalidate();
    crate::forum::posting::after_plugin_post(state, bot, &plugins::bot_username(plugin), &title, &slug, topic, node, 1).await;
    Ok(json!({ "id": topic, "slug": slug }))
}

/// `http_request`: GET, POST, PUT, PATCH or DELETE to a host the administrator approved, through
/// the same SSRF-safe fetcher as webhooks: https only, public addresses only, no redirects.
/// `{"method", "url", "headers": {...}, "body": text | "json": value}` answers
/// `{"status", "body"}` (text, cut at 256 KB).
pub(super) async fn http_request(
    state: &AppState,
    plugin: &str,
    hosts: &[String],
    req: &Value,
) -> Result<Value, String> {
    let url = req.get("url").and_then(Value::as_str).unwrap_or_default();
    if !http_allowed(hosts, url) {
        return Err(en("This plugin may not contact that host.").into());
    }
    let method = match req.get("method").and_then(Value::as_str).unwrap_or("GET").to_ascii_uppercase().as_str() {
        "GET" => reqwest::Method::GET,
        "POST" => reqwest::Method::POST,
        "PUT" => reqwest::Method::PUT,
        "PATCH" => reqwest::Method::PATCH,
        "DELETE" => reqwest::Method::DELETE,
        _ => return Err(en("Use GET, POST, PUT, PATCH or DELETE.").into()),
    };
    let mut headers: Vec<(String, String)> = Vec::new();
    for (k, v) in req.get("headers").and_then(Value::as_object).into_iter().flatten() {
        let v = v.as_str().unwrap_or_default();
        let lower = k.to_ascii_lowercase();
        let hop = ["host", "content-length", "transfer-encoding", "connection", "upgrade", "te", "trailer"];
        if hop.contains(&lower.as_str()) || k.is_empty() || k.len() > 64 || v.len() > 2048 || headers.len() >= MAX_HEADERS {
            return Err(en("A header is not allowed.").into());
        }
        headers.push((k.clone(), v.to_owned()));
    }
    let body: Option<Vec<u8>> = match (req.get("json"), req.get("body").and_then(Value::as_str)) {
        (Some(j), _) => {
            if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type")) {
                headers.push(("content-type".into(), "application/json".into()));
            }
            Some(j.to_string().into_bytes())
        }
        (None, Some(b)) => Some(b.as_bytes().to_vec()),
        (None, None) => None,
    };
    if body.as_ref().is_some_and(|b| b.len() > MAX_VALUE) || (method == reqwest::Method::GET && body.is_some()) {
        return Err(en("The request body is too large, or a GET has one.").into());
    }
    if !allow(plugin, "http", 60, state.settings().int("plugin_http_per_minute"), 1) {
        return Err(en("This plugin has made too many requests in the last minute.").into());
    }
    let sent = tokio::time::timeout(
        super::HTTP_TIMEOUT * 2,
        crate::platform::outbound::request_public(method, url, &headers, body, MAX_HTTP_ANSWER),
    )
    .await;
    match sent {
        Ok(Ok((status, bytes))) => Ok(json!({ "status": status, "body": String::from_utf8_lossy(&bytes) })),
        _ => Err(en("The request failed.").into()),
    }
}

/// The operations that need only the app state. `catalogs` is the plugin's own messages.
pub(super) async fn more(
    state: &AppState,
    plugin: &str,
    catalogs: &Value,
    op: &str,
    req: &Value,
) -> Option<Result<Value, String>> {
    Some(match op {
        "user.get" => user_get(state, req).await,
        "user.can" => user_can(state, plugin, req).await,
        "settings.get" => settings_get(state, plugin, req),
        "email.send" => email_send(state, plugin, catalogs, req).await,
        "realtime.push" => realtime_push(state, plugin, req),
        "schedule" => schedule(state, plugin, req).await,
        "profile.set" => super::registry::profile_set(state, plugin, req).await,
        "group.add" => super::registry::group_member(state, plugin, req, true).await,
        "group.remove" => super::registry::group_member(state, plugin, req, false).await,
        "badge.grant" => super::registry::badge_grant(state, plugin, req).await,
        "content.create_topic" => create_topic(state, plugin, req).await,
        other => return super::ops_content::more(state, plugin, other, req).await,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogs_fall_back_to_the_base_language_then_english() {
        let c = json!({ "en": { "hi": "Hello {name}", "only_en": "E" }, "pl": { "hi": "Czesc {name}" } });
        assert_eq!(catalog_text(&c, "pl-PL", "hi").as_deref(), Some("Czesc {name}"));
        assert_eq!(catalog_text(&c, "ar", "hi").as_deref(), Some("Hello {name}"));
        assert_eq!(catalog_text(&c, "pl", "only_en").as_deref(), Some("E"));
        assert_eq!(catalog_text(&c, "pl", "nope"), None);
    }

    #[test]
    fn placeholders_are_filled() {
        assert_eq!(fill("Hi {name}, {n}", Some(&json!({ "name": "Ann", "n": 3 }))), "Hi Ann, 3");
        assert_eq!(fill("Hi {name}", None), "Hi {name}");
    }
}
