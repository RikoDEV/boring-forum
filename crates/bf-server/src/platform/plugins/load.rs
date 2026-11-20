//! Loading enabled, approved plugins into the in-memory registry.

use crate::http::render::DbResult;
use crate::state::AppState;
use bf_core::AppError;
use bf_db::platform::plugins;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use wasmtime::Module;

use super::extras::Activation;
use super::{ACTIVE, CLIENTS, CONNECT_PREFIX, Code, ENGINE, Loaded, NATIVE, Panel, schedule};

/// Loads enabled, approved plugins: compiled modules, slots and client code.
pub async fn reload(state: &AppState) -> Result<(), AppError> {
    let pool = state.db()?;
    let rows = plugins::all(pool).await.db()?;
    let mut active = Vec::new();
    let mut slots: HashMap<String, String> = HashMap::new();
    let mut clients = HashMap::new();
    let mut scripts = Vec::new();
    let mut nav = Vec::new();
    let mut panel_links = Vec::new();
    let mut extra = Activation::default();
    let mut connect: Vec<String> = Vec::new();
    let site = json!({ "site_name": state.settings().str("site_name") });
    for p in rows.into_iter().filter(|p| p.enabled && p.approved) {
        for (name, src) in p.slots.as_object().into_iter().flatten() {
            match src.as_str().map(|s| bf_web::theme::render_slot(s, &site)) {
                Some(Ok(html)) if bf_web::theme::SLOTS.contains(&name.as_str()) => {
                    slots.entry(name.clone()).or_default().push_str(&html);
                }
                // A failing fragment renders nothing.
                Some(Err(e)) => tracing::warn!(plugin = %p.id, slot = %name, error = %e, "plugin slot failed"),
                _ => {}
            }
        }
        // Links are checked like any other navigation link.
        for link in p
            .schema
            .as_ref()
            .and_then(|s| s.get("x-nav"))
            .map(|v| bf_core::nav::parse(&v.to_string()))
            .unwrap_or_default()
        {
            if link.key.is_none() && link.group.is_none() && link.groups.is_empty() {
                nav.push((link.url, link.label));
            }
        }
        extra.add(&p.id, p.schema.as_ref(), &p.hooks);
        let tables: Vec<plugins::TableSpec> = schema_list(p.schema.as_ref(), "x-tables");
        let panels: Vec<Panel> = schema_list(p.schema.as_ref(), "x-panels");
        let can_login = p.capabilities.iter().any(|c| c == "login") && p.hooks.iter().any(|h| h == "login.begin");
        for x in &panels {
            if x.area == "login" {
                if can_login {
                    panel_links.push(("login".to_owned(), format!("/auth/p/{}/{}", p.id, x.key), x.label.clone()));
                }
            } else if p.hooks.iter().any(|h| h == "panel.render") {
                panel_links.push((x.area.clone(), format!("/plugin/{}/{}", p.id, x.key), x.label.clone()));
                if x.area == "content" {
                    nav.push((format!("/plugin/{}/{}", p.id, x.key), x.label.clone()));
                }
            }
        }
        // Browser code runs on this site's origin, outside the sandbox: only with the capability.
        let may_client = p.capabilities.iter().any(|c| c == "client");
        connect.extend(p.capabilities.iter().filter_map(|c| c.strip_prefix(CONNECT_PREFIX)).map(str::to_owned));
        let catalogs =
            Arc::new(p.schema.as_ref().and_then(|s| s.get("x-locales")).cloned().unwrap_or_else(|| json!({})));
        if let Some(js) = p.client_js.filter(|_| may_client) {
            let hash: String = Sha256::digest(js.as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect();
            let file = format!("{}.{hash}.js", p.id);
            let islands = p
                .schema
                .as_ref()
                .and_then(|s| s.get("x-islands"))
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            scripts.push((format!("/plugins/{file}"), islands));
            clients.insert(file, Arc::new(js));
        }
        if let Some(bytes) = p.module {
            let id = p.id.clone();
            let compiled =
                tokio::task::spawn_blocking(move || Module::new(&ENGINE, bytes)).await.map_err(AppError::internal)?;
            match compiled {
                Ok(module) => active.push(Arc::new(Loaded {
                    id,
                    code: Code::Wasm(module),
                    capabilities: p.capabilities,
                    hooks: p.hooks,
                    settings: super::forms::with_defaults(p.settings, p.schema.as_ref()),
                    tables: Arc::new(tables),
                    panels,
                    routes: schema_list(p.schema.as_ref(), "x-routes"),
                    types: schema_list(p.schema.as_ref(), "x-content-types"),
                    catalogs,
                })),
                Err(e) => tracing::error!(plugin = %id, error = %e, "plugin failed to compile"),
            }
        }
    }
    for n in NATIVE.read().unwrap_or_else(std::sync::PoisonError::into_inner).iter() {
        active.push(Arc::new(Loaded {
            id: n.id().to_owned(),
            code: Code::Native(n.clone()),
            capabilities: vec![],
            hooks: n.hooks(),
            settings: json!({}),
            tables: Arc::default(),
            panels: vec![],
            routes: vec![],
            types: vec![],
            catalogs: Arc::default(),
        }));
    }
    *ACTIVE.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(active);
    *CLIENTS.write().unwrap_or_else(std::sync::PoisonError::into_inner) = clients;
    bf_web::theme::set_plugin_output(slots, scripts, nav, panel_links);
    extra.publish(state);
    connect.sort();
    connect.dedup();
    bf_web::security::set_plugin_connect(connect);
    schedule::ensure_started(state);
    super::registry::sync_all(state).await;
    state.microcache.clear();
    Ok(())
}

pub(super) fn schema_list<T: serde::de::DeserializeOwned>(schema: Option<&Value>, key: &str) -> Vec<T> {
    schema.and_then(|s| s.get(key)).and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
}
