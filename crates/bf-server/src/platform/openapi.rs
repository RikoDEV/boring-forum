//! OpenAPI document. `boring-forum openapi` prints it; `web/` turns it into a TS client
//! (`pnpm openapi` in `web/`, which CI re-runs and diffs against `web/api/src/schema.d.ts`).
//!
//! Member routes are listed here with the handler that serves them; adding a route to the API
//! means annotating its handler with `#[utoipa::path]` and naming it below, or adding a row to
//! `ROUTES` (method, path, tag, summary, who may call, body) for a route with no annotation yet.
//! The test at the bottom fails when a route registered under `/api/v1` is in neither.
//! `Documented` also adds what every route shares: the key scope it needs, `Idempotency-Key` on
//! writes and `ETag`/`If-None-Match` on reads.

use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Boring Forum API",
        version = "1",
        description = "Member and admin JSON API. Authenticate with a session cookie (writes also send `X-CSRF-Token`) or an API key as a bearer token. A key has a base scope (read, write, admin) and may be limited to resources (`read:topics`, `write:posts`) and to plugin scopes. Writes accept an `Idempotency-Key` header."
    ),
    paths(
        crate::admin::settings::public_settings,
        crate::admin::settings::admin_settings,
        crate::admin::settings::update_setting,
        crate::forum::api::forum_tree,
        crate::forum::api::topic_detail,
        crate::forum::api::post_detail,
        crate::forum::api::edit_post,
        crate::forum::api::delete_post,
        crate::forum::api::react_post,
        crate::forum::api::flag_post,
        crate::forum::api::move_topic,
        crate::forum::api::user_profile,
        crate::forum::api::user_activity,
        crate::forum::search::search_api,
        crate::community::notifications::recent_notifications,
        crate::community::messages::recent,
        crate::forum::bookmarks::recent_bookmarks,
        crate::platform::api_keys::me,
        crate::platform::api_keys::list_keys,
        crate::platform::api_keys::create_key,
        crate::platform::api_keys::delete_key,
        crate::platform::webhooks::test_webhook,
        crate::platform::webhooks::replay_delivery,
        crate::admin::maintenance::site_export,
    ),
    components(schemas(
        bf_core::error::Problem,
        crate::admin::settings::AdminSetting,
        crate::admin::settings::UpdateSetting,
        crate::forum::api::ForumNode,
        crate::forum::api::PostJson,
        crate::forum::api::TopicDetail,
        crate::forum::api::UserJson,
        crate::forum::api::ActivityItem,
        crate::forum::api::EditPost,
        crate::forum::api::Done,
        crate::forum::api::React,
        crate::forum::api::Flag,
        crate::forum::api::Move,
        crate::platform::api_keys::NewKey,
        crate::platform::api_keys::CreatedKey,
        crate::platform::webhooks::TestResult,
    )),
    modifiers(&AdminToken, &Documented)
)]
struct ApiDoc;

struct AdminToken;

impl Modify for AdminToken {
    fn modify(&self, api: &mut utoipa::openapi::OpenApi) {
        if let Some(c) = api.components.as_mut() {
            c.add_security_scheme(
                "admin_token",
                SecurityScheme::Http(HttpBuilder::new().scheme(HttpAuthScheme::Bearer).build()),
            );
        }
    }
}

pub fn spec() -> utoipa::openapi::OpenApi {
    ApiDoc::openapi()
}

#[expect(clippy::expect_used, reason = "the document is built from static derives; serde_json cannot fail on it")]
pub fn spec_json() -> String {
    spec().to_pretty_json().expect("openapi serializes")
}

/// Who may call a route.
#[derive(Clone, Copy)]
enum Who {
    /// Anyone; no session or key needed.
    Anyone,
    /// Guests too, but what comes back depends on the viewer.
    Viewer,
    /// A signed-in member (session, or an API key).
    Member,
}

/// What a route takes in the body.
#[derive(Clone, Copy)]
enum Body {
    None,
    Json,
    Multipart,
}

use Body::{Json as J, Multipart as M, None as N};
use Who::{Anyone as A, Member as U, Viewer as V};

type Row = (&'static str, &'static str, &'static str, &'static str, Who, Body);

/// Member routes without a `#[utoipa::path]` on their handler: method, path, tag, summary, who, body.
const ROUTES: &[Row] = &[
    ("get", "/api/v1/topics/{id}/related", "topics", "Topics related by meaning", V, N),
    ("get", "/api/v1/topics/{id}/card", "topics", "Preview card of a topic", V, N),
    ("get", "/api/v1/topics/{id}/links", "topics", "Links posted in a topic, with click counts", V, N),
    ("get", "/api/v1/topics/{id}/state", "topics", "Reply count, last post and the viewer's read position", V, N),
    ("get", "/api/v1/topics/{id}/dates", "topics", "Post dates of a topic, for the timeline", V, N),
    ("get", "/api/v1/topics/{id}/posts/{n}", "topics", "A post by its number in the topic, with context", V, N),
    ("get", "/api/v1/topics/{id}/posts", "topics", "A window of a topic's posts", V, N),
    ("get", "/api/v1/topics/{id}/post-list", "topics", "Posts of a topic as plugin-extended rows", V, N),
    ("get", "/api/v1/forums/{id}/topics", "forums", "Topics of a forum as plugin-extended rows", V, N),
    ("get", "/api/v1/topics/{id}/ai-summary", "ai", "The stored AI summary of a topic", V, N),
    ("post", "/api/v1/topics/{id}/ai-summary", "ai", "Summarize a topic with the AI helper", U, J),
    ("post", "/api/v1/posts/{id}/ai-translate", "ai", "Translate a post with the AI helper", U, J),
    ("get", "/api/v1/ai/similar", "ai", "Topics similar in meaning to a text", U, N),
    ("post", "/api/v1/ai/helper", "ai", "Ask the writing helper", U, J),
    ("get", "/api/v1/posts/{id}/source", "posts", "Markdown source of a post, for editing", U, N),
    ("get", "/api/v1/posts/{id}/raw", "posts", "Raw text of a post", V, N),
    ("get", "/api/v1/posts/{id}/revisions", "posts", "Edit history of a post", U, N),
    ("get", "/api/v1/posts/{id}/reactions", "posts", "Who reacted to a post, by reaction", V, N),
    ("post", "/api/v1/preview", "posts", "Render Markdown to HTML as a post would be shown", V, J),
    ("get", "/api/v1/autocomplete", "search", "Complete a @mention, tag or emoji", V, N),
    ("get", "/api/v1/onebox", "posts", "Preview card of a link", V, N),
    ("post", "/api/v1/links/click", "topics", "Count a click on a link in a post", V, J),
    ("post", "/api/v1/links/seen", "topics", "Mark links as seen", V, J),
    ("get", "/api/v1/uploads/counts", "uploads", "Download counts of uploaded files", V, N),
    ("get", "/api/v1/emoji", "posts", "The emoji set, with custom emoji", V, N),
    ("get", "/api/v1/quotes/deleted", "posts", "Which quoted posts have since been deleted", V, N),
    ("get", "/api/v1/users/{name}/card", "users", "Hover card of a member", V, N),
    ("get", "/api/v1/online", "users", "Members online now", V, N),
    ("get", "/api/v1/live", "me", "Server-sent events: new posts, notifications, presence", V, N),
    ("put", "/api/v1/me/infinite-scroll", "me", "Turn infinite scroll on or off for the viewer", U, J),
    ("get", "/api/v1/saved-replies", "me", "The viewer's saved replies", U, N),
    ("post", "/api/v1/saved-replies", "me", "Save a reply", U, J),
    ("put", "/api/v1/saved-replies/{id}", "me", "Change a saved reply", U, J),
    ("delete", "/api/v1/saved-replies/{id}", "me", "Delete a saved reply", U, N),
    ("get", "/api/v1/drafts", "me", "A stored draft (query: `context`)", U, N),
    ("put", "/api/v1/drafts", "me", "Store a draft (query: `context`)", U, J),
    ("delete", "/api/v1/drafts", "me", "Discard a draft (query: `context`)", U, N),
    ("post", "/api/v1/uploads", "uploads", "Upload a file (multipart)", U, M),
    ("get", "/api/v1/chat/channels", "chat", "Channels the viewer may see", U, N),
    ("post", "/api/v1/chat/channels", "chat", "Create a channel of members", U, J),
    ("get", "/api/v1/chat/channels/{id}/members", "chat", "Members of a channel", U, N),
    ("post", "/api/v1/chat/channels/{id}/members", "chat", "Add a member to a channel", U, J),
    ("delete", "/api/v1/chat/channels/{id}/members/{user}", "chat", "Remove a member from a channel", U, N),
    ("get", "/api/v1/chat/me", "chat", "The viewer's chat state: unread counts, muted channels", U, N),
    ("post", "/api/v1/chat/dm", "chat", "Open a direct message with a member", U, J),
    ("get", "/api/v1/chat/channels/{id}/messages", "chat", "Messages of a channel, newest first", U, N),
    ("post", "/api/v1/chat/channels/{id}/messages", "chat", "Send a message", U, J),
    ("post", "/api/v1/chat/channels/{id}/join", "chat", "Join a channel", U, N),
    ("post", "/api/v1/chat/channels/{id}/leave", "chat", "Leave a channel", U, N),
    ("post", "/api/v1/chat/channels/{id}/mute", "chat", "Mute or unmute a channel", U, J),
    ("post", "/api/v1/chat/channels/{id}/read", "chat", "Mark a channel read", U, J),
    ("put", "/api/v1/chat/messages/{id}", "chat", "Edit a message", U, J),
    ("delete", "/api/v1/chat/messages/{id}", "chat", "Delete a message", U, N),
    ("post", "/api/v1/chat/messages/{id}/react", "chat", "React to a message", U, J),
    ("post", "/api/v1/chat/messages/{id}/pin", "chat", "Pin or unpin a message", U, J),
    ("post", "/api/v1/chat/messages/{id}/flag", "chat", "Report a message to staff", U, J),
    ("get", "/api/v1/push/key", "me", "The VAPID public key for Web Push", A, N),
    ("post", "/api/v1/push/subscribe", "me", "Subscribe this browser to Web Push", U, J),
    ("delete", "/api/v1/push/subscribe", "me", "Unsubscribe this browser from Web Push", U, J),
    ("get", "/api/v1/captcha/altcha", "auth", "A proof-of-work challenge for forms", A, N),
    ("post", "/api/v1/passkeys/register/options", "auth", "WebAuthn options to register a passkey", U, N),
    ("post", "/api/v1/passkeys/register", "auth", "Finish registering a passkey", U, J),
    ("post", "/api/v1/passkeys/login/options", "auth", "WebAuthn options to sign in with a passkey", A, N),
    ("post", "/api/v1/passkeys/login", "auth", "Finish signing in with a passkey", A, J),
    ("get", "/api/v1/passkeys/signal", "auth", "The passkeys to signal to the authenticator", U, N),
    ("get", "/api/v1/sudo", "auth", "Which ways the viewer can confirm sudo (password, passkey, ...)", U, N),
    ("post", "/api/v1/sudo", "auth", "Confirm sudo for staff actions", U, J),
    ("post", "/api/v1/csp-report", "auth", "Receiver for Content-Security-Policy reports", A, J),
    ("get", "/api/v1/openapi.json", "settings", "This document", A, N),
];

/// The tags, in the order a reader meets them.
const TAGS: &[(&str, &str)] = &[
    ("forums", "Forum tree and the topics of a forum"),
    ("topics", "Topics and the reads that hang off them"),
    ("posts", "Posts: read, edit, react, report; Markdown and link previews"),
    ("users", "Member profiles, cards and activity"),
    ("search", "Search and autocomplete"),
    ("me", "The caller: identity, notifications, messages, bookmarks, drafts, API keys, push"),
    ("chat", "Chat channels and messages"),
    ("ai", "AI helper features; each needs the feature on and a signed-in member"),
    ("uploads", "File uploads"),
    ("auth", "Passkeys, sudo, captcha and browser reports"),
    ("settings", "Public site settings"),
    ("admin", "Administration; needs an administrator session or an admin key"),
];

/// Fills in the routes `ROUTES` lists and the parts every route shares.
struct Documented;

impl Modify for Documented {
    #[expect(clippy::expect_used, reason = "the document is static; a failure here is a bug caught by the test")]
    fn modify(&self, api: &mut utoipa::openapi::OpenApi) {
        use serde_json::{Value, json};
        let mut doc = serde_json::to_value(&*api).expect("openapi to value");
        let problem = json!({ "$ref": "#/components/schemas/Problem" });
        let err = |what: &str| json!({ "description": what, "content": { "application/problem+json": { "schema": problem } } });
        let paths = doc["paths"].as_object_mut().expect("paths");
        for &(method, path, tag, summary, who, body) in ROUTES {
            let mut op = json!({
                "tags": [tag],
                "summary": summary,
                "operationId": format!("{method}_{}", path.trim_start_matches("/api/v1/").replace(['/', '{', '}', '-'], "_")),
                "responses": {
                    "200": { "description": "Success; JSON unless the summary says otherwise" },
                    "401": err("Sign in required"),
                    "403": err("Not allowed, or the key lacks the scope"),
                    "404": err("No such item, or not visible to the caller"),
                },
            });
            let params: Vec<Value> = path
                .split('/')
                .filter_map(|s| s.strip_prefix('{').and_then(|s| s.strip_suffix('}')))
                .map(|name| {
                    let kind = if matches!(name, "name" | "user") { "string" } else { "integer" };
                    json!({ "name": name, "in": "path", "required": true, "schema": { "type": kind } })
                })
                .collect();
            if !params.is_empty() {
                op["parameters"] = Value::Array(params);
            }
            let media = match body {
                Body::None => None,
                Body::Json => Some("application/json"),
                Body::Multipart => Some("multipart/form-data"),
            };
            if let Some(media) = media {
                op["requestBody"] = json!({ "content": { media: { "schema": { "type": "object" } } } });
            }
            if matches!(who, Who::Anyone) {
                op["security"] = json!([{}]);
                op["responses"].as_object_mut().expect("responses").remove("401");
            }
            let item = paths.entry(path).or_insert_with(|| json!({}));
            if item.get(method).is_none() {
                item[method] = op;
            }
        }
        // Typed bodies, query parameters and who-may-call, written per handler in `openapi_docs`.
        for d in super::openapi_docs::all() {
            let (Some(method), Some(path)) = (d["method"].as_str(), d["path"].as_str()) else { continue };
            let Some(op) = paths.get_mut(path).and_then(|p| p.get_mut(method)) else { continue };
            if let Some(q) = d["query"].as_array() {
                let mut params: Vec<Value> = op.get("parameters").and_then(Value::as_array).cloned().unwrap_or_default();
                for p in q {
                    params.push(json!({
                        "name": p["name"], "in": "query",
                        "required": p.get("required").and_then(Value::as_bool).unwrap_or(false),
                        "description": p.get("description").cloned().unwrap_or(Value::Null),
                        "schema": p.get("schema").cloned().unwrap_or_else(|| json!({ "type": "string" })),
                    }));
                }
                op["parameters"] = Value::Array(params);
            }
            if let Some(req) = d.get("request").filter(|v| !v.is_null()) {
                let media = if op.pointer("/requestBody/content/multipart~1form-data").is_some() { "multipart/form-data" } else { "application/json" };
                op["requestBody"] = json!({ "required": true, "content": { media: { "schema": req } } });
            }
            let status = d["status"].as_str().unwrap_or("200");
            let resp = d.get("response").filter(|v| !v.is_null());
            let security = match d["auth"].as_str() {
                Some("anyone") => Some(json!([{}])),
                Some("viewer") => Some(json!([{}, { "session": [] }, { "api_key": [] }])),
                _ => None,
            };
            if let Some(r) = op["responses"].as_object_mut() {
                if status != "200" || resp.is_some() {
                    let ok = r.remove("200");
                    if let Some(resp) = resp {
                        let mut entry = ok.unwrap_or_else(|| json!({}));
                        entry["description"] = d.get("description").cloned().unwrap_or_else(|| json!("Success"));
                        entry["content"] = json!({ "application/json": { "schema": resp } });
                        r.insert(status.to_owned(), entry);
                    } else if let Some(ok) = ok {
                        r.insert(status.to_owned(), ok);
                    }
                }
                if security.is_some() {
                    r.remove("401");
                }
            }
            if let Some(security) = security {
                op["security"] = security;
            }
        }
        // What every member route shares. Admin routes keep their own bearer scheme.
        for (path, item) in paths.iter_mut() {
            let Some(item) = item.as_object_mut() else { continue };
            for (method, op) in item.iter_mut() {
                if !matches!(method.as_str(), "get" | "post" | "put" | "delete" | "patch") {
                    continue;
                }
                let reads = method == "get";
                let mut params: Vec<Value> =
                    op.get("parameters").and_then(Value::as_array).cloned().unwrap_or_default();
                if reads && path != "/api/v1/live" {
                    params.push(json!({ "name": "If-None-Match", "in": "header", "required": false,
                        "description": "An `ETag` from an earlier response; answers 304 with no body while the data is unchanged.",
                        "schema": { "type": "string" } }));
                    if let Some(ok) = op.pointer_mut("/responses/200").and_then(Value::as_object_mut) {
                        ok.entry("headers").or_insert_with(|| {
                            json!({ "ETag": {
                                "description": "Validator of the JSON body; send it back as `If-None-Match`.",
                                "schema": { "type": "string" } } })
                        });
                    }
                    if let Some(r) = op.get_mut("responses").and_then(Value::as_object_mut) {
                        r.entry("304").or_insert_with(|| json!({ "description": "Not modified: the `If-None-Match` value is current" }));
                    }
                } else if !reads {
                    params.push(json!({ "name": "Idempotency-Key", "in": "header", "required": false,
                        "description": "1 to 255 visible ASCII characters. A retry with the same key and request replays the stored answer instead of acting twice; the same key on a different request is 422, and while the first is still running 409.",
                        "schema": { "type": "string", "maxLength": 255 } }));
                    if let Some(r) = op.get_mut("responses").and_then(Value::as_object_mut) {
                        r.entry("409").or_insert_with(|| err("A request with this `Idempotency-Key` is still being processed"));
                        r.entry("422").or_insert_with(|| err("This `Idempotency-Key` was used for a different request"));
                    }
                }
                op["parameters"] = Value::Array(params);
                if path.starts_with("/api/v1/admin/") {
                    continue;
                }
                // The scope an API key needs: the base scope, and `read:<resource>` or
                // `write:<resource>` when the key is limited to resources (`api_keys::resource_allowed`).
                let base = if reads { "read" } else { "write" };
                let resource = crate::platform::api_keys::resource_of(path);
                let note = match resource {
                    Some(r) => {
                        op["x-required-scope"] = json!({ "base": base, "resource": format!("{base}:{r}") });
                        format!("API key: `{base}`, or `{base}:{r}` when the key is limited to resources.")
                    }
                    None => {
                        op["x-required-scope"] = json!({ "base": base });
                        format!("API key: `{base}`; a key limited to resources cannot reach this route.")
                    }
                };
                let note = if reads { note } else { format!("{note} Writes with a session cookie also send `X-CSRF-Token`.") };
                if op.get("security").is_none() {
                    op["security"] = json!([{ "session": [] }, { "api_key": [] }]);
                }
                let had = op.get("description").and_then(Value::as_str).unwrap_or_default().to_owned();
                op["description"] = Value::String(if had.is_empty() { note } else { format!("{had}\n\n{note}") });
            }
        }
        doc["tags"] = Value::Array(TAGS.iter().map(|(n, d)| json!({ "name": n, "description": d })).collect());
        if let Some(schemes) = doc.pointer_mut("/components/securitySchemes").and_then(Value::as_object_mut) {
            schemes.insert(
                "session".into(),
                json!({ "type": "apiKey", "in": "cookie", "name": "__Host-bf_session",
                    "description": "The session cookie. Writes also send `X-CSRF-Token`." }),
            );
            schemes.insert(
                "api_key".into(),
                json!({ "type": "http", "scheme": "bearer",
                    "description": "An API key from /api/v1/me/api-keys; its base scope and resource scopes limit what it may call." }),
            );
        }
        *api = serde_json::from_value(doc).expect("openapi from value");
    }
}

#[cfg(test)]
mod tests {
    /// Every row of `ROUTES` has a typed entry in `openapi_docs` (a body or response schema, and
    /// who may call), and the other way round.
    #[test]
    #[expect(clippy::expect_used, reason = "a test over static data")]
    fn every_listed_route_is_typed() {
        let docs = super::super::openapi_docs::all();
        let key = |m: &str, p: &str| format!("{m} {p}");
        let typed: std::collections::HashSet<String> =
            docs.iter().map(|d| key(d["method"].as_str().expect("method"), d["path"].as_str().expect("path"))).collect();
        let listed: std::collections::HashSet<String> = super::ROUTES.iter().map(|r| key(r.0, r.1)).collect();
        let untyped: Vec<_> = listed.difference(&typed).collect();
        let stray: Vec<_> = typed.difference(&listed).collect();
        assert!(untyped.is_empty(), "ROUTES rows without an openapi_docs entry: {untyped:#?}");
        assert!(stray.is_empty(), "openapi_docs entries without a ROUTES row: {stray:#?}");
        for d in &docs {
            assert!(d.get("auth").is_some_and(|a| a.is_string()), "no auth in {d}");
            assert!(d.get("response").is_some() || d["status"].as_str().is_some_and(|s| s != "200"), "no response in {d}");
        }
    }

    /// Member routes that are deliberately not in the document, each with the reason. Everything
    /// else registered under `/api/v1` must be documented, method by method.
    const NOT_DOCUMENTED: &[(&str, &str)] = &[
        ("/api/v1/webhooks/bounce", "receiver for the mail provider's bounce webhook, signed by the provider; not for clients"),
        ("/api/v1/webhooks/inbound-mail", "receiver for the mail provider's inbound mail webhook; not for clients"),
    ];

    /// Every `/api/v1` route registered in the server (admin routes, which stay out of the public
    /// spec, and `NOT_DOCUMENTED` aside) is in the document, method by method. Reads `.route(` calls
    /// whether the path sits on the same line or the next, and the router nested under `/api/v1`.
    #[test]
    #[expect(clippy::expect_used, reason = "a test over the source tree")]
    fn every_member_route_is_documented() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).expect("read_dir").flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        let mut files = Vec::new();
        walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
        let spec = serde_json::to_value(super::spec()).expect("spec");
        let paths = spec["paths"].as_object().expect("paths");
        let mut missing = Vec::new();
        let mut exempt_seen = std::collections::HashSet::new();
        let mut seen = 0;
        for f in files {
            let text = std::fs::read_to_string(&f).expect("read");
            // `admin::settings::router()` is nested under `/api/v1` in `lib.rs`.
            let nested = f.ends_with("admin/settings.rs");
            let mut from = 0;
            while let Some(at) = text[from..].find(".route(") {
                let start = from + at + ".route(".len();
                from = start;
                let rest = text[start..].trim_start();
                let Some(rest) = rest.strip_prefix('"') else { continue };
                let Some(end) = rest.find('"') else { continue };
                let (raw, tail) = rest.split_at(end);
                let path = if nested { format!("/api/v1{raw}") } else { raw.to_owned() };
                if !path.starts_with("/api/v1/") || path.starts_with("/api/v1/admin/") {
                    continue;
                }
                // The method calls up to the next route or the end of the statement.
                let stop = [tail.find(".route("), tail.find(';'), Some(400.min(tail.len()))].into_iter().flatten().min().unwrap_or(tail.len());
                let tail = &tail[..tail.floor_char_boundary(stop)];
                if let Some((p, _)) = NOT_DOCUMENTED.iter().find(|(p, _)| *p == path) {
                    exempt_seen.insert(*p);
                    continue;
                }
                for method in ["get", "post", "put", "delete", "patch"] {
                    let call = format!("{method}(");
                    let found = tail
                        .match_indices(&call)
                        .any(|(i, _)| !tail[..i].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_'));
                    if found {
                        seen += 1;
                        if paths.get(&path).and_then(|p| p.get(method)).is_none() {
                            missing.push(format!("{} {path}", method.to_uppercase()));
                        }
                    }
                }
            }
        }
        assert!(seen > 80, "the scan found only {seen} routes; did the router move?");
        for (p, why) in NOT_DOCUMENTED {
            assert!(exempt_seen.contains(p), "NOT_DOCUMENTED lists {p} ({why}) but no such route is registered; remove it");
        }
        assert!(missing.is_empty(), "routes missing from the OpenAPI document (annotate the handler with #[utoipa::path], or add a ROUTES row and an openapi_docs entry): {missing:#?}");
    }
}
