//! The entry points the rest of the site uses to ask plugins things: filters, renderers, jobs and fire-and-forget actions.

use crate::state::AppState;
use bf_core::AppError;
use bf_db::platform::plugins;
use serde_json::{Value, json};

use super::{call, listening, local_path};

/// `post.before_create`: each plugin may change the text or refuse it. A plugin that fails is
/// skipped, because a bug in an add-on must not stop people posting.
pub(crate) async fn before_post(state: &AppState, author: i64, raw: &str) -> Result<String, AppError> {
    let mut text = raw.to_owned();
    for hook in ["post.before_create", "post.before_cook"] {
        for p in listening(hook) {
            let data = json!({ "raw": text, "author_id": author });
            if let Some(out) = call(state, p, hook, &data).await {
                if let Some(reason) = out.get("reject").and_then(Value::as_str) {
                    return Err(AppError::Validation(reason.chars().take(300).collect()));
                }
                if let Some(new) = out.pointer("/data/raw").and_then(Value::as_str) {
                    new.clone_into(&mut text);
                }
            }
        }
    }
    Ok(text)
}

/// The text filters for content a system makes (bot, automation, federation, importer, mail
/// in): `post.before_create` and `post.before_cook` rewrite the text, and a refusal returns
/// `None`, so the caller drops that post instead of showing an error to anyone.
pub(crate) async fn before_post_system(state: &AppState, author: i64, raw: &str) -> Option<String> {
    if listening("post.before_create").is_empty() && listening("post.before_cook").is_empty() {
        return Some(raw.to_owned());
    }
    before_post(state, author, raw).await.ok()
}

/// A filter over one text field: `item.before_create` (title), `search.query` (q).
pub(crate) async fn filter_text(state: &AppState, hook: &str, field: &str, value: &str) -> Result<String, AppError> {
    let mut text = value.to_owned();
    for p in listening(hook) {
        if let Some(out) = call(state, p, hook, &json!({ field: text })).await {
            if let Some(reason) = out.get("reject").and_then(Value::as_str) {
                return Err(AppError::Validation(reason.chars().take(300).collect()));
            }
            if let Some(new) = out.get("data").and_then(|d| d.get(field)).and_then(Value::as_str) {
                text = new.to_owned();
            }
        }
    }
    Ok(text)
}

/// `user.before_register`: a plugin's message if it refuses this registration.
pub(crate) async fn registration_refused(state: &AppState, username: &str, email: &str) -> Option<String> {
    for p in listening("user.before_register") {
        if let Some(out) =
            call(state, p, "user.before_register", &json!({ "username": username, "email": email })).await
            && let Some(reason) = out.get("reject").and_then(Value::as_str)
        {
            return Some(reason.chars().take(300).collect());
        }
    }
    None
}

/// The cooker plus `post.after_cook` (`cook_extension`, `onebox_provider`): plugins may change
/// the HTML, which goes through the post sanitizer again, so they cannot add what a post cannot.
pub(crate) async fn cook(state: &AppState, raw: &str) -> bf_cook::Cooked {
    let mut cooked = bf_cook::cook(raw);
    super::extras::finish_callbacks(state, &mut cooked).await;
    cooked.html = after_cook(state, std::mem::take(&mut cooked.html)).await;
    cooked
}

pub(crate) async fn after_cook(state: &AppState, mut html: String) -> String {
    for p in listening("post.after_cook") {
        if let Some(out) = call(state, p, "post.after_cook", &json!({ "html": html })).await
            && let Some(new) = out.pointer("/data/html").and_then(Value::as_str)
        {
            html = bf_cook::sanitize(new);
        }
    }
    html
}

/// `sitemap.urls` (`sitemap_source`): extra paths on this site, each a path or
/// `{"path": "/app/x/entries/a", "lastmod": "2026-11-01T10:00:00Z"}`. A plugin that keeps a
/// content type lists its public entries here; up to 5000 a plugin.
pub(crate) async fn sitemap_paths(state: &AppState) -> Vec<(String, Option<chrono::DateTime<chrono::Utc>>)> {
    let mut paths = Vec::new();
    for p in listening("sitemap.urls") {
        if let Some(out) = call(state, p, "sitemap.urls", &json!({})).await {
            let list = out.get("urls").and_then(Value::as_array).cloned().unwrap_or_default();
            paths.extend(
                list.iter()
                    .filter_map(|u| match u {
                        Value::String(s) => Some((s.as_str(), None)),
                        Value::Object(o) => Some((
                            o.get("path").and_then(Value::as_str)?,
                            o.get("lastmod")
                                .and_then(Value::as_str)
                                .and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok())
                                .map(|d| d.with_timezone(&chrono::Utc)),
                        )),
                        _ => None,
                    })
                    .filter(|(u, _)| local_path(u))
                    .take(5000)
                    .map(|(u, at)| (u.to_owned(), at)),
            );
        }
    }
    paths
}

/// `widget.render` (`widget`). Widgets are cached for everyone, so no viewer is passed.
pub(crate) async fn render_widget(state: &AppState, plugin: &str, key: &str, settings: &Value) -> String {
    let Some(p) = listening("widget.render").into_iter().find(|p| p.id == plugin) else { return String::new() };
    let out = call(state, p, "widget.render", &json!({ "key": key, "settings": settings })).await;
    out.and_then(|o| o.get("html").and_then(Value::as_str).map(bf_web::theme::clean_html)).unwrap_or_default()
}

/// `api.serialize` (`serializer_fields`): fields plugins add to an API answer, each under
/// its plugin's id so they cannot change or hide what the core returns. Costs nothing unless a
/// plugin listens.
pub(crate) async fn extra_fields(state: &AppState, kind: &str, id: i64) -> Option<Value> {
    let plugins = listening("api.serialize");
    if plugins.is_empty() {
        return None;
    }
    let mut all = serde_json::Map::new();
    for p in plugins {
        let name = p.id.clone();
        if let Some(out) = call(state, p, "api.serialize", &json!({ "kind": kind, "id": id })).await
            && let Some(fields) = out.get("fields")
        {
            all.insert(name, fields.clone());
        }
    }
    Some(Value::Object(all))
}

/// Plugins that add fields to API answers.
pub(crate) fn has_serializers() -> bool {
    !listening("api.serialize").is_empty()
}

/// `api.serialize` on an answer already built: the plugins' fields go under `plugins`.
/// `kind` is `user`, `topic`, `post` or `topic_state`; `id` is that thing's id.
pub(crate) async fn add_plugin_fields(state: &AppState, kind: &str, id: i64, answer: &mut Value) {
    // Metadata plugins declared `public` (`[[meta]]`): under `meta.<plugin>.<key>`.
    if matches!(kind, "topic" | "post" | "user")
        && let Some(pool) = state.pool.as_ref()
        && let Ok(meta) = bf_db::platform::registry_content::public_meta(pool, kind, id).await
        && meta.as_object().is_some_and(|m| !m.is_empty())
    {
        answer["meta"] = meta;
    }
    if let Some(extra) = extra_fields(state, kind, id).await {
        answer["plugins"] = extra;
    }
}

/// Runs queued jobs. Without a plugin listening it costs no query. Three attempts, then dropped.
pub fn spawn_jobs(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(bf_core::scaled(std::time::Duration::from_secs(5)));
        loop {
            tick.tick().await;
            // Jobs call plugins, which may mail or post: paused while the site is offline.
            if state.settings().bool("offline") {
                continue;
            }
            let listeners = listening("job.run");
            let (Some(pool), false) = (state.pool.clone(), listeners.is_empty()) else { continue };
            let ids: Vec<String> = listeners.iter().map(|p| p.id.clone()).collect();
            let jobs = match plugins::due_jobs(&pool, &ids, 20).await {
                Ok(j) => j,
                Err(e) => {
                    tracing::warn!(error = %e, "plugin jobs query failed");
                    continue;
                }
            };
            for job in jobs {
                let Some(p) = listeners.iter().find(|p| p.id == job.plugin).cloned() else { continue };
                let done = call(&state, p, "job.run", &json!({ "payload": job.payload })).await.is_some();
                let saved = if done || job.attempts >= 2 {
                    plugins::finish_job(&pool, job.id).await
                } else {
                    plugins::retry_job(&pool, job.id, job.attempts + 1).await
                };
                if let Err(e) = saved {
                    tracing::warn!(error = %e, "plugin job update failed");
                }
            }
        }
    });
}

/// Actions run in the background, so a slow plugin never delays the request.
pub(crate) fn dispatch(state: &AppState, hook: &str, data: &Value) {
    for p in listening(hook) {
        let (state, hook, data) = (state.clone(), hook.to_owned(), data.clone());
        tokio::spawn(async move {
            call(&state, p, &hook, &data).await;
        });
    }
}

/// Like `dispatch`, but returns once every listening plugin has been called, for events where the
/// data goes away right after (`user.erased`): a plugin gets to purge what it keeps for that user.
pub(crate) async fn dispatch_wait(state: &AppState, hook: &str, data: &Value) {
    for p in listening(hook) {
        call(state, p, hook, data).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::plugins::{Plugin, register_native, reload};
    use std::sync::Arc;

    struct Filter;
    impl Plugin for Filter {
        fn id(&self) -> &str {
            "native-filter"
        }
        fn hooks(&self) -> Vec<String> {
            vec!["post.before_create".into()]
        }
        fn handle(&self, _hook: &str, data: &Value) -> Result<Value, String> {
            let raw = data["raw"].as_str().unwrap_or_default();
            Ok(json!({ "data": { "raw": format!("{raw} [native]") } }))
        }
    }

    /// The live path: registered, loaded by `reload`, and run by the filter chain. Needs a
    /// database: BF_DATABASE_URL=... cargo test -p bf-server --lib native_plugin -- --ignored
    #[tokio::test]
    #[ignore]
    async fn native_plugin_runs_through_the_hook_chain() {
        let config = bf_core::config::Config::from_env().unwrap();
        let pool = bf_db::connect(&config.database_url, 2).await.unwrap();
        bf_db::migrate(&pool).await.unwrap();
        let state = AppState::new(config, pool).await.unwrap();
        register_native(Arc::new(Filter));
        reload(&state).await.unwrap();
        assert_eq!(before_post(&state, 1, "hello").await.unwrap(), "hello [native]");
    }
}
