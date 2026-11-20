//! Rules that send forum activity to outside chat services.

use crate::admin::api::admin;
use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use bf_core::AppError;
use bf_db::community::chat;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{PROVIDERS, bad_node};

#[derive(Deserialize)]
pub(super) struct RuleBody {
    pub(super) provider: String,
    #[serde(default)]
    pub(super) url: String,
    #[serde(default)]
    pub(super) token: String,
    #[serde(default)]
    pub(super) channel: String,
    #[serde(default = "watch")]
    pub(super) filter: String,
    pub(super) node_id: Option<i64>,
    #[serde(default)]
    pub(super) tag: String,
    #[serde(default = "yes")]
    pub(super) enabled: bool,
}

pub(super) fn watch() -> String {
    "watch".into()
}

pub(super) fn yes() -> bool {
    true
}

impl RuleBody {
    fn into_rule(self, adding: bool) -> Result<chat::Rule, AppError> {
        if !PROVIDERS.contains(&self.provider.as_str()) {
            return Err(AppError::invalid("Provider must be one of {n}.", &[("n", &PROVIDERS.join(", "))]));
        }
        if !matches!(self.filter.as_str(), "follow" | "watch" | "mute") {
            return Err(AppError::Validation("Filter must be follow, watch or mute.".into()));
        }
        let (needs_url, needs_token, needs_channel) = match self.provider.as_str() {
            "telegram" => (false, true, true),
            "matrix" | "zulip" => (true, true, true),
            _ => (true, false, false),
        };
        if adding
            && self.filter != "mute"
            && ((needs_url && self.url.trim().is_empty()) || (needs_token && self.token.trim().is_empty()))
        {
            return Err(AppError::Validation("This provider needs its address and token.".into()));
        }
        if needs_channel && self.filter != "mute" && self.channel.trim().is_empty() {
            return Err(AppError::Validation("This provider needs a channel.".into()));
        }
        Ok(chat::Rule {
            id: 0,
            provider: self.provider,
            url: self.url.trim().to_owned(),
            token: self.token.trim().to_owned(),
            channel: self.channel.trim().to_owned(),
            filter: self.filter,
            node_id: self.node_id,
            tag: self.tag.trim().to_lowercase(),
            enabled: self.enabled,
        })
    }
}

pub(super) async fn list_rules(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, false)?;
    let rules = chat::rules(state.db()?).await.db()?;
    // Webhook addresses are credentials: only their host is shown again.
    let rows: Vec<Value> = rules
        .iter()
        .map(|r| {
            let host = url::Url::parse(&r.url).ok().and_then(|u| u.host_str().map(str::to_owned)).unwrap_or_default();
            json!({ "id": r.id, "provider": r.provider, "host": host, "has_token": !r.token.is_empty(), "channel": r.channel,
                    "filter": r.filter, "node_id": r.node_id, "tag": r.tag, "enabled": r.enabled })
        })
        .collect();
    Ok(Json(json!({ "rules": rows, "providers": PROVIDERS })).into_response())
}

pub(super) async fn add_rule(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Json(b): Json<RuleBody>,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, true)?;
    let id = chat::save_rule(state.db()?, None, &b.into_rule(true)?).await.map_err(bad_node)?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))).into_response())
}

pub(super) async fn update_rule(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(b): Json<RuleBody>,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, true)?;
    match chat::save_rule(state.db()?, Some(id), &b.into_rule(false)?).await {
        Ok(_) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(sqlx::Error::RowNotFound) => Err(AppError::NotFound),
        Err(e) => Err(bad_node(e)),
    }
}

pub(super) async fn delete_rule(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, true)?;
    if !chat::delete_rule(state.db()?, id).await.db()? {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub(super) async fn test_rule(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, true)?;
    let rules = chat::rules(state.db()?).await.db()?;
    let rule = rules.iter().find(|r| r.id == id).ok_or(AppError::NotFound)?;
    let line = format!("Test message from {}", state.settings().str("site_name"));
    match deliver(rule, &line).await {
        Ok(()) => Ok(Json(json!({ "ok": true })).into_response()),
        Err(e) => Err(AppError::invalid("The service refused it: {e}", &[("e", &e)])),
    }
}

/// Sends one line to a rule's service. Every address goes through the same check as webhooks, so
/// a rule cannot reach this server's own network.
pub(crate) async fn deliver(r: &chat::Rule, line: &str) -> Result<(), String> {
    const JSON_TYPE: (&str, &str) = ("content-type", "application/json");
    let base = r.url.trim_end_matches('/');
    let encode = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
    let status = match r.provider.as_str() {
        "discord" => post_json(&r.url, &[JSON_TYPE], json!({ "content": line })).await?,
        "telegram" => {
            let to = format!("https://api.telegram.org/bot{}/sendMessage", r.token);
            post_json(&to, &[JSON_TYPE], json!({ "chat_id": r.channel, "text": line })).await?
        }
        "matrix" => {
            let to = format!("{base}/_matrix/client/v3/rooms/{}/send/m.room.message", encode(&r.channel));
            let auth = format!("Bearer {}", r.token);
            post_json(&to, &[JSON_TYPE, ("authorization", &auth)], json!({ "msgtype": "m.text", "body": line })).await?
        }
        "zulip" => {
            use base64::Engine as _;
            // `token` is bot-email:api-key, `channel` is stream or stream/topic.
            let (stream, topic) = r.channel.split_once('/').unwrap_or((&r.channel, "forum"));
            let auth = format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(&r.token));
            let form = format!("type=stream&to={}&topic={}&content={}", encode(stream), encode(topic), encode(line));
            crate::platform::outbound::post_public(
                &format!("{base}/api/v1/messages"),
                &[("content-type", "application/x-www-form-urlencoded"), ("authorization", &auth)],
                form.into_bytes(),
            )
            .await?
        }
        "webex" => post_json(&r.url, &[JSON_TYPE], json!({ "markdown": line })).await?,
        // Slack, Mattermost, Rocket.Chat, Teams and Google Chat all read `text`.
        _ => post_json(&r.url, &[JSON_TYPE], json!({ "text": line })).await?,
    };
    if (200..300).contains(&status) { Ok(()) } else { Err(format!("status {status}")) }
}

pub(super) async fn post_json(to: &str, headers: &[(&str, &str)], body: Value) -> Result<u16, String> {
    crate::platform::outbound::post_public(to, headers, body.to_string().into_bytes()).await
}

/// A rule's scope covers this topic.
pub(super) fn matches(r: &chat::Rule, node: i64, tags: &[String]) -> bool {
    r.enabled && r.node_id.is_none_or(|n| n == node) && (r.tag.is_empty() || tags.contains(&r.tag))
}

/// Which rules fire: `follow` for new topics only, `watch` for every post. A matching `mute`
/// silences that provider for this topic.
pub(crate) fn firing<'a>(rules: &'a [chat::Rule], node: i64, tags: &[String], new_topic: bool) -> Vec<&'a chat::Rule> {
    let hit: Vec<&chat::Rule> = rules.iter().filter(|r| matches(r, node, tags)).collect();
    let muted: Vec<&str> = hit.iter().filter(|r| r.filter == "mute").map(|r| r.provider.as_str()).collect();
    hit.into_iter()
        .filter(|r| {
            !muted.contains(&r.provider.as_str()) && (r.filter == "watch" || (r.filter == "follow" && new_topic))
        })
        .collect()
}

/// Sends a public post to every matching rule. Failures go to the admin dashboard.
pub(crate) async fn integrate(state: &AppState, node: i64, topic: i64, new_topic: bool, line: &str) {
    let Some(pool) = &state.pool else { return };
    let Ok(rules) = chat::rules(pool).await else { return };
    if rules.is_empty() {
        return;
    }
    let tags = chat::topic_tags(pool, topic).await.unwrap_or_default();
    for r in firing(&rules, node, &tags, new_topic) {
        if let Err(e) = deliver(r, line).await {
            tracing::warn!(rule = r.id, error = %e, "chat integration failed");
            let text = format!("Chat integration rule {} ({}) failed: {e}", r.id, r.provider);
            let _ = bf_db::admin::notices::add_notice(pool, "chat", &text).await;
        }
    }
}

/// From the maintenance tick.
pub(crate) async fn run_retention(state: &AppState) {
    let Some(pool) = &state.pool else { return };
    let s = state.settings();
    if let Err(e) = chat::purge(pool, s.int("chat_retention_days") as i32, s.int("chat_dm_retention_days") as i32).await
    {
        tracing::warn!(error = %e, "chat retention failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(filter: &str, provider: &str, node: Option<i64>, tag: &str) -> chat::Rule {
        chat::Rule {
            id: 0,
            provider: provider.into(),
            url: String::new(),
            token: String::new(),
            channel: String::new(),
            filter: filter.into(),
            node_id: node,
            tag: tag.into(),
            enabled: true,
        }
    }

    #[test]
    fn follow_is_for_new_topics_and_watch_for_every_post() {
        let rules = [rule("follow", "slack", None, ""), rule("watch", "discord", None, "")];
        assert_eq!(firing(&rules, 1, &[], true).len(), 2);
        assert_eq!(firing(&rules, 1, &[], false)[0].provider, "discord");
    }

    #[test]
    fn scope_and_mute() {
        let rules = [
            rule("watch", "slack", Some(2), ""),
            rule("watch", "discord", None, "news"),
            rule("watch", "teams", None, ""),
            rule("mute", "teams", Some(3), ""),
        ];
        assert_eq!(
            firing(&rules, 2, &[], false).iter().map(|r| r.provider.as_str()).collect::<Vec<_>>(),
            ["slack", "teams"]
        );
        assert_eq!(
            firing(&rules, 9, &["news".to_owned()], false).iter().map(|r| r.provider.as_str()).collect::<Vec<_>>(),
            ["discord", "teams"]
        );
        assert!(firing(&rules, 3, &[], false).is_empty());
    }
}
