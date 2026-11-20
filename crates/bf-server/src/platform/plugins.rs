//! Plugin host: WebAssembly modules with no ambient access. A module can only call the
//! host functions below, each gated by a capability an admin approved, with limits on work
//! and memory per call. A module that keeps failing is disabled automatically.
//!
//! Module ABI: export `memory`, `alloc(len) -> ptr` and `handle(ptr, len) -> (ptr << 32 | len)`.
//! Input and output are JSON. Input: `{"hook", "data", "settings"}`. A filter may answer
//! `{"reject": "message"}` or `{"data": {...}}`; an `api.request` answers `{"status", "json"}`.

use crate::state::AppState;
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{any, get, post, put};
use bf_db::platform::plugins;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, RwLock};
use wasmtime::{Config, Engine, Module};

mod admin;
mod api;
mod apps;
mod content_types;
mod dev;
mod extras;
mod forms;
mod gate;
mod hooks;
mod host;
mod load;
mod ops;
mod ops_content;
mod package;
mod registry;
mod routes;
mod run;
mod schedule;
mod stats;
mod subject;
mod version;

use self::admin::{disable, enable, grant_permission, install, install_url, list, rollback, set_settings, uninstall};
pub(crate) use self::content_types::pending_count as held_for_review_count;
pub(crate) use self::extras::{check_widget_config, emoji_entries, finish_callbacks};
use self::host::{HostState, linker};
use self::load::schema_list;
use self::package::read_package;
use self::routes::{
    action_run, api_route, asset_file, client_file, emoji_file, login_begin, login_callback, member_filter, panel_page,
    slot_render,
};
pub(crate) use self::routes::{onebox_card, user_card_html};
use self::run::{call, listening, write_out};

pub use self::admin::scaffold;
pub(crate) use self::api::attach_meta;
pub use self::dev::{pack, sign, watch};
pub(crate) use self::gate::{action_gate, denials, denials_cached};
pub use self::hooks::spawn_jobs;
pub(crate) use self::hooks::{
    add_plugin_fields, after_cook, before_post, before_post_system, cook, dispatch, dispatch_wait, extra_fields,
    filter_text, has_serializers, registration_refused, render_widget, sitemap_paths,
};
pub(crate) use self::host::local_path;
pub use self::load::reload;
pub(crate) use self::package::archive_prefix;
pub(crate) use self::registry::signup_refused;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/plugins/{file}", get(client_file))
        .route("/api/v1/forums/{id}/topics", get(api::forum_topics))
        .route("/api/v1/topics/{id}/post-list", get(api::post_list))
        .route("/plugin-files/{id}/{*path}", get(ops_content::serve_file))
        .route("/plugins/{dir}/{*path}", get(asset_file))
        .route("/plugin-slot/{slot}", get(slot_render))
        .route("/plugin-emoji/{id}/{*path}", get(emoji_file))
        .route("/plugin/{id}/action/{key}", post(action_run))
        .route("/api/p/{id}/{*path}", any(api_route))
        .route("/app/{id}", any(apps::app_root))
        .route("/app/{id}/{*rest}", any(apps::app_path))
        .route("/review/apps", get(content_types::review_page))
        .route("/review/apps/{id}", post(content_types::decide))
        .route("/plugin/{id}/{key}", get(panel_page))
        .route("/auth/p/{id}/{key}", get(login_begin))
        .route("/auth/p/{id}/{key}/callback", get(login_callback))
        .route("/api/v1/admin/plugins/{id}/member-filter", post(member_filter))
        .route("/api/v1/admin/plugins", get(list))
        .route("/api/v1/admin/plugins/install", post(install).layer(DefaultBodyLimit::max(MAX_PACKAGE)))
        .route("/api/v1/admin/plugins/install-url", post(install_url))
        .route("/api/v1/admin/plugins/{id}/enable", post(enable))
        .route("/api/v1/admin/plugins/{id}/disable", post(disable))
        .route("/api/v1/admin/plugins/{id}/rollback", post(rollback))
        .route("/api/v1/admin/plugins/{id}/permissions/{key}", put(grant_permission))
        .route("/api/v1/admin/plugins/{id}/settings", put(set_settings))
        .route("/api/v1/admin/plugins/{id}", axum::routing::delete(uninstall))
}

/// The host API this forum provides. A plugin asks for `api = "^1.2"`: the major must match and
/// the minor must be one the host has, so a plugin that needs a newer op is refused here rather
/// than failing when it calls it. Minor 1: user.get, user.can, settings.get, http_request,
/// email.send, schedule, realtime.push, content.create_topic, i18n.t, time.now, random.bytes.
/// Minor 3 (content apps): `[[routes]]` and `page.request` under `/app/<id>/`, `[[content_types]]`
/// with `search.index`, `review.hold` and `content.review`, `db` offset/count/range filters, `unique`
/// columns, `file.fetch`, 8 MB files with `Range`, and `content.read`/`content.edit` on owned topics.
const HOST_API: &str = "1.3.0";

const FUEL_PER_CALL: u64 = 200_000_000;

const MEMORY_LIMIT: usize = 32 * 1024 * 1024;

const MAX_PACKAGE: usize = 32 * 1024 * 1024;

const MAX_VALUE: usize = 64 * 1024;

const MAX_FAILURES: i32 = 5;

const MAX_HTTP_PER_CALL: u32 = 3;

const HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

const MAX_WRITES_PER_CALL: u32 = 5;

/// `client` lets a package ship browser code, which runs on this site's own origin, outside the
/// sandbox, with the visitor's session: it is the most powerful capability and is shown as such.
pub const CAPABILITIES: &[&str] = &[
    "kv",
    "api_route",
    "tables",
    "content.read",
    "content.write",
    "jobs",
    "notify",
    "login",
    "client",
    "users.read",
    "settings.read",
    "email",
    "realtime",
    "profile_fields",
    "groups",
    "permissions",
    "badges",
    "notification_kinds",
    "reactions",
    "flag_reasons",
    "settings",
    "api_scopes",
    "tags",
    "nodes",
    "meta",
    "files",
    "pages",
    "content_types",
];

///
/// Action hooks (fire and forget, payload below; each is also a name a plugin may list):
/// - `post.created` / `topic.created`: `{topic_id, post_number, title, url, author, node_id}`
/// - `post.edited`: `{topic_id, post_number, editor, reverted?, remote?}`
/// - `topic.moved`: `{topic_id, from, to}`
/// - `post.deleted`: `{topic_id, post_number, actor}`; `topic.deleted`: `{topic_id, actor}`
/// - `user.registered`: `{user_id, username, method?}` (method is the provider for external sign-ups)
/// - `user.login`: `{user_id, method}` (password, magic_link, 2fa, passkey, or a provider key);
///   `user.logout`: `{user_id, username}`
/// - `user.password_changed`: `{user_id, username?}`; `user.email_changed`: `{user_id}`
/// - `profile.updated`: `{user_id, username}`
/// - `user.erased`: `{user_id}` (GDPR erasure; awaited before the data is removed, so purge by user id)
/// - `reaction.changed`: `{user_id, post_id, reaction, on}` (reaction is "like" or the reaction key)
/// - `vote.cast`: `{user_id, target_type: "post"|"topic", target_id, topic_id, value}`
/// - `poll.voted`: `{user_id, poll_id, topic_id, options?, withdrawn}`
/// - `flag.created`: `{user_id, post_id, topic_id, reason, score}`
/// - `moderation.action`: `{action, actor, target_type, target_id, detail}`
/// - `upload.created`: `{user_id, name, key, size, is_image}`
/// - `follow.changed`: `{user_id, target_type: "topic"|"forum"|"user", target_id, level?|on?}`
/// - `badge.awarded`: `{user_id, badge}` (badge is its name); `trust.changed`: `{user_id, to, by}`
/// - `chat.message`: `{channel, user_id, message_id, kind}`;
///   `message.created`: `{from_id, to_ids, conversation_id, new}`
///
/// Filters (`post.before_create`, `item.before_create`, `search.query`, `user.before_register`) may
/// answer `{reject}` or `{data}`; see `hooks.rs`.
pub const HOOKS: &[&str] = &[
    "user.can",
    "signup.validate",
    "login.begin",
    "login.callback",
    "member.filter",
    "api.serialize",
    "post.after_cook",
    "sitemap.urls",
    "panel.render",
    "widget.render",
    "job.run",
    "post.before_create",
    "post.before_cook",
    "item.before_create",
    "user.before_register",
    "search.query",
    "post.created",
    "topic.created",
    "post.edited",
    "topic.moved",
    "user.registered",
    "api.request",
    "post.deleted",
    "topic.deleted",
    "user.login",
    "user.logout",
    "user.password_changed",
    "user.email_changed",
    "profile.updated",
    "user.erased",
    "reaction.changed",
    "vote.cast",
    "poll.voted",
    "flag.created",
    "moderation.action",
    "upload.created",
    "follow.changed",
    "badge.awarded",
    "trust.changed",
    "chat.message",
    "message.created",
    "slot.render",
    "onebox.resolve",
    "action.run",
    "cook.rule",
    "admin.form.save",
    "page.request",
    "content.review",
];

/// Outbound HTTP is granted per host: `http:api.example.com`.
const HTTP_PREFIX: &str = "http:";

/// Origins the plugin's browser code may contact: `connect:https://api.example.com`. They are
/// added to the page's `connect-src`, so each one is approved like any other capability.
pub(crate) const CONNECT_PREFIX: &str = "connect:";

fn known_capability(c: &str) -> bool {
    CAPABILITIES.contains(&c)
        || c.strip_prefix(HTTP_PREFIX).is_some_and(valid_http_host)
        || c.strip_prefix(CONNECT_PREFIX).is_some_and(valid_connect_origin)
}

/// A bare origin in its canonical form: https or wss, a host, an optional port, no path, no
/// credentials, no wildcard.
fn valid_connect_origin(o: &str) -> bool {
    reqwest::Url::parse(o).is_ok_and(|u| {
        matches!(u.scheme(), "https" | "wss")
            && u.host_str().is_some_and(valid_http_host)
            && u.username().is_empty()
            && u.password().is_none()
            && u.path() == "/"
            && u.query().is_none()
            && u.fragment().is_none()
            && u.origin().ascii_serialization() == o
    })
}

fn valid_http_host(h: &str) -> bool {
    !h.is_empty() && h.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
}

/// Exact host match only, on the parsed URL, so `api.example.com.evil.test` or
/// `https://api.example.com@evil.test/` cannot pass as the approved host.
fn http_allowed(hosts: &[String], raw: &str) -> bool {
    reqwest::Url::parse(raw)
        .is_ok_and(|u| u.username().is_empty() && u.host_str().is_some_and(|h| hosts.iter().any(|a| a == h)))
}

/// Also a hook: runs every ten minutes.
pub const SCHEDULED: &str = "scheduled.tick";

/// A first-party plugin compiled into the binary. It is trusted code, so it has no
/// sandbox and no approval step; a crate adds one with `register_native` before the server starts.
pub trait Plugin: Send + Sync {
    fn id(&self) -> &str;
    fn hooks(&self) -> Vec<String>;
    /// Same contract as a module's `handle`; runs on a blocking thread.
    fn handle(&self, hook: &str, data: &Value) -> Result<Value, String>;
}

static NATIVE: LazyLock<RwLock<Vec<Arc<dyn Plugin>>>> = LazyLock::new(Default::default);

pub fn register_native(plugin: Arc<dyn Plugin>) {
    NATIVE.write().unwrap_or_else(std::sync::PoisonError::into_inner).push(plugin);
}

enum Code {
    Wasm(Module),
    Native(Arc<dyn Plugin>),
}

/// A page a plugin adds to one area of the site, drawn by its `panel.render` hook.
#[derive(Clone, serde::Serialize, Deserialize)]
struct Panel {
    area: String,
    key: String,
    /// English; may be left out when `label_key` names it in `locales/en.json`.
    #[serde(default)]
    label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    label_key: Option<String>,
}

/// "content" is a public, indexed page: with the plugin's tables and `sitemap.urls` it is how a
/// plugin adds a content type. "login" is a sign-in button, and needs the `login` capability.
const PANEL_AREAS: [&str; 6] = ["settings", "modcp", "profile", "admin", "content", "login"];

struct Loaded {
    id: String,
    code: Code,
    capabilities: Vec<String>,
    hooks: Vec<String>,
    settings: Value,
    tables: Arc<Vec<plugins::TableSpec>>,
    panels: Vec<Panel>,
    /// Pages the plugin serves under `/app/<id>/` (`[[routes]]`).
    routes: Vec<apps::RouteDecl>,
    /// Kinds of entry it keeps (`[[content_types]]`).
    types: Vec<content_types::ContentTypeDecl>,
    /// The plugin's own message catalogs from `locales/<lang>.json`: `{lang: {key: text}}`.
    catalogs: Arc<Value>,
}

#[expect(
    clippy::expect_used,
    reason = "startup: the engine config is a constant, so this fails only if wasmtime cannot run at all"
)]
static ENGINE: LazyLock<Engine> = LazyLock::new(|| {
    let mut c = Config::new();
    c.consume_fuel(true);
    c.epoch_interruption(true);
    let engine = Engine::new(&c).expect("wasm engine");
    // Fuel counts instructions, not time: a module that waits in host calls is stopped by the
    // wall-clock deadline (`CALL_DEADLINE`) instead, counted in these ticks.
    let ticker = engine.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(EPOCH_TICK);
            ticker.increment_epoch();
        }
    });
    engine
});

const EPOCH_TICK: std::time::Duration = std::time::Duration::from_millis(100);

/// Ticks of `EPOCH_TICK` one call may take in all: host calls (database, HTTP) wait outside
/// the fuel count, and a call past this is stopped when it next returns to the module.
const CALL_DEADLINE: u64 = 150;

/// Largest table a module may grow, in elements.
const TABLE_ELEMENTS: usize = 10_000;

static ACTIVE: LazyLock<RwLock<Arc<Vec<Arc<Loaded>>>>> = LazyLock::new(Default::default);

/// Client code of enabled plugins, by hash-named file.
static CLIENTS: LazyLock<RwLock<HashMap<String, Arc<String>>>> = LazyLock::new(Default::default);

/// Files of enabled plugins under `/plugins/<id>.<hash>/…`: style sheets (already in the
/// `plugin` cascade layer) and `assets/`, with their media type.
#[expect(clippy::type_complexity, reason = "a private table keyed by plugin and path")]
static ASSETS: LazyLock<RwLock<HashMap<String, HashMap<String, (String, Arc<Vec<u8>>)>>>> =
    LazyLock::new(Default::default);

/// The content types an enabled plugin declared; none for an unknown or disabled one.
fn types_of(plugin: &str) -> Vec<content_types::ContentTypeDecl> {
    ACTIVE
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .find(|p| p.id == plugin)
        .map(|p| p.types.clone())
        .unwrap_or_default()
}

/// Enabled plugins that may offer entries to site search.
pub(crate) fn searchable_plugins() -> Vec<String> {
    ACTIVE
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .filter(|p| p.capabilities.iter().any(|c| c == "content_types") && p.types.iter().any(|t| t.search))
        .map(|p| p.id.clone())
        .collect()
}
