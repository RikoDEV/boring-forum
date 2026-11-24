//! The administrator's tools of the MCP server: find a route of the admin API, read it, change it.
//!
//! Nothing here widens what the API allows. The caller's `admin` key (or the bootstrap admin
//! token) is passed to the API, whose own checks decide; these tools only add a narrower door:
//! off unless `mcp_admin_enabled` is on, a short list of routes held back (a shell, impersonating
//! a member, arbitrary SQL, installing code, restoring a backup, exports of personal data), an
//! agent cannot change the switches that give it this access, and every write leaves a line in
//! the audit log saying it came through MCP.

use super::catalog::ROUTES;
use super::{Ctx, Payload, failed, finish, forward};
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Method, header};
use serde_json::{Map, Value, json};
use std::net::SocketAddr;

type Peer = Option<ConnectInfo<SocketAddr>>;

const ADMIN_TOOLS: [&str; 3] = ["admin_routes", "admin_get", "admin_write"];

pub(super) fn is_admin_tool(name: &str) -> bool {
    ADMIN_TOOLS.contains(&name)
}

/// Routes an agent never reaches through MCP, whatever the key: they run code or SQL on the server,
/// act as someone else, or hand out the whole site's or a person's data.
const HELD_BACK: &[&str] = &[
    "/api/v1/admin/shell",
    "/api/v1/admin/explorer/run",
    "/api/v1/admin/users/{id}/impersonate",
    "/api/v1/admin/users/{id}/export",
    "/api/v1/admin/users/{id}/api-keys/{key}",
    "/api/v1/admin/plugins/install",
    "/api/v1/admin/plugins/install-url",
    "/api/v1/admin/themes/install",
    "/api/v1/admin/themes/install-url",
    "/api/v1/admin/backups/{name}/restore",
    "/api/v1/admin/backups/{name}/download-link",
    "/api/v1/admin/site-export",
    "/api/v1/admin/settings-import",
];

/// Settings an agent may not write: the ones that give it this access.
const OWN_SWITCHES: [&str; 2] = ["mcp_enabled", "mcp_admin_enabled"];

pub(super) fn tools() -> Vec<Value> {
    let path = json!({ "type": "string", "description": "A route of the admin API from admin_routes, with its {placeholders} filled in, such as /api/v1/admin/users/12. No query string." });
    vec![
        json!({
            "name": "admin_routes",
            "title": "Find an admin API route",
            "description": "The routes of the admin API, one per line: method, path and the name of what it does. Filter by a word of the path or name. Routes that are never available here are left out.",
            "inputSchema": { "type": "object", "properties": {
                "filter": { "type": "string", "description": "Only routes whose path or name contains this, such as `users` or `ban`." },
                "method": { "type": "string", "enum": ["GET", "POST", "PUT", "PATCH", "DELETE"] },
            }, "required": [], "additionalProperties": false },
            "annotations": { "title": "Find an admin API route", "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false },
        }),
        json!({
            "name": "admin_get",
            "title": "Read from the admin API",
            "description": "GET a route of the admin API: users, settings, forums, reports, logs, statistics and so on. Read before you write: what a route returns is usually the shape its write takes.",
            "inputSchema": { "type": "object", "properties": {
                "path": path,
                "query": { "type": "object", "description": "Query parameters, such as {\"page\": 1}.", "additionalProperties": { "type": ["string", "number", "boolean"] } },
            }, "required": ["path"], "additionalProperties": false },
            "annotations": { "title": "Read from the admin API", "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false },
        }),
        json!({
            "name": "admin_write",
            "title": "Change the forum through the admin API",
            "description": "POST, PUT, PATCH or DELETE a route of the admin API, as the administrator who owns the key. It changes the live forum; some changes cannot be undone (deleting, banning, merging, sending mail to everyone). Look at the current state with admin_get first, and say what you are about to change. The change is logged as made through MCP.",
            "inputSchema": { "type": "object", "properties": {
                "method": { "type": "string", "enum": ["POST", "PUT", "PATCH", "DELETE"] },
                "path": path,
                "body": { "type": "object", "description": "The JSON body, where the route takes one." },
                "idempotency_key": { "type": "string", "description": "Optional. Sending the same key again repeats the first answer instead of making the change twice." },
            }, "required": ["method", "path"], "additionalProperties": false },
            "annotations": { "title": "Change the forum through the admin API", "readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": false },
        }),
    ]
}

/// Whether a route may be used here, in the form of its template.
fn held_back(template: &str) -> bool {
    HELD_BACK.contains(&template)
}

/// A path segment a caller may fill a placeholder with: one name or number, never a way out of it.
fn segment_ok(seg: &str) -> bool {
    !seg.is_empty()
        && seg != "."
        && seg != ".."
        && seg.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'-' | b':' | b'@'))
}

/// The catalogued route a method and path ask for: `(template, handler)`, or why not.
fn find(method: &str, path: &str) -> Result<(&'static str, &'static str), String> {
    if !path.starts_with("/api/v1/admin/") {
        return Err(
            "The path must be a route of the admin API, starting /api/v1/admin/. Find one with admin_routes.".into()
        );
    }
    let given: Vec<&str> = path.split('/').collect();
    let mut other_method = false;
    for (m, template, handler) in ROUTES {
        let parts: Vec<&str> = template.split('/').collect();
        let fits = parts.len() == given.len()
            && parts.iter().zip(&given).all(|(t, g)| if t.starts_with('{') { segment_ok(g) } else { t == g });
        if !fits {
            continue;
        }
        if held_back(template) {
            return Err("That route is not available through MCP.".into());
        }
        if *m == method {
            return Ok((template, handler));
        }
        other_method = true;
    }
    Err(if other_method {
        format!("That route does not take {method}. See admin_routes for what it does take.")
    } else {
        "No such admin route. Find one with admin_routes.".into()
    })
}

/// Who may use the admin tools: `Some(id)` is an administrator's key (`None` inside is the
/// bootstrap token, which belongs to no member); the outer `None` is anyone else, or the tools off.
pub(super) async fn caller(ctx: &Ctx, headers: &HeaderMap, peer: &Peer) -> Option<Option<i64>> {
    if !ctx.state.settings().bool("mcp_admin_enabled") {
        return None;
    }
    let bearer = headers.get(header::AUTHORIZATION)?.to_str().ok()?.strip_prefix("Bearer ")?;
    if let Some(token) = ctx.state.config.admin_token.as_deref()
        && !bearer.starts_with("bf_")
    {
        return crate::admin::api::constant_time_eq(bearer.as_bytes(), token.as_bytes()).then_some(None);
    }
    let answer = forward(ctx, headers, peer, Method::GET, "/api/v1/me", Payload::None, None).await.ok()?;
    let me: Value = serde_json::from_str(&answer.text).ok()?;
    (answer.status.is_success() && me.get("scope").and_then(Value::as_str) == Some("admin"))
        .then(|| me.get("id").and_then(Value::as_i64))
}

pub(super) async fn call(ctx: &Ctx, headers: &HeaderMap, peer: &Peer, name: &str, args: &Map<String, Value>) -> Value {
    let Some(who) = caller(ctx, headers, peer).await else {
        return failed(
            "The admin tools need an `admin` API key of an administrator, and the site's MCP admin setting on (Admin, Settings, MCP server).",
        );
    };
    let text = |key: &str| args.get(key).and_then(Value::as_str);
    match name {
        "admin_routes" => routes(text("filter"), text("method")),
        "admin_get" => {
            let Some(path) = text("path") else { return failed("`path` is required.") };
            match find("GET", path) {
                Ok(_) => read(ctx, headers, peer, path, args).await,
                Err(e) => failed(e),
            }
        }
        "admin_write" => write(ctx, headers, peer, who, args).await,
        _ => failed("Unknown tool."),
    }
}

fn routes(filter: Option<&str>, method: Option<&str>) -> Value {
    let filter = filter.map(str::to_lowercase);
    let lines: Vec<String> = ROUTES
        .iter()
        .filter(|(_, template, _)| !held_back(template))
        .filter(|(m, ..)| method.is_none_or(|want| want.eq_ignore_ascii_case(m)))
        .filter(|(_, template, handler)| {
            filter.as_deref().is_none_or(|f| template.to_lowercase().contains(f) || handler.to_lowercase().contains(f))
        })
        .map(|(m, template, handler)| format!("{m} {template}  ({handler})"))
        .collect();
    if lines.is_empty() {
        return json!({ "content": [{ "type": "text", "text": "No route matches." }] });
    }
    json!({ "content": [{ "type": "text", "text": lines.join("\n") }] })
}

/// A query string from an object of scalars.
fn query_string(args: &Map<String, Value>) -> Result<String, String> {
    let Some(query) = args.get("query") else { return Ok(String::new()) };
    let Some(query) = query.as_object() else { return Err("`query` must be an object.".into()) };
    let mut parts = Vec::new();
    for (k, v) in query {
        let value = match v {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            _ => return Err(format!("`query.{k}` must be a string, number or boolean.")),
        };
        parts.push(format!("{}={}", super::enc(k), super::enc(&value)));
    }
    Ok(if parts.is_empty() { String::new() } else { format!("?{}", parts.join("&")) })
}

async fn read(ctx: &Ctx, headers: &HeaderMap, peer: &Peer, path: &str, args: &Map<String, Value>) -> Value {
    let query = match query_string(args) {
        Ok(q) => q,
        Err(e) => return failed(e),
    };
    match forward(ctx, headers, peer, Method::GET, &format!("{path}{query}"), Payload::None, None).await {
        Ok(a) => finish(a.status, redact_text(a.text)),
        Err(e) => failed(e),
    }
}

async fn write(ctx: &Ctx, headers: &HeaderMap, peer: &Peer, who: Option<i64>, args: &Map<String, Value>) -> Value {
    let (Some(method), Some(path)) =
        (args.get("method").and_then(Value::as_str), args.get("path").and_then(Value::as_str))
    else {
        return failed("`method` and `path` are required.");
    };
    if !matches!(method, "POST" | "PUT" | "PATCH" | "DELETE") {
        return failed("`method` must be POST, PUT, PATCH or DELETE.");
    }
    if let Err(e) = find(method, path) {
        return failed(e);
    }
    if let Some(key) = path.strip_prefix("/api/v1/admin/settings/")
        && OWN_SWITCHES.contains(&key)
    {
        return failed("The MCP switches are changed in the admin panel, not through MCP.");
    }
    let body = match args.get("body") {
        None | Some(Value::Null) => Payload::None,
        Some(b @ Value::Object(_)) => Payload::Json(b.to_string().into_bytes()),
        Some(_) => return failed("`body` must be a JSON object."),
    };
    let key = args.get("idempotency_key").and_then(Value::as_str);
    let Ok(verb) = Method::from_bytes(method.as_bytes()) else { return failed("`method` is not valid.") };
    let answer = forward(ctx, headers, peer, verb, path, body, key).await;
    let status = answer.as_ref().map_or(0, |a| a.status.as_u16());
    // A line in the audit log for every attempt, so what an agent did is told apart from what the
    // administrator did in the panel.
    if let Some(pool) = &ctx.state.pool {
        let details = json!({ "method": method, "status": status });
        if let Err(e) = crate::admin::api::audit_log(pool, who, "mcp.admin_write", path, details).await {
            tracing::warn!(error = %e, "could not record an MCP admin write");
        }
    }
    match answer {
        Ok(a) => finish(a.status, redact_text(a.text)),
        Err(e) => failed(e),
    }
}

/// Field names whose text is a credential, whatever route returned it.
fn secret_field(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("secret")
        || n.contains("password")
        || n.contains("token")
        || n.contains("passphrase")
        || n.ends_with("_hash")
        || n.ends_with("api_key")
        || n.ends_with("private_key")
}

fn redact(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                if secret_field(k) && v.as_str().is_some_and(|s| !s.is_empty()) {
                    *v = Value::String("[hidden]".into());
                } else {
                    redact(v);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact),
        _ => {}
    }
}

/// An answer with credentials hidden. A key and a secret never leave the admin API anyway (settings
/// say only that one is set); this covers a route that forgot, because what is returned goes on to a
/// model provider.
fn redact_text(text: String) -> String {
    match serde_json::from_str::<Value>(&text) {
        Ok(mut v) => {
            redact(&mut v);
            v.to_string()
        }
        Err(_) => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_route_by_method_and_template() {
        assert_eq!(
            find("GET", "/api/v1/admin/users/12/overview").map(|r| r.0),
            Ok("/api/v1/admin/users/{id}/overview")
        );
        assert!(find("PATCH", "/api/v1/admin/users/12").is_ok());
        assert!(find("GET", "/api/v1/admin/users/12").is_err_and(|e| e.contains("does not take GET")));
        assert!(find("GET", "/api/v1/admin/nope").is_err());
        assert!(find("GET", "/api/v1/me").is_err());
    }

    #[test]
    fn a_placeholder_cannot_leave_its_segment() {
        for bad in [
            "/api/v1/admin/users/..",
            "/api/v1/admin/users/../settings",
            "/api/v1/admin/users/1%2F..",
            "/api/v1/admin/users/",
            "/api/v1/admin/users/1/",
            "/api/v1/admin/users/a b",
        ] {
            assert!(find("GET", bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_dangerous_routes_are_held_back_for_every_method() {
        for template in HELD_BACK {
            let path = template.replace("{id}", "1").replace("{key}", "k").replace("{name}", "n");
            for m in ["GET", "POST", "PUT", "PATCH", "DELETE"] {
                assert!(find(m, &path).is_err(), "{m} {path}");
            }
        }
        assert!(routes(None, None)["content"][0]["text"].as_str().is_some_and(|t| !t.contains("/admin/shell")));
    }

    #[test]
    fn every_held_back_route_is_in_the_catalog() {
        for template in HELD_BACK {
            assert!(ROUTES.iter().any(|(_, t, _)| t == template), "{template} is not a route (renamed?)");
        }
    }

    #[test]
    fn credentials_are_hidden_in_what_is_returned() {
        let out =
            redact_text(r#"{"name":"a","secret":"s3","nested":[{"api_key":"k","token_count":3,"token":""}]}"#.into());
        assert!(out.contains("[hidden]") && !out.contains("s3") && !out.contains("\"k\""));
        assert!(out.contains("\"token_count\":3"));
    }

    #[test]
    fn the_catalog_covers_the_router() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
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
        let mut missing = Vec::new();
        for f in files {
            if f.to_string_lossy().contains("mcp") {
                continue;
            }
            let src = std::fs::read_to_string(&f).unwrap_or_default();
            for part in src.split(".route(").skip(1) {
                let Some(path) = part.trim_start().strip_prefix('"').and_then(|r| r.split('"').next()) else {
                    continue;
                };
                let full = if path.starts_with("/admin/") { format!("/api/v1{path}") } else { path.to_owned() };
                if full.starts_with("/api/v1/admin/") && !ROUTES.iter().any(|(_, t, _)| *t == full) {
                    missing.push(full);
                }
            }
        }
        assert!(missing.is_empty(), "run `python scripts/gen-mcp-catalog.py`: not in the MCP catalog: {missing:?}");
    }
}
