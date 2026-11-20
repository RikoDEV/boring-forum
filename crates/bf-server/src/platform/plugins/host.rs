//! The functions a WebAssembly module may call, each behind a capability, and the limits on what they do.

use crate::http::render::pool;
use crate::state::AppState;
use bf_core::i18n::en;
use bf_db::platform::plugins;
use serde_json::{Value, json};
use std::sync::Arc;
use wasmtime::{Caller, Linker, StoreLimits};

use super::ops::failed;
use super::stats::{self, allow};
use super::{ENGINE, HTTP_TIMEOUT, MAX_HTTP_PER_CALL, MAX_VALUE, MAX_WRITES_PER_CALL, cook, http_allowed, ops, ops_content, write_out};

pub(super) struct HostState {
    pub(super) limits: StoreLimits,
    pub(super) plugin: String,
    pub(super) state: Option<AppState>,
    pub(super) capabilities: Vec<String>,
    pub(super) tables: Arc<Vec<plugins::TableSpec>>,
    pub(super) catalogs: Arc<Value>,
    pub(super) writes: u32,
    pub(super) can_kv: bool,
    pub(super) http_hosts: Vec<String>,
    pub(super) http_calls: u32,
    pub(super) pool: sqlx::PgPool,
    pub(super) handle: tokio::runtime::Handle,
}

pub(super) fn read_bytes(caller: &mut Caller<'_, HostState>, ptr: i32, len: i32) -> Option<Vec<u8>> {
    let memory = caller.get_export("memory")?.into_memory()?;
    let (start, len) = (usize::try_from(ptr).ok()?, usize::try_from(len).ok()?);
    if len > MAX_VALUE {
        return None;
    }
    memory.data(&*caller).get(start..start.checked_add(len)?).map(<[u8]>::to_vec)
}

fn read_text(caller: &mut Caller<'_, HostState>, ptr: i32, len: i32) -> Option<String> {
    read_bytes(caller, ptr, len).map(|b| String::from_utf8_lossy(&b).into_owned())
}

pub(super) fn linker() -> Result<Linker<HostState>, wasmtime::Error> {
    let mut l = Linker::new(&ENGINE);
    l.func_wrap("bf", "log", |mut c: Caller<'_, HostState>, ptr: i32, len: i32| {
        if let Some(b) = read_bytes(&mut c, ptr, len) {
            let message = String::from_utf8_lossy(&b);
            tracing::info!(plugin = %c.data().plugin, message = %message, "plugin log");
            stats::log(&c.data().plugin, &message);
        }
    })?;
    link_key_value(&mut l)?;
    link_request(&mut l)?;
    link_http_get(&mut l)?;
    Ok(l)
}

/// The plugin's private key-value store, behind the `kv` capability.
fn link_key_value(l: &mut Linker<HostState>) -> Result<(), wasmtime::Error> {
    // Returns (ptr << 32 | len) of the value, 0 when there is none, -1 when not allowed.
    l.func_wrap("bf", "kv_get", |mut c: Caller<'_, HostState>, ptr: i32, len: i32| -> Result<i64, wasmtime::Error> {
        if !c.data().can_kv {
            return Ok(-1);
        }
        let Some(key) = read_text(&mut c, ptr, len) else { return Ok(-1) };
        let (pool, plugin, handle) = (c.data().pool.clone(), c.data().plugin.clone(), c.data().handle.clone());
        let Ok(Some(value)) = handle.block_on(plugins::kv_get(&pool, &plugin, &key)) else { return Ok(0) };
        write_out(&mut c, value.as_bytes())
    })?;
    l.func_wrap("bf", "kv_set", |mut c: Caller<'_, HostState>, kp: i32, kl: i32, vp: i32, vl: i32| -> i32 {
        if !c.data().can_kv {
            return -1;
        }
        let (Some(key), Some(value)) = (read_text(&mut c, kp, kl), read_text(&mut c, vp, vl)) else { return -1 };
        let (pool, plugin, handle) = (c.data().pool.clone(), c.data().plugin.clone(), c.data().handle.clone());
        if let Some(state) = c.data().state.clone()
            && handle.block_on(storage_check(&state, &plugin)).is_err()
        {
            return -1;
        }
        i32::from(handle.block_on(plugins::kv_set(&pool, &plugin, &key, &value)).is_ok()) - 1
    })?;
    l.func_wrap("bf", "kv_delete", |mut c: Caller<'_, HostState>, ptr: i32, len: i32| -> i32 {
        if !c.data().can_kv {
            return -1;
        }
        let Some(key) = read_text(&mut c, ptr, len) else { return -1 };
        let (pool, plugin, handle) = (c.data().pool.clone(), c.data().plugin.clone(), c.data().handle.clone());
        i32::from(handle.block_on(plugins::kv_delete(&pool, &plugin, &key)).is_ok()) - 1
    })?;
    // Keys under a prefix, as a JSON array.
    l.func_wrap("bf", "kv_keys", |mut c: Caller<'_, HostState>, ptr: i32, len: i32| -> Result<i64, wasmtime::Error> {
        if !c.data().can_kv {
            return Ok(-1);
        }
        let Some(prefix) = read_text(&mut c, ptr, len) else { return Ok(-1) };
        let (pool, plugin, handle) = (c.data().pool.clone(), c.data().plugin.clone(), c.data().handle.clone());
        let keys = handle.block_on(plugins::kv_keys(&pool, &plugin, &prefix)).unwrap_or_default();
        write_out(&mut c, serde_json::to_string(&keys)?.as_bytes())
    })?;
    Ok(())
}

/// `bf.request`: database, content, job and notify operations, each behind its own capability.
fn link_request(l: &mut Linker<HostState>) -> Result<(), wasmtime::Error> {
    // JSON request, JSON answer: {"ok": ...} or {"error": "..."}; -1 when the capability for
    // this operation was not approved. See `host_request`.
    l.func_wrap("bf", "request", |mut c: Caller<'_, HostState>, ptr: i32, len: i32| -> Result<i64, wasmtime::Error> {
        let Some(bytes) = read_bytes(&mut c, ptr, len) else { return Ok(-1) };
        let Ok(req) = serde_json::from_slice::<Value>(&bytes) else { return Ok(-1) };
        let op = req.get("op").and_then(Value::as_str).unwrap_or_default().to_owned();
        // An empty capability: no approval needed, the operation reaches nothing outside the call.
        let needed = match op.as_str() {
            "db" => "tables",
            "content.read" => "content.read",
            "content.reply" | "content.create_topic" | "content.edit" => "content.write",
            "job" | "schedule" => "jobs",
            "notify" => "notify",
            "user.get" | "user.can" => "users.read",
            "settings.get" => "settings.read",
            "email.send" => "email",
            "realtime.push" => "realtime",
            "profile.set" => "profile_fields",
            "group.add" | "group.remove" => "groups",
            "badge.grant" => "badges",
            "tags.set" => "tags",
            "nodes.create" => "nodes",
            "meta.get" | "meta.set" => "meta",
            "file.put" | "file.get" | "file.delete" | "file.fetch" => "files",
            "search.index" | "search.remove" | "review.hold" | "review.withdraw" | "content.can" => "content_types",
            "time.now" | "random.bytes" | "i18n.t" | "http_request" => "",
            _ => return Ok(-1),
        };
        if !needed.is_empty() && !c.data().capabilities.iter().any(|x| x == needed) {
            return Ok(-1);
        }
        // Operations with no outside effect answer here, even without a database.
        let pure = match op.as_str() {
            "time.now" => Some(json!({ "unix": chrono::Utc::now().timestamp(), "iso": chrono::Utc::now().to_rfc3339() })),
            "random.bytes" => {
                let n = req.get("n").and_then(Value::as_i64).unwrap_or(16).clamp(1, 1024);
                let hex: String = (0..n).map(|_| format!("{:02x}", rand::random::<u8>())).collect();
                Some(json!({ "hex": hex }))
            }
            "i18n.t" => {
                let key = req.get("key").and_then(Value::as_str).unwrap_or_default();
                let locale = req.get("locale").and_then(Value::as_str).unwrap_or("en");
                let text = ops::catalog_text(&c.data().catalogs, locale, key).unwrap_or_else(|| key.to_owned());
                Some(json!({ "text": ops::fill(&text, req.get("vars")) }))
            }
            _ => None,
        };
        if let Some(v) = pure {
            return reply(&mut c, Ok(v));
        }
        let Some(state) = c.data().state.clone() else { return Ok(-1) };
        let (plugin, handle) = (c.data().plugin.clone(), c.data().handle.clone());
        if op == "file.fetch" || op == "http_request" {
            let fetch = op == "file.fetch";
            if c.data().http_calls >= MAX_HTTP_PER_CALL || (fetch && c.data().writes >= MAX_WRITES_PER_CALL) {
                let limit = if fetch { en("Too many outbound requests or writes in one call.") } else { en("Too many outbound requests in one call.") };
                return reply(&mut c, Err(limit.into()));
            }
            c.data_mut().http_calls += 1;
            c.data_mut().writes += u32::from(fetch);
            let hosts = c.data().http_hosts.clone();
            let result = if fetch {
                handle.block_on(ops_content::file_fetch(&state, &plugin, &hosts, &req))
            } else {
                handle.block_on(ops::http_request(&state, &plugin, &hosts, &req))
            };
            return reply(&mut c, result);
        }
        let reads = ["content.read", "user.get", "user.can", "settings.get", "meta.get", "file.get", "content.can"];
        let db_read = op == "db" && matches!(req.get("action").and_then(Value::as_str), Some("select" | "count"));
        if !reads.contains(&op.as_str()) && !db_read {
            if c.data().writes >= MAX_WRITES_PER_CALL {
                return reply(&mut c, Err(en("Too many writes in one call.").into()));
            }
            c.data_mut().writes += 1;
        }
        let (tables, catalogs) = (c.data().tables.clone(), c.data().catalogs.clone());
        reply(&mut c, handle.block_on(host_request(&state, &plugin, &tables, &catalogs, &op, &req)))
    })?;
    Ok(())
}

/// Answers a `bf.request` call: `{"ok": ...}` or `{"error": "..."}`.
fn reply(c: &mut Caller<'_, HostState>, result: Result<Value, String>) -> Result<i64, wasmtime::Error> {
    let answer = result.map_or_else(|e| json!({ "error": e }), |v| json!({ "ok": v }));
    write_out(c, serde_json::to_string(&answer)?.as_bytes())
}

/// `bf.http_get`: outbound GET to the hosts an admin approved.
fn link_http_get(l: &mut Linker<HostState>) -> Result<(), wasmtime::Error> {
    // GET through the SSRF-safe fetcher. Returns the body like kv_get; 0 when the request
    // failed, -1 when this host was not approved or the per-call limit is used up.
    l.func_wrap(
        "bf",
        "http_get",
        |mut c: Caller<'_, HostState>, ptr: i32, len: i32| -> Result<i64, wasmtime::Error> {
            let Some(url) = read_text(&mut c, ptr, len) else { return Ok(-1) };
            if !http_allowed(&c.data().http_hosts, &url) || c.data().http_calls >= MAX_HTTP_PER_CALL {
                return Ok(-1);
            }
            c.data_mut().http_calls += 1;
            let handle = c.data().handle.clone();
            let fetched = handle
                .block_on(tokio::time::timeout(HTTP_TIMEOUT, crate::platform::outbound::fetch_public(&url, MAX_VALUE)));
            match fetched {
                Ok(Ok(body)) => write_out(&mut c, &body),
                _ => Ok(0),
            }
        },
    )?;
    Ok(())
}

/// Refuses more data once a plugin is over `plugin_storage_mb`. If the size cannot be read the
/// write is refused too: the limit protects the whole site's disk.
pub(super) async fn storage_check(state: &AppState, plugin: &str) -> Result<(), String> {
    let full = || en("This plugin has used all its storage.").to_owned();
    let pool = pool(state).map_err(|_| full())?;
    let used = plugins::storage_used(pool, plugin).await.map_err(|_| full())?;
    if used >= state.settings().int("plugin_storage_mb").saturating_mul(1024 * 1024) {
        return Err(full());
    }
    Ok(())
}

/// A topic plugins may see: only what a guest could read, whatever the plugin was asked about.
pub(super) async fn public_topic(state: &AppState, id: i64) -> Result<bf_db::forum::topics::TopicRow, String> {
    let missing = || en("No such topic.").to_owned();
    let t = any_topic(state, id).await?;
    let tree = state.forum.get(pool(state).map_err(|_| missing())?).await.map_err(|_| missing())?;
    let guest = bf_core::perms::Actor { groups: vec![1.into()], ..Default::default() };
    if t.state != "visible" || !tree.perms(&guest, t.node_id).has(bf_core::perms::Perms::READ) {
        return Err(missing());
    }
    Ok(t)
}

/// A topic the plugin owns, whatever its state, unless it is deleted.
pub(super) async fn any_topic(state: &AppState, id: i64) -> Result<bf_db::forum::topics::TopicRow, String> {
    let missing = || en("No such topic.").to_owned();
    let pool = pool(state).map_err(|_| missing())?;
    let t = bf_db::forum::topics::topic_for(pool, id, None).await.map_err(|_| missing())?.ok_or_else(missing)?;
    if t.state == "deleted" {
        return Err(missing());
    }
    Ok(t)
}

pub(super) async fn host_request(
    state: &AppState,
    plugin: &str,
    tables: &[plugins::TableSpec],
    catalogs: &Value,
    op: &str,
    req: &Value,
) -> Result<Value, String> {
    let pool = pool(state).map_err(|_| en("No database.").to_owned())?;
    let topic = req.get("topic").and_then(Value::as_i64).unwrap_or_default();
    match op {
        "db" => {
            if matches!(req.get("action").and_then(Value::as_str), Some("select" | "count"))
                && !allow(plugin, "db_read", 60, state.settings().int("plugin_db_reads_per_minute"), 1)
            {
                return Err(en("This plugin has read too much in the last minute.").into());
            }
            // Deleting stays possible at the limit, so a plugin can make room.
            if matches!(req.get("action").and_then(Value::as_str), Some("insert" | "update")) {
                storage_check(state, plugin).await?;
            }
            plugins::table_op(pool, plugin, tables, req).await
        }
        "content.read" => {
            // A topic the plugin owns (in a forum it declared or created, or one its account
            // opened) is readable whatever its forum's permissions say; any other only as a guest could.
            let owned = match plugins::bot_user(pool, plugin).await {
                Ok(Some(bot)) => bf_db::platform::plugin_content::topic_owned(pool, plugin, bot, topic).await.unwrap_or(false),
                _ => false,
            };
            let t = if owned { any_topic(state, topic).await? } else { public_topic(state, topic).await? };
            let from = req.get("from").and_then(Value::as_i64).unwrap_or(1).clamp(1, i64::from(i32::MAX)) as i32;
            let limit = req.get("limit").and_then(Value::as_i64).unwrap_or(50).clamp(1, 100);
            let posts =
                bf_db::forum::posts::topic_posts(pool, topic, from, limit, false, None).await.map_err(failed)?;
            let posts: Vec<Value> = posts
                .iter()
                .map(|p| json!({ "number": p.post_number, "author": p.author, "html": p.cooked, "created_at": p.created_at }))
                .collect();
            Ok(json!({ "id": t.id, "title": t.title, "slug": t.slug, "locked": t.locked, "state": t.state, "node_id": t.node_id, "owned": owned, "posts": posts }))
        }
        // Posts as the plugin's own account. Watchers are told, but plugin post.created hooks do
        // not run for these, so two plugins cannot keep answering each other forever.
        "content.reply" => {
            let body = req.get("body").and_then(Value::as_str).unwrap_or_default();
            if body.trim().is_empty() || body.chars().count() > 20_000 {
                return Err(en("The body must be 1 to 20000 characters.").into());
            }
            let t = public_topic(state, topic).await?;
            let tree = state.forum.get(pool).await.map_err(|_| en("No forum.").to_owned())?;
            let archived = tree.node(t.node_id).is_some_and(|n| n.kind == "archive");
            if t.locked || t.archived || archived {
                return Err(en("This topic is closed.").into());
            }
            let bot =
                plugins::bot_user(pool, plugin).await.map_err(failed)?.ok_or(en("This plugin has no account."))?;
            // The plugin's account must itself be allowed to reply in that forum, as `create_topic`
            // checks it may create there (a forum closed to its group stays closed to the plugin).
            let actor = bf_core::perms::Actor { user_id: Some(bot.into()), groups: vec![2.into()], ..Default::default() };
            let perms = tree.perms(&actor, t.node_id);
            if !perms.has(bf_core::perms::Perms::READ) || !perms.has(bf_core::perms::Perms::REPLY) {
                return Err(en("This plugin cannot reply in that forum.").into());
            }
            // Watchers get these by email, so a busy plugin must not be able to flood them.
            if plugins::posts_last_hour(pool, bot).await.map_err(failed)?
                >= state.settings().int("plugin_posts_per_hour")
            {
                return Err(en("This plugin has posted too much in the last hour.").into());
            }
            let cooked = cook(state, body).await;
            let post = bf_db::forum::posts::NewPost {
                author_id: bot,
                raw: body,
                cooked: &cooked.html,
                cooked_version: bf_cook::COOK_VERSION,
                ip: None,
                pending: false,
            };
            let number = bf_db::forum::posts::create_reply(pool, topic, t.node_id, &post).await.map_err(failed)?;
            bf_db::forum::links::record_links(pool, topic, number, &cooked.links).await.map_err(failed)?;
            state.forum.invalidate();
            let by = plugins::bot_username(plugin);
            crate::forum::posting::after_plugin_post(state, bot, &by, &t.title, &t.slug, topic, t.node_id, number)
                .await;
            Ok(json!({ "number": number }))
        }
        "job" => {
            let delay = req.get("delay").and_then(Value::as_i64).unwrap_or(0).clamp(0, 86_400);
            let payload = req.get("payload").cloned().unwrap_or_else(|| json!({}));
            if !allow(plugin, "job", 60, state.settings().int("plugin_jobs_per_minute"), 1) {
                return Err(en("This plugin has queued too many jobs in the last minute.").into());
            }
            if plugins::enqueue_job(pool, plugin, &payload, delay).await.map_err(failed)? {
                Ok(json!({ "queued": true }))
            } else {
                Err(en("Too many jobs are waiting.").into())
            }
        }
        "notify" => {
            let ids: Vec<i64> = ops::list(req, "user_ids");
            let text = req.get("text").and_then(Value::as_str).unwrap_or_default();
            let url = req.get("url").and_then(Value::as_str).unwrap_or("/notifications");
            if ids.is_empty() || ids.len() > 50 || text.trim().is_empty() || text.chars().count() > 200 {
                return Err(en("Send 1 to 50 user_ids and a text of up to 200 characters.").into());
            }
            if !local_path(url) {
                return Err(en("The url must be a path on this site.").into());
            }
            let cost = i64::try_from(ids.len()).unwrap_or(50);
            if !allow(plugin, "notify", 3600, state.settings().int("plugin_notifications_per_hour"), cost) {
                return Err(en("This plugin has sent too many notifications in the last hour.").into());
            }
            // A kind the plugin declared (`[[notification_kinds]]`) follows members' own settings.
            let kind = match req.get("kind").and_then(Value::as_str) {
                Some(k) => {
                    let full = format!("plugin:{plugin}:{k}");
                    if bf_core::notifications::find(&full).is_none() {
                        return Err(en("That notification kind is not declared by this plugin.").into());
                    }
                    full
                }
                None => "plugin".to_owned(),
            };
            let data = json!({ "text": text, "url": url, "plugin": plugin });
            // Through the same delivery as the site's own kinds: the member's channel choices,
            // quiet hours and ignore list apply, and an open page hears about it live.
            let base = &state.config.base_url;
            let body = format!("{text}

{base}{url}
");
            let delay = state.settings().int("email_delay_minutes") as i32;
            let fresh = bf_db::community::notifications::deliver(pool, base, 0, &ids, &kind, &data, text, &body, delay)
                .await
                .map_err(failed)?;
            for id in fresh {
                state.hub.publish(&format!("user:{id}"), "notification", data.clone());
            }
            Ok(json!({ "sent": ids.len() }))
        }
        other => ops::more(state, plugin, catalogs, other, req).await.unwrap_or_else(|| Err(en("Unknown operation.").into())),
    }
}

/// On this site for certain: rules out `//host`, backslash tricks and control characters.
pub(crate) fn local_path(p: &str) -> bool {
    p.starts_with('/') && !p.starts_with("//") && !p.contains('\\') && !p.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_on_site() {
        assert!(local_path("/latest"));
        for bad in ["//evil.test", "/\\evil.test", "https://evil.test", "/a\nb", ""] {
            assert!(!local_path(bad), "{bad}");
        }
    }
}
