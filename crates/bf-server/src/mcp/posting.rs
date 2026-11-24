//! Posting through MCP: start a topic, reply to one.
//!
//! The forum's own forms do the work (`/c/{id}/new`, `/t/{id}/reply`), as a member's browser would
//! submit them: the member's key is passed on, so the forum's rules apply as they do by hand, among
//! them what the member may post where, the daily and new-member limits, spam screening, a topic
//! being locked, and the queue for a first post. A key with the `read` scope cannot post; one limited
//! to resources needs `write:topics` or `write:posts`. Off unless `mcp_posting_enabled` is on.

use super::{Ctx, Payload, failed, forward};
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Method, header};
use serde_json::{Map, Value, json};
use std::net::SocketAddr;

const POSTING_TOOLS: [&str; 2] = ["create_topic", "reply_to_topic"];

pub(super) fn is_posting_tool(name: &str) -> bool {
    POSTING_TOOLS.contains(&name)
}

/// Whether these tools are listed: the site allows them and the caller sent a key. Whether the key
/// may post is the forum's to say when it is used.
pub(super) fn offered(ctx: &Ctx, headers: &HeaderMap) -> bool {
    ctx.state.settings().bool("mcp_posting_enabled") && headers.contains_key(header::AUTHORIZATION)
}

pub(super) fn tools() -> Vec<Value> {
    let body = json!({ "type": "string", "description": "The text, in the forum's markup (Markdown)." });
    vec![
        json!({
            "name": "create_topic",
            "title": "Start a topic",
            "description": "Start a new topic in a forum, as the member who owns the API key (it needs the `write` scope). It is public once posted, unless the forum holds a member's first posts for approval. Check the forum's rules and its existing topics first (search, get_topic).",
            "inputSchema": { "type": "object", "properties": {
                "forum_id": { "type": "integer", "description": "The forum's id, from list_forums." },
                "title": { "type": "string" },
                "body": body,
                "tags": { "type": "string", "description": "Comma-separated tags. Optional." },
                "prefix": { "type": "string", "description": "A topic prefix the forum offers. Optional." },
            }, "required": ["forum_id", "title", "body"], "additionalProperties": false },
            "annotations": { "title": "Start a topic", "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false },
        }),
        json!({
            "name": "reply_to_topic",
            "title": "Reply to a topic",
            "description": "Post a reply in a topic, as the member who owns the API key (it needs the `write` scope). Read the topic first with get_topic so the reply fits. Posting twice posts twice.",
            "inputSchema": { "type": "object", "properties": {
                "topic_id": { "type": "integer", "description": "The topic's id." },
                "body": body,
                "reply_to": { "type": "integer", "description": "Number of the post in this topic that the reply answers. Optional." },
            }, "required": ["topic_id", "body"], "additionalProperties": false },
            "annotations": { "title": "Reply to a topic", "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false },
        }),
    ]
}

fn id(args: &Map<String, Value>, key: &str) -> Result<i64, String> {
    args.get(key)
        .and_then(Value::as_i64)
        .filter(|n| *n > 0)
        .ok_or_else(|| format!("`{key}` is required and must be a positive whole number."))
}

fn text<'a>(args: &'a Map<String, Value>, key: &str, required: bool) -> Result<&'a str, String> {
    match args.get(key) {
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(s),
        None | Some(Value::Null) if !required => Ok(""),
        Some(Value::String(_)) | None | Some(Value::Null) => Err(format!("`{key}` is required and must not be empty.")),
        Some(_) => Err(format!("`{key}` must be a string.")),
    }
}

/// The form a page submits. The token is for the form's own check, which a key skips.
fn form(fields: &[(&str, &str)]) -> String {
    let mut out = url::form_urlencoded::Serializer::new(String::new());
    out.append_pair("csrf", "mcp");
    for (k, v) in fields.iter().filter(|(_, v)| !v.is_empty()) {
        out.append_pair(k, v);
    }
    out.finish()
}

/// The request for a call: where to post and what.
fn request(name: &str, args: &Map<String, Value>) -> Result<(String, String), String> {
    if let Some(unknown) = args.keys().find(|k| {
        let known: &[&str] = if name == "create_topic" {
            &["forum_id", "title", "body", "tags", "prefix"]
        } else {
            &["topic_id", "body", "reply_to"]
        };
        !known.contains(&k.as_str())
    }) {
        return Err(format!("Unknown argument `{unknown}`."));
    }
    if name == "create_topic" {
        let forum = id(args, "forum_id")?;
        let fields = [
            ("title", text(args, "title", true)?),
            ("body", text(args, "body", true)?),
            ("tags", text(args, "tags", false)?),
            ("prefix", text(args, "prefix", false)?),
        ];
        return Ok((format!("/c/{forum}/new"), form(&fields)));
    }
    let topic = id(args, "topic_id")?;
    let reply_to = match args.get("reply_to") {
        None | Some(Value::Null) => String::new(),
        Some(_) => id(args, "reply_to")?.to_string(),
    };
    let fields = [("body", text(args, "body", true)?), ("reply_to", reply_to.as_str())];
    Ok((format!("/t/{topic}/reply"), form(&fields)))
}

pub(super) async fn call(
    ctx: &Ctx,
    headers: &HeaderMap,
    peer: &Option<ConnectInfo<SocketAddr>>,
    name: &str,
    args: &Map<String, Value>,
) -> Value {
    if !ctx.state.settings().bool("mcp_posting_enabled") {
        return failed("Posting through MCP is off. An administrator turns it on in Admin, Settings, MCP server.");
    }
    if !headers.contains_key(header::AUTHORIZATION) {
        return failed("Posting needs an API key with the `write` scope: send `Authorization: Bearer <key>`.");
    }
    let (path, body) = match request(name, args) {
        Ok(r) => r,
        Err(e) => return failed(e),
    };
    let answer = match forward(ctx, headers, peer, Method::POST, &path, Payload::Form(body), None).await {
        Ok(a) => a,
        Err(e) => return failed(e),
    };
    if answer.status.is_redirection() {
        let at = answer.location.unwrap_or_default();
        return json!({ "content": [{ "type": "text", "text": format!("Posted: {at}") }] });
    }
    if answer.status.is_success() {
        // The forum answered with a page, not a redirect: the post is waiting for a moderator.
        return json!({ "content": [{ "type": "text", "text": "Submitted. It is waiting for a moderator to approve it." }] });
    }
    super::finish(answer.status, answer.text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: Value) -> Map<String, Value> {
        match v {
            Value::Object(m) => m,
            other => panic!("not an object: {other}"),
        }
    }

    #[test]
    fn a_reply_is_the_form_a_page_submits() {
        let (path, body) =
            request("reply_to_topic", &args(json!({ "topic_id": 7, "body": "a & b", "reply_to": 2 }))).expect("ok");
        assert_eq!(path, "/t/7/reply");
        assert_eq!(body, "csrf=mcp&body=a+%26+b&reply_to=2");
    }

    #[test]
    fn a_topic_is_posted_to_its_forum() {
        let (path, body) =
            request("create_topic", &args(json!({ "forum_id": 3, "title": "T", "body": "B", "tags": "x,y" })))
                .expect("ok");
        assert_eq!(path, "/c/3/new");
        assert_eq!(body, "csrf=mcp&title=T&body=B&tags=x%2Cy");
    }

    #[test]
    fn bad_arguments_are_refused() {
        assert!(request("reply_to_topic", &args(json!({ "topic_id": 0, "body": "x" }))).is_err());
        assert!(request("reply_to_topic", &args(json!({ "topic_id": 1, "body": "  " }))).is_err());
        assert!(request("reply_to_topic", &args(json!({ "topic_id": 1, "body": "x", "extra": 1 }))).is_err());
        assert!(request("create_topic", &args(json!({ "forum_id": 1, "title": "t" }))).is_err());
        assert!(request("create_topic", &args(json!({ "forum_id": "1", "title": "t", "body": "b" }))).is_err());
    }
}
