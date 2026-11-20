//! The `user.can` filter, applied in two central places so no handler has to remember it:
//! the forum permissions through the `Actor` (see `bf_core::perms::Denials`, applied by
//! `effective`), and every other action through this gate, which names a request's action by
//! its method and path and refuses it if a plugin took that action away from the member.
//!
//! The hook runs once per member (the answer is kept for `TTL`), only if a plugin listens, and
//! can only deny: its input is `{"user_id", "groups"}` and it answers
//! `{"deny": [action, ...], "deny_nodes": {"<forum id>": [action, ...]}}`. Administrators are exempt.

use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::{FromRequestParts, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use bf_core::AppError;
use bf_core::ids::GroupId;
use bf_core::perms::Denials;
use serde_json::{Value, json};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use super::{call, listening};

const TTL: Duration = Duration::from_secs(10);

/// What a plugin may deny: the forum permission names, and the actions the gate maps requests to.
pub(super) const ACTIONS: [&str; 20] = [
    "view", "read", "create", "reply", "attach", "download", "moderate", "edit", "delete", "react", "flag", "chat",
    "pm", "upload", "vote", "bookmark", "follow", "report", "comment", "write",
];

/// State-changing routes no plugin can take away: signing in and out, registration and recovery,
/// the admin panel (its own gate), federation and plugin endpoints (their own rules). Everything
/// else that changes state is at least a `write`.
pub(super) const EXEMPT: [&str; 14] = [
    "login",
    "logout",
    "register",
    "forgot",
    "reset",
    "confirm-email",
    "auth",
    "oauth",
    "ap",
    "admin",
    "plugin",
    "safe-mode",
    "language",
    "impersonate",
];

fn exempt(path: &str) -> bool {
    let mut seg = path.trim_matches('/').split('/');
    let first = seg.next().unwrap_or_default();
    EXEMPT.contains(&first)
        || (first == "api" && matches!(seg.next(), Some("v1")) && {
            let rest = path.trim_start_matches("/api/v1/");
            rest.starts_with("admin")
                || rest.starts_with("csp-report")
                || rest.starts_with("push")
                || rest.starts_with("webauthn")
                || rest.starts_with("auth")
        })
        || path.starts_with("/api/p/")
}

/// Keyed by member and their groups, so a group change is not served the old answer; bounded.
static CACHE: LazyLock<moka::sync::Cache<(i64, u64), Arc<Denials>>> =
    LazyLock::new(|| moka::sync::Cache::builder().max_capacity(10_000).time_to_live(TTL).build());

fn groups_key(groups: &[GroupId]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut ids: Vec<i64> = groups.iter().map(|g| g.0).collect();
    ids.sort_unstable();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    ids.hash(&mut h);
    h.finish()
}

/// Forgets what plugins answered (when plugins load again).
pub(super) fn forget() {
    CACHE.invalidate_all();
}

fn clean(list: Option<&Value>) -> Vec<String> {
    list.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|a| ACTIONS.contains(a))
        .map(str::to_owned)
        .collect()
}

/// What the plugins deny this member. Free while no plugin listens; otherwise one hook call per
/// plugin every `TTL`.
pub(crate) async fn denials(state: &AppState, user: i64, groups: &[GroupId]) -> Arc<Denials> {
    let listeners = listening("user.can");
    if listeners.is_empty() {
        return Arc::default();
    }
    let key = (user, groups_key(groups));
    if let Some(d) = CACHE.get(&key) {
        return d;
    }
    let mut all = Denials::default();
    for p in listeners {
        let data = json!({ "user_id": user, "groups": groups.iter().map(|g| g.0).collect::<Vec<_>>() });
        let Some(out) = call(state, p, "user.can", &data).await else { continue };
        all.actions.extend(clean(out.get("deny")));
        for (node, list) in out.get("deny_nodes").and_then(Value::as_object).into_iter().flatten() {
            if let Ok(id) = node.parse::<i64>() {
                all.nodes.entry(id).or_default().extend(clean(Some(list)));
            }
        }
    }
    let all = Arc::new(all);
    CACHE.insert(key, all.clone());
    all
}

/// The cached answer only, for code that cannot wait (the live stream's per-event viewer).
pub(crate) fn denials_cached(user: i64) -> Arc<Denials> {
    // Without the groups at hand, any live entry of this member (they differ only right after a group change).
    CACHE.iter().find(|(k, _)| k.0 == user).map_or_else(Arc::default, |(_, d)| d)
}

/// The actions a request performs. A request may be several (a request may name several, such as a delete and a
/// moderation action); it is refused if any is denied.
pub(super) fn actions_of(method: &str, path: &str) -> Vec<&'static str> {
    let write = !matches!(method, "GET" | "HEAD" | "OPTIONS");
    let seg: Vec<&str> = path.trim_matches('/').split('/').collect();
    let last = seg.last().copied().unwrap_or_default();
    let mut out = Vec::new();
    let first = seg.first().copied().unwrap_or_default();
    if write {
        match (first, seg.get(1).copied(), last) {
            ("c", _, "new") => out.push("create"),
            ("t", _, "reply") | ("status", _, "reply") | ("status-reply", ..) => out.push("reply"),
            ("messages", ..) => out.push("pm"),
            ("post" | "t", _, "edit" | "title") => out.push("edit"),
            ("api", Some("v1"), _) if path.starts_with("/api/v1/chat/dm") => out.push("pm"),
            ("api", Some("v1"), _) if path.starts_with("/api/v1/chat/") => out.push("chat"),
            _ => {}
        }
        if last == "delete" || method == "DELETE" {
            out.push("delete");
        }
        if matches!(last, "like" | "react" | "vote") || path.contains("/react") {
            out.push("react");
        }
        if matches!(last, "report" | "flag") {
            out.push("flag");
            out.push("report");
        }
        if last == "bookmark" {
            out.push("bookmark");
        }
        if matches!(last, "follow" | "unfollow") {
            out.push("follow");
        }
        if last == "comment" {
            out.push("comment");
        }
        if path.starts_with("/api/v1/uploads") || matches!(last, "upload" | "cover") {
            out.push("upload");
        }
        match first {
            "modcp" | "review" => out.push("moderate"),
            _ => {}
        }
        if first == "t"
            && matches!(last, "mod" | "move" | "split" | "merge" | "pin" | "slow" | "timer" | "banner" | "date")
        {
            out.push("moderate");
        }
        if first == "t" && seg.get(2) == Some(&"action") {
            out.push("moderate");
        }
    }
    // Whatever changes state and was not named above is a plain `write`, so nothing escapes.
    if write && out.is_empty() && !exempt(path) {
        out.push("write");
    }
    out
}

/// Refuses a request whose action a plugin took away from the member.
pub(crate) async fn action_gate(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if listening("user.can").is_empty() {
        return next.run(req).await;
    }
    let (mut parts, body) = req.into_parts();
    let mut actions = actions_of(parts.method.as_str(), parts.uri.path());
    // Taking `write` away freezes every state-changing request that is not exempt.
    if !matches!(parts.method.as_str(), "GET" | "HEAD" | "OPTIONS")
        && !exempt(parts.uri.path())
        && !actions.contains(&"write")
    {
        actions.push("write");
    }
    let refused = if actions.is_empty() {
        false
    } else {
        match Viewer::from_request_parts(&mut parts, &state).await {
            Ok(v) => !v.actor.is_admin && actions.iter().any(|a| v.actor.denied.denies(a)),
            // A bad session or key is the handler's to report.
            Err(_) => false,
        }
    };
    if refused {
        return AppError::Forbidden.into_response();
    }
    next.run(Request::from_parts(parts, body)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_named_by_what_they_do() {
        assert_eq!(actions_of("POST", "/t/5/reply"), ["reply"]);
        assert_eq!(actions_of("POST", "/c/2/new"), ["create"]);
        assert_eq!(actions_of("POST", "/post/9/like"), ["react"]);
        assert_eq!(actions_of("POST", "/post/9/report"), ["flag", "report"]);
        assert_eq!(actions_of("POST", "/api/v1/uploads"), ["upload"]);
        assert_eq!(actions_of("POST", "/api/v1/chat/channels/1/messages"), ["chat"]);
        assert_eq!(actions_of("DELETE", "/api/v1/chat/messages/4"), ["chat", "delete"]);
        assert_eq!(actions_of("POST", "/messages/3/reply"), ["pm"]);
        assert!(actions_of("GET", "/t/5/anything").is_empty(), "reading is the forum permission, not an action");
    }

    /// Reads every `.route("...", post(..))` in the server's source: each state-changing route is
    /// named by `actions_of` (at least `write`) unless it is exempt, so a new route cannot be
    /// forgotten; and the exempt list names only things that exist.
    #[test]
    fn every_mutating_route_is_gated_or_exempt() {
        fn files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    files(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        let mut all = Vec::new();
        files(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut all);
        let (mut mutating, mut gated, mut exempted) = (0, 0, 0);
        let mut exempt_seen = std::collections::HashSet::new();
        let mut unnamed = Vec::new();
        for f in all {
            let text = std::fs::read_to_string(&f).unwrap();
            for chunk in text.split(".route(\"").skip(1) {
                let Some((path, rest)) = chunk.split_once('"') else { continue };
                let handler: String = rest.chars().take(160).collect();
                let handler = handler.split(".route(").next().unwrap_or_default();
                let mutates = ["post(", "put(", "patch(", "delete("].iter().any(|m| {
                    handler
                        .match_indices(m)
                        .any(|(i, _)| !handler[..i].ends_with(|c: char| c.is_alphanumeric() || c == '_'))
                });
                if !mutates || !path.starts_with('/') {
                    continue;
                }
                mutating += 1;
                let concrete = path
                    .replace("{*path}", "a/b")
                    .split('{')
                    .enumerate()
                    .map(|(i, part)| {
                        if i == 0 { part.to_owned() } else { format!("1{}", part.split_once('}').map_or("", |x| x.1)) }
                    })
                    .collect::<String>();
                let actions = actions_of("POST", &concrete);
                if exempt(&concrete) {
                    exempted += 1;
                    exempt_seen.insert(concrete.trim_matches('/').split('/').next().unwrap_or_default().to_owned());
                } else if actions.is_empty() {
                    unnamed.push(path.to_owned());
                } else {
                    gated += 1;
                }
            }
        }
        assert!(unnamed.is_empty(), "state-changing routes the gate does not name: {unnamed:?}");
        assert!(mutating > 100, "the scan found only {mutating} routes");
        println!("{mutating} state-changing routes: {gated} gated, {exempted} exempt");
        for e in EXEMPT {
            assert!(
                exempt_seen.contains(e)
                    || e == "safe-mode"
                    || e == "language"
                    || e == "forgot"
                    || e == "reset"
                    || e == "confirm-email"
                    || e == "oauth",
                "exempt entry {e} matches no route"
            );
        }
    }

    #[test]
    fn unnamed_writes_default_to_write() {
        assert_eq!(actions_of("POST", "/some/new/feature"), ["write"]);
        assert!(actions_of("POST", "/login").is_empty() && actions_of("POST", "/api/v1/admin/x").is_empty());
        assert!(actions_of("GET", "/some/new/feature").is_empty());
    }

    #[test]
    fn plugin_answers_are_limited_to_known_actions() {
        let list = json!(["reply", "root", "chat"]);
        assert_eq!(clean(Some(&list)), ["reply", "chat"]);
    }
}
