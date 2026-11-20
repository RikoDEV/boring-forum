//! Compiled-in plugins: chat webhooks and Akismet.

use crate::platform::plugins::Plugin;
use crate::state::AppState;
use bf_core::i18n::en;
use bf_core::perms::Perms;
use serde_json::{Value, json};
use std::sync::{Arc, OnceLock};

static STATE: OnceLock<AppState> = OnceLock::new();

/// The webhook's JSON. A zero-width space after `@` and inside `<!` breaks `@everyone`, `@here`,
/// `<@id>` and Slack's `<!channel>`, and `allowed_mentions` tells Discord to ping nobody anyway.
fn webhook_body(line: &str) -> Value {
    let safe = line.replace('@', "@\u{200b}").replace("<!", "<\u{200b}!");
    json!({ "content": safe, "text": safe, "allowed_mentions": { "parse": [] } })
}

/// Posts new topics and replies to a Discord or Slack incoming webhook (`chat_webhook_url`).
struct ChatWebhook;

impl Plugin for ChatWebhook {
    fn id(&self) -> &str {
        "builtin-chat-webhook"
    }
    fn hooks(&self) -> Vec<String> {
        vec!["post.created".into(), "topic.created".into()]
    }
    fn handle(&self, _hook: &str, data: &Value) -> Result<Value, String> {
        let Some(state) = STATE.get() else { return Ok(json!({})) };
        let url = state.settings().str("chat_webhook_url").to_owned();
        // Only what any guest could read goes to a chat room.
        let forum = data["node_id"].as_i64().unwrap_or(0);
        let handle = tokio::runtime::Handle::current();
        let tree = state.pool.as_ref().map(|p| handle.block_on(state.forum.get(p)));
        let guest = bf_core::perms::Actor { groups: vec![1.into()], ..Default::default() };
        let public = matches!(tree, Some(Ok(t)) if t.perms(&guest, forum).has(Perms::READ));
        if !public {
            return Ok(json!({}));
        }
        let line = format!(
            "{} posted in {}: {}",
            data["author"].as_str().unwrap_or("someone"),
            data["title"].as_str().unwrap_or("a topic"),
            data["url"].as_str().unwrap_or_default()
        );
        let topic = data["topic_id"].as_i64().unwrap_or(0);
        handle.block_on(crate::community::chat::integrate(state, forum, topic, _hook == "topic.created", &line));
        if url.is_empty() {
            return Ok(json!({}));
        }
        // Discord reads `content`, Slack reads `text`; each ignores the other. Names and titles
        // are the members' own words, so none of it may ping anyone.
        let body = serde_json::to_vec(&webhook_body(&line)).map_err(|e| e.to_string())?;
        handle.block_on(crate::platform::outbound::post_public(&url, &[("content-type", "application/json")], body))?;
        Ok(json!({}))
    }
}

/// Asks Akismet about new posts (`akismet_key`). Members with trust level 2 or more are not
/// asked, nor is anything refused when Akismet cannot be reached: a spam check must not stop
/// a forum from working.
struct Akismet;

impl Plugin for Akismet {
    fn id(&self) -> &str {
        "builtin-akismet"
    }
    fn hooks(&self) -> Vec<String> {
        vec!["post.before_create".into()]
    }
    fn handle(&self, _hook: &str, data: &Value) -> Result<Value, String> {
        let Some(state) = STATE.get() else { return Ok(json!({})) };
        let key = state.settings().str("akismet_key").to_owned();
        if key.is_empty() {
            return Ok(json!({}));
        }
        let handle = tokio::runtime::Handle::current();
        let Some(pool) = state.pool.as_ref() else { return Ok(json!({})) };
        let author = data["author_id"].as_i64().unwrap_or(0);
        let user = handle.block_on(bf_db::accounts::users::user_by_id(pool, author.into())).ok().flatten();
        if user.as_ref().is_none_or(|u| u.trust_level >= 2 || u.is_admin) {
            return Ok(json!({}));
        }
        // Akismet requires the commenter's address; the member's latest session is the best we have.
        let ip: Option<String> = handle
            .block_on(
                sqlx::query_scalar(
                    "SELECT host(ip) FROM sessions WHERE user_id = $1 AND ip IS NOT NULL ORDER BY last_used_at DESC LIMIT 1",
                )
                .bind(author)
                .fetch_optional(pool),
            )
            .ok()
            .flatten();
        let Some(ip) = ip else { return Ok(json!({})) };
        let form = [
            ("blog", state.config.base_url.as_str()),
            ("user_ip", ip.as_str()),
            ("comment_type", "forum-post"),
            ("comment_content", data["raw"].as_str().unwrap_or_default()),
        ];
        let body = serde_urlencoded_form(&form);
        let Some(url) = akismet_url(&key) else { return Ok(json!({})) };
        let spam = handle.block_on(ask_akismet(&url, body));
        Ok(if spam { json!({ "reject": en("This looks like spam, so it was not posted.") }) } else { json!({}) })
    }
}

/// The same question for a chat message. Never refuses when Akismet cannot be reached.
pub(crate) async fn looks_like_spam(state: &AppState, user: &bf_db::accounts::users::User, text: &str) -> bool {
    let key = state.settings().str("akismet_key").to_owned();
    if key.is_empty() || user.trust_level >= 2 || user.is_admin {
        return false;
    }
    let Some(url) = akismet_url(&key) else { return false };
    let form = [("blog", state.config.base_url.as_str()), ("comment_type", "message"), ("comment_content", text)];
    ask_akismet(&url, serde_urlencoded_form(&form)).await
}

/// Akismet's address for this key. The key becomes part of the host name, so anything but
/// letters and digits is refused: no other characters may choose where the request goes.
fn akismet_url(key: &str) -> Option<String> {
    let key = key.trim();
    if key.is_empty() || key.len() > 64 || !key.bytes().all(|b| b.is_ascii_alphanumeric()) {
        tracing::warn!("the Akismet key is not 1 to 64 letters and digits; the spam check is skipped");
        return None;
    }
    Some(format!("https://{key}.rest.akismet.com/1.1/comment-check"))
}

fn serde_urlencoded_form(pairs: &[(&str, &str)]) -> String {
    url::form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs).finish()
}

/// Through the guarded client (public addresses only, pinned, no redirects), like every other
/// request to an address a setting shapes. The answer is one word, and is never logged.
async fn ask_akismet(url: &str, body: String) -> bool {
    let headers = [("content-type".to_owned(), "application/x-www-form-urlencoded".to_owned())];
    let sent = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        crate::platform::outbound::request_public(reqwest::Method::POST, url, &headers, Some(body.into_bytes()), 1024),
    )
    .await;
    match sent {
        Ok(Ok((status, answer))) => match String::from_utf8_lossy(&answer).trim() {
            "true" => true,
            "false" => false,
            _ => {
                tracing::warn!(status, "Akismet gave no verdict; check the key and settings");
                false
            }
        },
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "Akismet could not be reached");
            false
        }
        Err(_) => {
            tracing::warn!("Akismet did not answer in time");
            false
        }
    }
}

/// Registers the compiled-in plugins. Each does nothing until its setting is filled in.
pub(crate) fn register(state: &AppState) {
    if STATE.set(state.clone()).is_ok() {
        crate::platform::plugins::register_native(Arc::new(ChatWebhook));
        crate::platform::plugins::register_native(Arc::new(Akismet));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webhook_lines_cannot_ping() {
        let body = webhook_body("@everyone posted in <@123> <!channel> @here: x");
        let text = body["content"].as_str().unwrap();
        for ping in ["@everyone", "@here", "<@123>", "<!channel>"] {
            assert!(!text.contains(ping), "{ping} still in {text:?}");
        }
        assert_eq!(body["allowed_mentions"]["parse"], json!([]));
        assert_eq!(body["text"], body["content"]);
    }
}
