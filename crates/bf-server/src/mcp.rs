//! A Model Context Protocol server at `POST /mcp` (Streamable HTTP, stateless, one JSON answer per
//! request), so an assistant can read the forum.
//!
//! Every tool is one `GET` of the JSON API, run in-process through the whole router. Nothing is
//! re-implemented: what a caller may read, an API key's scope and resources, the rate limit and the
//! private-forum rules are the API's own. A request without a key reads as a guest; one with
//! `Authorization: Bearer bf_…` reads as that member. Cookies are never passed on, so a browser
//! session cannot act through this endpoint.
//!
//! The member tools only read: the API has no JSON route for writing posts. An administrator's
//! `admin` key also gets the tools of `admin` (the admin API, with the dangerous routes held back),
//! when the `mcp_admin_enabled` setting is on. A member's key with the `write` scope can also post
//! (`posting`), when `mcp_posting_enabled` is on: through the forum's own forms, so every rule,
//! limit and approval queue of a post made by hand applies.

mod admin;
mod catalog;
mod posting;

use crate::state::AppState;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde_json::{Map, Value, json};
use std::net::SocketAddr;
use tower::ServiceExt;

/// Newest first: the one a client asks for is answered when it is listed, else the newest.
const VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// A request larger than this is refused unread.
const MAX_REQUEST: usize = 256 * 1024;

/// The most an API answer may weigh before it is refused, and the most text one tool returns.
const MAX_ANSWER: usize = 4 * 1024 * 1024;
const MAX_TEXT: usize = 120_000;

#[derive(Clone, Copy)]
enum Kind {
    Int,
    Text,
}

#[derive(Clone, Copy)]
struct Param {
    name: &'static str,
    kind: Kind,
    required: bool,
    /// In the path (replacing `{name}`) rather than the query string.
    in_path: bool,
    help: &'static str,
}

const fn path(name: &'static str, kind: Kind, help: &'static str) -> Param {
    Param { name, kind, required: true, in_path: true, help }
}

const fn query(name: &'static str, kind: Kind, required: bool, help: &'static str) -> Param {
    Param { name, kind, required, in_path: false, help }
}

struct Tool {
    name: &'static str,
    title: &'static str,
    help: &'static str,
    route: &'static str,
    params: &'static [Param],
}

const TOOLS: &[Tool] = &[
    Tool {
        name: "whoami",
        title: "Who am I",
        help: "The member the API key belongs to and what the key may do. Needs an API key; refused without one.",
        route: "/api/v1/me",
        params: &[],
    },
    Tool {
        name: "list_forums",
        title: "List forums",
        help: "Every forum and category the caller may read, flat and in index order. `parent_id` builds the tree; each has an `id`, a title, counts and the time of the last activity.",
        route: "/api/v1/forums",
        params: &[],
    },
    Tool {
        name: "list_topics",
        title: "List a forum's topics",
        help: "Topics of one forum, newest activity first, 30 to a page.",
        route: "/api/v1/forums/{forum_id}/topics",
        params: &[
            path("forum_id", Kind::Int, "The forum's id, from list_forums."),
            query("page", Kind::Int, false, "Zero-based page. Default 0."),
        ],
    },
    Tool {
        name: "get_topic",
        title: "Read a topic",
        help: "A topic and a window of its posts, in order. Page through a long topic with `from` and `count`.",
        route: "/api/v1/topics/{topic_id}",
        params: &[
            path("topic_id", Kind::Int, "The topic's id."),
            query("from", Kind::Int, false, "Number of the first post to return. Default 1."),
            query("count", Kind::Int, false, "How many posts, at most 50."),
        ],
    },
    Tool {
        name: "get_post",
        title: "Read a post",
        help: "One post by its id, if the caller may read its topic.",
        route: "/api/v1/posts/{post_id}",
        params: &[path("post_id", Kind::Int, "The post's id (not its number in the topic).")],
    },
    Tool {
        name: "search",
        title: "Search the forum",
        help: "Matching forums, tags and members first, then posts with a snippet. Narrow it with the optional filters.",
        route: "/api/v1/search",
        params: &[
            query("q", Kind::Text, true, "What to search for."),
            query("author", Kind::Text, false, "Only posts by this username."),
            query("forum", Kind::Text, false, "Only this forum."),
            query("in", Kind::Text, false, "Set to `topics` to search titles only."),
            query("topic", Kind::Int, false, "Only posts of this topic id."),
        ],
    },
    Tool {
        name: "get_user",
        title: "Read a member's profile",
        help: "A member's public profile, with the visibility rules of their profile page.",
        route: "/api/v1/users/{username}",
        params: &[path("username", Kind::Text, "The member's username.")],
    },
    Tool {
        name: "user_activity",
        title: "A member's recent activity",
        help: "A member's latest posts or topics.",
        route: "/api/v1/users/{username}/activity",
        params: &[
            path("username", Kind::Text, "The member's username."),
            query("kind", Kind::Text, false, "`posts` (default) or `topics`."),
            query("count", Kind::Int, false, "How many items."),
        ],
    },
    Tool {
        name: "related_topics",
        title: "Topics related to a topic",
        help: "Topics close in meaning to one topic. Empty unless the forum has AI embeddings on.",
        route: "/api/v1/topics/{topic_id}/related",
        params: &[path("topic_id", Kind::Int, "The topic's id.")],
    },
];

fn schema(tool: &Tool) -> Value {
    let mut props = Map::new();
    let mut required = Vec::new();
    for p in tool.params {
        let ty = if matches!(p.kind, Kind::Int) { "integer" } else { "string" };
        props.insert(p.name.into(), json!({ "type": ty, "description": p.help }));
        if p.required {
            required.push(p.name);
        }
    }
    json!({ "type": "object", "properties": props, "required": required, "additionalProperties": false })
}

fn tool_list(admin: bool, posting: bool) -> Value {
    let tools: Vec<Value> = TOOLS
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "title": t.title,
                "description": t.help,
                "inputSchema": schema(t),
                "annotations": { "title": t.title, "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false },
            })
        })
        .chain(admin.then(admin::tools).into_iter().flatten())
        .chain(posting.then(posting::tools).into_iter().flatten())
        .collect();
    json!({ "tools": tools })
}

/// Percent-encodes everything outside the RFC 3986 unreserved set.
fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The API address a call asks for, or what is wrong with its arguments.
fn request_path(tool: &Tool, args: &Map<String, Value>) -> Result<String, String> {
    let mut route = tool.route.to_owned();
    let mut qs = Vec::new();
    for p in tool.params {
        let text = match args.get(p.name) {
            None | Some(Value::Null) if p.required => return Err(format!("`{}` is required.", p.name)),
            None | Some(Value::Null) => continue,
            Some(Value::Number(n)) if matches!(p.kind, Kind::Int) => {
                n.as_i64().ok_or_else(|| format!("`{}` must be a whole number.", p.name))?.to_string()
            }
            Some(Value::String(s)) if matches!(p.kind, Kind::Text) => {
                if s.trim().is_empty() && p.required {
                    return Err(format!("`{}` must not be empty.", p.name));
                }
                s.clone()
            }
            Some(_) => {
                let want = if matches!(p.kind, Kind::Int) { "a whole number" } else { "a string" };
                return Err(format!("`{}` must be {want}.", p.name));
            }
        };
        if p.in_path {
            route = route.replace(&format!("{{{}}}", p.name), &enc(&text));
        } else {
            qs.push(format!("{}={}", p.name, enc(&text)));
        }
    }
    if let Some(unknown) = args.keys().find(|k| !tool.params.iter().any(|p| p.name == k.as_str())) {
        return Err(format!("Unknown argument `{unknown}`."));
    }
    if !qs.is_empty() {
        route.push('?');
        route.push_str(&qs.join("&"));
    }
    Ok(route)
}

#[derive(Clone)]
struct Ctx {
    state: AppState,
    /// The site's router with all its gates, without this endpoint.
    core: Router,
}

/// Adds `POST /mcp` to the site's router.
pub fn mount(core: Router, state: AppState) -> Router {
    let limiter = state.rate_limiter.clone();
    let ctx = Ctx { state: state.clone(), core: core.clone() };
    let endpoint = Router::new()
        .route("/mcp", post(handle))
        .with_state(ctx)
        // The same gates as the rest of the site: a plain-HTTP request is turned away before it
        // can carry a key, and a client has one request budget.
        .layer(middleware::from_fn_with_state(limiter, crate::http::ratelimit::layer))
        .layer(middleware::from_fn_with_state(state, crate::http::middleware::https_redirect));
    core.merge(endpoint)
}

fn json_response(status: StatusCode, body: &Value) -> Response {
    let mut res = (status, body.to_string()).into_response();
    res.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

fn plain(status: StatusCode) -> Response {
    let mut res = status.into_response();
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

/// A page on another origin may not drive this endpoint (DNS rebinding): when the browser names an
/// origin, it must be this site's own.
fn origin_ok(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else { return true };
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or_default();
    origin.split_once("://").is_some_and(|(_, authority)| authority.eq_ignore_ascii_case(host))
}

async fn handle(State(ctx): State<Ctx>, req: Request) -> Response {
    if !ctx.state.settings().bool("mcp_enabled") {
        return plain(StatusCode::NOT_FOUND);
    }
    if !origin_ok(req.headers()) {
        return plain(StatusCode::FORBIDDEN);
    }
    let (parts, body) = req.into_parts();
    let Ok(bytes) = to_bytes(body, MAX_REQUEST).await else {
        return plain(StatusCode::PAYLOAD_TOO_LARGE);
    };
    let peer = parts.extensions.get::<ConnectInfo<SocketAddr>>().cloned();
    let Ok(msg) = serde_json::from_slice::<Value>(&bytes) else {
        return json_response(StatusCode::BAD_REQUEST, &failure(&Value::Null, -32700, "Parse error."));
    };
    // JSON-RPC allows a batch; the newest protocol revision dropped it, older clients still send it.
    let answer = match msg {
        Value::Array(items) if items.is_empty() => Some(failure(&Value::Null, -32600, "Empty batch.")),
        Value::Array(items) => {
            let mut out = Vec::new();
            for item in items {
                out.extend(rpc(&ctx, &parts.headers, &peer, item).await);
            }
            (!out.is_empty()).then_some(Value::Array(out))
        }
        item => rpc(&ctx, &parts.headers, &peer, item).await,
    };
    match answer {
        Some(body) => json_response(StatusCode::OK, &body),
        // Only notifications and responses came in: nothing to answer.
        None => plain(StatusCode::ACCEPTED),
    }
}

fn success(id: &Value, result: Value) -> Value {
    let mut answer = json!({ "jsonrpc": "2.0", "id": id });
    answer["result"] = result;
    answer
}

fn failure(id: &Value, code: i32, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// One JSON-RPC message. A notification or a response gets no answer.
async fn rpc(ctx: &Ctx, headers: &HeaderMap, peer: &Option<ConnectInfo<SocketAddr>>, msg: Value) -> Option<Value> {
    let Some(obj) = msg.as_object() else {
        return Some(failure(&Value::Null, -32600, "A request is a JSON object."));
    };
    let Some(method) = obj.get("method").and_then(Value::as_str) else {
        // A response to something this server never asked.
        return None;
    };
    let id = obj.get("id").filter(|i| i.is_string() || i.is_number())?;
    let params = obj.get("params").and_then(Value::as_object).cloned().unwrap_or_default();
    Some(match method {
        "initialize" => success(id, initialize(ctx, &params)),
        "ping" => success(id, json!({})),
        "tools/list" => {
            let admin = admin::caller(ctx, headers, peer).await.is_some();
            success(id, tool_list(admin, posting::offered(ctx, headers)))
        }
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(failure(id, -32602, "`name` is required."));
            };
            let args = params.get("arguments").and_then(Value::as_object).cloned().unwrap_or_default();
            if posting::is_posting_tool(name) {
                return Some(success(id, posting::call(ctx, headers, peer, name, &args).await));
            }
            if admin::is_admin_tool(name) {
                return Some(success(id, admin::call(ctx, headers, peer, name, &args).await));
            }
            let Some(tool) = TOOLS.iter().find(|t| t.name == name) else {
                return Some(failure(id, -32602, &format!("Unknown tool `{name}`.")));
            };
            success(id, call(ctx, headers, peer, tool, &args).await)
        }
        _ => failure(id, -32601, "Method not found."),
    })
}

fn initialize(ctx: &Ctx, params: &Map<String, Value>) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or_default();
    let version = VERSIONS.iter().find(|v| **v == asked).unwrap_or(&VERSIONS[0]);
    let settings = ctx.state.settings();
    let site = settings.str("site_name");
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "boring-forum", "title": site, "version": env!("CARGO_PKG_VERSION") },
        "instructions": format!(
            "Access to the {site} forum. Start with list_forums or search; read a topic with get_topic. \
             Without an API key you read what a guest can; send `Authorization: Bearer <key>` to read as a member. \
             An administrator's `admin` API key also gets admin_routes, admin_get and admin_write when the site \
             allows it: find a route with admin_routes, read it with admin_get (a write usually takes the shape a \
             read returns), then admin_write; every write is in the audit log. A member's `write` key can post with \
             create_topic and reply_to_topic when the site allows it."
        ),
    })
}

/// Headers that describe the MCP request itself, or would let more than a key through, are not
/// passed to the API.
const NOT_FORWARDED: [HeaderName; 12] = [
    header::COOKIE,
    header::CONTENT_LENGTH,
    header::CONTENT_TYPE,
    header::CONTENT_ENCODING,
    header::ACCEPT,
    header::ACCEPT_ENCODING,
    header::IF_NONE_MATCH,
    header::IF_MODIFIED_SINCE,
    header::ORIGIN,
    header::RANGE,
    header::TRANSFER_ENCODING,
    header::CONNECTION,
];

/// What a request to the site carries.
enum Payload {
    None,
    Json(Vec<u8>),
    /// An HTML form, as the forum's own pages send it.
    Form(String),
}

/// The site's answer, as far as a tool needs it.
struct Answer {
    status: StatusCode,
    /// Where a redirect after a post goes: the new topic or post.
    location: Option<String>,
    text: String,
}

/// One request to the site's own router, as the caller: its key and language, never its cookies.
async fn forward(
    ctx: &Ctx,
    headers: &HeaderMap,
    peer: &Option<ConnectInfo<SocketAddr>>,
    method: Method,
    uri: &str,
    payload: Payload,
    idempotency_key: Option<&str>,
) -> Result<Answer, String> {
    let (content_type, body) = match payload {
        Payload::None => (None, Body::empty()),
        Payload::Json(b) => (Some("application/json"), Body::from(b)),
        Payload::Form(f) => (Some("application/x-www-form-urlencoded"), Body::from(f)),
    };
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .body(body)
        .map_err(|_| "That request could not be built.".to_owned())?;
    for (name, value) in headers {
        if !NOT_FORWARDED.contains(name) && name != "idempotency-key" {
            req.headers_mut().append(name.clone(), value.clone());
        }
    }
    req.headers_mut().insert(header::ACCEPT, HeaderValue::from_static("application/json"));
    if let Some(ct) = content_type {
        req.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(ct));
    }
    if let Some(k) = idempotency_key.and_then(|k| HeaderValue::from_str(k).ok()) {
        req.headers_mut().insert("idempotency-key", k);
    }
    if let Some(p) = peer {
        req.extensions_mut().insert(*p);
    }
    let Ok(res) = ctx.core.clone().oneshot(req).await;
    let status = res.status();
    let location = res.headers().get(header::LOCATION).and_then(|v| v.to_str().ok()).map(str::to_owned);
    let body = to_bytes(res.into_body(), MAX_ANSWER).await.map_err(|_| "The answer is too large.".to_owned())?;
    Ok(Answer { status, location, text: String::from_utf8_lossy(&body).into_owned() })
}

/// A tool's result: the API's answer as text, or its refusal as a tool error.
fn finish(status: StatusCode, text: String) -> Value {
    if !status.is_success() {
        // A problem document names what is wrong; a body the framework refused says so in plain text.
        let message = match serde_json::from_str::<Value>(&text) {
            Ok(p) => {
                let field = |k: &str| p.get(k).and_then(Value::as_str).map(str::to_owned);
                field("detail").or_else(|| field("title")).unwrap_or_default()
            }
            Err(_) if !text.contains('<') => text.trim().chars().take(300).collect(),
            Err(_) => String::new(),
        };
        let message =
            if message.is_empty() { status.canonical_reason().unwrap_or("Request failed").to_owned() } else { message };
        return failed(format!("{} {message}", status.as_u16()));
    }
    let mut out = if text.is_empty() { format!("Done ({}).", status.as_u16()) } else { text };
    if let Some((cut, _)) = out.char_indices().nth(MAX_TEXT) {
        out.truncate(cut);
        out.push_str(
            "
… (cut: ask for less, or a later page or `from`)",
        );
    }
    json!({ "content": [{ "type": "text", "text": out }] })
}

fn failed(text: impl Into<String>) -> Value {
    json!({ "isError": true, "content": [{ "type": "text", "text": text.into() }] })
}

async fn call(
    ctx: &Ctx,
    headers: &HeaderMap,
    peer: &Option<ConnectInfo<SocketAddr>>,
    tool: &Tool,
    args: &Map<String, Value>,
) -> Value {
    let uri = match request_path(tool, args) {
        Ok(p) => p,
        Err(e) => return failed(e),
    };
    match forward(ctx, headers, peer, Method::GET, &uri, Payload::None, None).await {
        Ok(a) => finish(a.status, a.text),
        Err(e) => failed(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str) -> &'static Tool {
        TOOLS.iter().find(|t| t.name == name).expect("tool")
    }

    fn args(v: Value) -> Map<String, Value> {
        match v {
            Value::Object(m) => m,
            other => panic!("not an object: {other}"),
        }
    }

    #[test]
    fn every_tool_reads_one_api_route_and_has_an_object_schema() {
        let mut names = std::collections::HashSet::new();
        for t in TOOLS {
            assert!(names.insert(t.name), "duplicate tool {}", t.name);
            assert!(t.route.starts_with("/api/v1/"));
            assert_eq!(schema(t)["type"], "object");
            for p in t.params.iter().filter(|p| p.in_path) {
                assert!(t.route.contains(&format!("{{{}}}", p.name)), "{} not in {}", p.name, t.route);
            }
        }
        assert_eq!(tool_list(false, false)["tools"].as_array().map(Vec::len), Some(TOOLS.len()));
    }

    #[test]
    fn builds_the_path_and_query() {
        let p = request_path(tool("list_topics"), &args(json!({ "forum_id": 7, "page": 2 })));
        assert_eq!(p.as_deref(), Ok("/api/v1/forums/7/topics?page=2"));
        let p = request_path(tool("search"), &args(json!({ "q": "a b&c", "topic": 3 })));
        assert_eq!(p.as_deref(), Ok("/api/v1/search?q=a%20b%26c&topic=3"));
    }

    #[test]
    fn a_path_value_cannot_leave_its_segment() {
        let p = request_path(tool("get_user"), &args(json!({ "username": "../admin/x?y#z" })));
        assert_eq!(p.as_deref(), Ok("/api/v1/users/..%2Fadmin%2Fx%3Fy%23z"));
    }

    #[test]
    fn refuses_bad_arguments() {
        assert!(request_path(tool("get_topic"), &args(json!({}))).is_err());
        assert!(request_path(tool("get_topic"), &args(json!({ "topic_id": "1" }))).is_err());
        assert!(request_path(tool("get_topic"), &args(json!({ "topic_id": 1.5 }))).is_err());
        assert!(request_path(tool("get_topic"), &args(json!({ "topic_id": 1, "admin": true }))).is_err());
        assert!(request_path(tool("search"), &args(json!({ "q": "  " }))).is_err());
    }

    #[test]
    fn origin_must_be_the_sites_own() {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, HeaderValue::from_static("forum.example.com"));
        assert!(origin_ok(&h));
        h.insert(header::ORIGIN, HeaderValue::from_static("https://forum.example.com"));
        assert!(origin_ok(&h));
        h.insert(header::ORIGIN, HeaderValue::from_static("https://evil.example"));
        assert!(!origin_ok(&h));
    }
}
