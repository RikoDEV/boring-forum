//! Admin endpoints that install, approve, configure and remove plugins, and the `plugin new` scaffold.

use crate::admin::api::admin;
use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Multipart, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use bf_core::AppError;
use bf_core::i18n::en;
use bf_db::platform::plugins;
use serde::Deserialize;
use serde_json::{Value, json};
use wasmtime::Module;

use super::version::{compare, satisfies, split_requirement};
use super::{ENGINE, MAX_PACKAGE, read_package, reload, schema_list, stats};

/// The installed plugins with what the admin page shows: which capabilities are new since the
/// last approval, which required plugins are missing, and the running statistics.
pub(super) async fn list(State(state): State<AppState>, v: Viewer, headers: HeaderMap) -> Result<Response, AppError> {
    admin(&state, &v, &headers, false)?;
    let all = plugins::summaries(state.db()?).await.db()?;
    let mut rows: Vec<Value> = all
        .iter()
        .map(|p| {
            let new_caps: Vec<&String> = p.capabilities.iter().filter(|c| !p.approved_capabilities.contains(c)).collect();
            let mut row = serde_json::to_value(p).unwrap_or_default();
            row["new_capabilities"] = json!(new_caps);
            row["requires"] = json!(requirements(p.schema.as_ref()));
            row["requires_unmet"] = json!(unmet(&all, p.schema.as_ref()));
            row["stats"] = stats::snapshot(&p.id);
            row
        })
        .collect();
    for row in &mut rows {
        let id = row["id"].as_str().unwrap_or_default().to_owned();
        let perms = bf_db::platform::registry_groups::permissions_of(state.db()?, &id).await.db()?;
        row["permissions"] = json!(perms.into_iter().map(|(key, label, groups)| json!({ "key": key, "label": label, "groups": groups })).collect::<Vec<_>>());
    }
    Ok(Json(rows).into_response())
}

#[derive(Deserialize)]
pub(super) struct Grant {
    pub(super) groups: Vec<i64>,
}

/// Which groups hold one of the plugin's declared permissions.
pub(super) async fn grant_permission(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path((id, key)): Path<(String, String)>,
    Json(b): Json<Grant>,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, true)?;
    if !bf_db::platform::registry_groups::grant_permission(state.db()?, &id, &key, &b.groups).await.db()? {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

fn requirements(schema: Option<&Value>) -> Vec<String> {
    schema_list(schema, "x-requires")
}

/// The entries of `requires` that are not met by an installed, enabled plugin of the right version.
fn unmet(all: &[plugins::Summary], schema: Option<&Value>) -> Vec<String> {
    requirements(schema)
        .into_iter()
        .filter(|r| {
            let (id, req) = split_requirement(r);
            !all.iter().any(|o| o.id == id && o.enabled && o.approved && satisfies(&o.version, req))
        })
        .collect()
}

/// Plugins that are on and need this one.
fn dependents(all: &[plugins::Summary], id: &str) -> Vec<String> {
    all.iter()
        .filter(|o| o.enabled && o.id != id && requirements(o.schema.as_ref()).iter().any(|r| split_requirement(r).0 == id))
        .map(|o| o.id.clone())
        .collect()
}

/// Installing never enables a new plugin: an admin sees the capabilities first and approves them
///. An upgrade keeps what was approved: if it asks for nothing new it keeps running (its
/// tables are migrated now); otherwise it waits, and only the new capabilities need a decision.
/// Going to an older version needs `rollback`.
pub(super) async fn install_bytes(state: &AppState, bytes: &[u8], rollback: bool) -> Result<Response, AppError> {
    let pkg = read_package(bytes).map_err(AppError::Validation)?;
    if state.settings().bool("require_signed_plugins") && !pkg.signed {
        return Err(AppError::Validation("This site only accepts signed plugins.".into()));
    }
    if let Some(module) = pkg.module.clone() {
        tokio::task::spawn_blocking(move || Module::new(&ENGINE, module))
            .await
            .map_err(AppError::internal)?
            .map_err(|e| AppError::invalid("The module does not compile: {e}", &[("e", &e)]))?;
    }
    let mf = &pkg.manifest;
    let pool = state.db()?;
    let existing = plugins::summaries(pool).await.db()?.into_iter().find(|p| p.id == mf.id);
    if let Some(old) = &existing
        && !rollback
        && compare(&mf.version, &old.version) == Some(std::cmp::Ordering::Less)
    {
        return Err(AppError::invalid(
            "Version {new} is older than the installed version {old}. Use Roll back to go back.",
            &[("new", &mf.version), ("old", &old.version)],
        ));
    }
    let keeps_approval = existing
        .as_ref()
        .is_some_and(|old| old.approved && mf.capabilities.iter().all(|c| old.approved_capabilities.contains(c)));
    if keeps_approval {
        // Before the new code runs: if the tables cannot be migrated the old version stays.
        let ddl = bf_db::ddl_pool(pool).await.db()?;
        plugins::migrate_tables(&ddl, &mf.id, &mf.tables).await.db()?;
        if mf.capabilities.iter().any(|c| c == "content.write") {
            plugins::ensure_bot(pool, &mf.id).await.db()?;
        }
    }
    plugins::upsert(
        pool,
        &plugins::NewPlugin {
            id: &mf.id,
            name: mf.name.trim(),
            version: &mf.version,
            capabilities: &mf.capabilities,
            hooks: &mf.hooks,
            module: pkg.module.as_deref(),
            slots: &pkg.slots,
            client_js: pkg.client.as_deref(),
            schema: pkg.schema.as_ref(),
            checksum: &pkg.checksum,
            package: bytes,
            rollback,
        },
    )
    .await
    .db()?;
    // A plugin whose requirements are no longer met is switched off rather than left half working.
    let all = plugins::summaries(pool).await.db()?;
    let missing = unmet(&all, pkg.schema.as_ref());
    let restored = rollback && existing.as_ref().is_some_and(|o| !o.approved) && {
        let now = plugins::summaries(pool).await.db()?;
        now.iter().any(|p| p.id == mf.id && p.enabled)
    };
    let enabled = (keeps_approval && existing.as_ref().is_some_and(|o| o.enabled) && missing.is_empty()) || restored;
    if (keeps_approval || restored) && !missing.is_empty() {
        plugins::disable(pool, &mf.id).await.db()?;
    }
    reload(state).await?;
    let new_caps: Vec<&String> = existing.as_ref().map_or_else(
        || mf.capabilities.iter().collect(),
        |o| mf.capabilities.iter().filter(|c| !o.approved_capabilities.contains(c)).collect(),
    );
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": mf.id, "capabilities": mf.capabilities, "new_capabilities": new_caps,
            "upgraded_from": existing.as_ref().map(|o| o.version.clone()), "enabled": enabled,
            "requires_unmet": missing, "checksum": pkg.checksum, "signed": pkg.signed,
        })),
    )
        .into_response())
}

pub(super) async fn install(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    mut m: Multipart,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, true)?;
    let mut bytes = None;
    while let Some(f) = m.next_field().await.map_err(|e| AppError::BadRequest(e.to_string()))? {
        if f.name() == Some("file") {
            bytes = Some(f.bytes().await.map_err(|e| AppError::BadRequest(e.to_string()))?);
        }
    }
    let bytes = bytes.ok_or_else(|| AppError::Validation("No file was sent.".into()))?;
    install_bytes(&state, &bytes, false).await
}

#[derive(Deserialize)]
pub(super) struct InstallUrl {
    pub(super) url: String,
}

pub(super) async fn install_url(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Json(b): Json<InstallUrl>,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, true)?;
    let bytes = crate::platform::outbound::fetch_public(&b.url, MAX_PACKAGE).await.map_err(AppError::Validation)?;
    install_bytes(&state, &bytes, false).await
}

/// Puts back the package the last upgrade replaced. The version it replaces becomes the one to go back to.
pub(super) async fn rollback(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, true)?;
    let bytes = plugins::previous_package(state.db()?, &id)
        .await
        .db()?
        .ok_or_else(|| AppError::Validation("There is no earlier version to go back to.".into()))?;
    install_bytes(&state, &bytes, true).await
}

#[derive(Deserialize)]
pub(super) struct Approval {
    pub(super) capabilities: Vec<String>,
}

/// The admin must name the capabilities they saw, so an update asking for more cannot slip through.
pub(super) async fn enable(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(b): Json<Approval>,
) -> Result<Response, AppError> {
    let who = admin(&state, &v, &headers, true)?;
    let pool = state.db()?;
    let all = plugins::summaries(pool).await.db()?;
    let current = all.iter().find(|p| p.id == id).cloned().ok_or(AppError::NotFound)?;
    if let Some(r) = unmet(&all, current.schema.as_ref()).first() {
        return Err(AppError::invalid("This plugin needs {requirement}, which is not installed and enabled.", &[("requirement", r)]));
    }
    let (mut asked, mut approved) = (current.capabilities, b.capabilities);
    asked.sort();
    approved.sort();
    if asked != approved {
        return Err(AppError::Validation("The approved capabilities do not match what this plugin asks for.".into()));
    }
    // Migrations run only now, after the admin has approved.
    let tables: Vec<plugins::TableSpec> = schema_list(current.schema.as_ref(), "x-tables");
    let ddl = bf_db::ddl_pool(pool).await.db()?;
    plugins::migrate_tables(&ddl, &id, &tables).await.db()?;
    if asked.iter().any(|c| c == "content.write") {
        plugins::ensure_bot(pool, &id).await.db()?;
    }
    plugins::approve_and_enable(pool, &id, who).await.db()?;
    reload(&state).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub(super) async fn disable(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, true)?;
    let all = plugins::summaries(state.db()?).await.db()?;
    if let Some(d) = dependents(&all, &id).first() {
        return Err(AppError::invalid("The plugin {other} needs this one. Disable it first.", &[("other", d)]));
    }
    if !plugins::disable(state.db()?, &id).await.db()? {
        return Err(AppError::NotFound);
    }
    reload(&state).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub(super) async fn set_settings(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(b): Json<Value>,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, true)?;
    if !b.is_object() {
        return Err(AppError::Validation("Settings must be an object.".into()));
    }
    let b = super::extras::check_settings(&state, &id, b).await?;
    if !plugins::set_settings(state.db()?, &id, &b).await.db()? {
        return Err(AppError::NotFound);
    }
    reload(&state).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Removes the plugin and everything it stored.
pub(super) async fn uninstall(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, true)?;
    let pool = state.db()?;
    let all = plugins::summaries(pool).await.db()?;
    if let Some(d) = dependents(&all, &id).first() {
        return Err(AppError::invalid("The plugin {other} needs this one. Disable it first.", &[("other", d)]));
    }
    bf_db::platform::registry_more::purge_plugin(pool, &id).await.db()?;
    bf_db::platform::registry_content::purge_plugin(pool, &id).await.db()?;
    if !plugins::delete(pool, &id).await.db()? {
        return Err(AppError::NotFound);
    }
    reload(&state).await?;
    let ddl = bf_db::ddl_pool(pool).await.db()?;
    plugins::drop_tables(&ddl, &id).await.db()?;
    bf_db::platform::registry_groups::purge_plugin(pool, &id).await.db()?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `boring-forum plugin new <id>`: a starting point with a module that does nothing yet.
pub fn scaffold(id: &str) -> Result<String, String> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        return Err(en("Use a lowercase id with dashes.").into());
    }
    let dir = std::path::Path::new(id);
    std::fs::create_dir_all(dir.join("templates")).map_err(|e| e.to_string())?;
    let manifest = format!(
        "id = \"{id}\"\nname = \"{id}\"\nversion = \"0.1.0\"\napi = \"^1.0\"\nkind = \"wasm\"\ncapabilities = []\nhooks = [\"post.created\"]\n\n[server]\nmodule = \"server.wat\"\n"
    );
    let module = "(module\n  (memory (export \"memory\") 1)\n  (global $top (mut i32) (i32.const 1024))\n  (data (i32.const 0) \"{}\")\n  (func (export \"alloc\") (param $n i32) (result i32)\n    (local $p i32) (local.set $p (global.get $top))\n    (global.set $top (i32.add (global.get $top) (local.get $n))) (local.get $p))\n  ;; Input and output are JSON; returns (ptr << 32 | len).\n  (func (export \"handle\") (param i32 i32) (result i64) (i64.const 2)))\n";
    std::fs::write(dir.join("plugin.toml"), manifest).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("server.wat"), module).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("templates/footer.html"), "<p class=\"small\">Hello from {{ site_name }}</p>\n")
        .map_err(|e| e.to_string())?;
    Ok(format!("Created {id}/. Zip its contents and install it under Admin → Plugins."))
}
