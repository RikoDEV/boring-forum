//! JSON API under `/api/v1`. Each handler is listed in `openapi.rs`.

use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::{get, put};
use axum::{Json, Router};
use bf_core::settings::SettingDef;
use bf_core::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use utoipa::ToSchema;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/settings", get(public_settings))
        .route("/admin/settings", get(admin_settings))
        .route("/admin/settings/{key}", put(update_setting))
        .route("/openapi.json", get(|| async { Json(crate::platform::openapi::spec()) }))
        .fallback(|| async { AppError::NotFound })
}

/// Settings that are safe to show to anyone.
#[utoipa::path(get, path = "/api/v1/settings", tag = "settings",
    responses((status = 200, body = Object, description = "Map of public setting keys to values")))]
pub async fn public_settings(State(state): State<AppState>) -> Json<BTreeMap<&'static str, Value>> {
    Json(state.settings().public_values())
}

#[derive(Serialize, ToSchema)]
pub struct AdminSetting {
    #[serde(flatten)]
    pub def: SettingDef,
    #[schema(value_type = Object)]
    pub value: Value,
}

#[utoipa::path(get, path = "/api/v1/admin/settings", tag = "admin", security(("admin_token" = [])),
    responses((status = 200, body = [AdminSetting]), (status = 403, body = bf_core::error::Problem)))]
pub async fn admin_settings(
    State(state): State<AppState>,
    crate::admin::api::Admin(_who, v): crate::admin::api::AdminRead,
) -> AppResult<Json<Vec<Value>>> {
    let s = state.settings();
    let t = v.t();
    // Descriptions are English in bf-core; the panel shows them in the admin's language.
    let rows = s
        .defs()
        .map(|d| {
            let stored = s.get(d.key).cloned().unwrap_or_default();
            // A key or secret never leaves the server again: the panel is told only that one is set.
            let secret = is_secret(d.key);
            let set = stored.as_str().is_some_and(|v| !v.is_empty());
            let value = if secret { Value::String(String::new()) } else { stored };
            let mut row = serde_json::to_value(AdminSetting { def: d.clone(), value }).unwrap_or_default();
            row["description"] = Value::String(t.msg(d.description));
            if secret {
                row["set"] = Value::Bool(set);
            }
            row
        })
        .collect();
    Ok(Json(rows))
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateSetting {
    #[schema(value_type = Object)]
    pub value: Value,
    /// For a key or secret: remove the stored one. An empty `value` alone keeps it.
    #[serde(default)]
    pub clear: bool,
}

#[utoipa::path(put, path = "/api/v1/admin/settings/{key}", tag = "admin", security(("admin_token" = [])),
    params(("key" = String, Path)), request_body = UpdateSetting,
    responses((status = 204), (status = 403, body = bf_core::error::Problem), (status = 422, body = bf_core::error::Problem)))]
pub async fn update_setting(
    State(state): State<AppState>,
    crate::admin::api::Admin(who, ..): crate::admin::api::AdminWrite,
    Path(key): Path<String>,
    Json(body): Json<UpdateSetting>,
) -> AppResult<axum::http::StatusCode> {
    if key == "site_name" && body.value.as_str().is_some_and(|s| s.trim().is_empty()) {
        return Err(AppError::Validation("The site needs a name.".into()));
    }
    // The panel never has a stored key or secret to send back: an empty value keeps what is
    // stored, and only an explicit `clear` removes it.
    if is_secret(&key) && !body.clear && body.value.as_str().is_some_and(str::is_empty) {
        return Ok(axum::http::StatusCode::NO_CONTENT);
    }
    // The built-in look can go once another theme is the site's default; and stays while none is.
    if let Some(pool) = &state.pool
        && matches!(key.as_str(), "builtin_theme_enabled" | "default_theme_id")
    {
        let now = state.settings();
        let builtin_on = if key == "builtin_theme_enabled" {
            body.value.as_bool() != Some(false)
        } else {
            now.bool("builtin_theme_enabled")
        };
        let default =
            if key == "default_theme_id" { body.value.as_i64().unwrap_or(0) } else { now.int("default_theme_id") };
        if !builtin_on {
            let themes = bf_db::admin::themes::all(pool).await.map_err(AppError::internal)?;
            if !themes.iter().any(|t| t.id == default && t.kind == "theme" && t.enabled) {
                return Err(AppError::Validation(
                    "Choose an enabled theme as the site's default before turning the built-in look off.".into(),
                ));
            }
        }
    }
    let was_on = state.settings().get(&key).and_then(Value::as_bool) == Some(true);
    let mut next = (*state.settings()).clone();
    next.set(&key, body.value.clone()).map_err(|e| AppError::Validation(e.to_string()))?;
    // Switching a module on puts its link in the header once; staff can move or drop it later.
    let nav = match bf_core::nav::module_for_setting(&key) {
        Some(module) if body.value.as_bool() == Some(true) && !was_on => {
            bf_core::nav::with_link(next.str("nav_links"), module)
        }
        _ => None,
    };
    if let Some(json) = &nav {
        next.set("nav_links", Value::String(json.clone())).map_err(|e| AppError::Validation(e.to_string()))?;
    }
    if let Some(pool) = &state.pool {
        bf_db::settings::save(pool, &key, &body.value).await.map_err(AppError::internal)?;
        bf_db_audit(pool, who, &key, &body.value).await?;
        if let Some(json) = &nav {
            let v = Value::String(json.clone());
            bf_db::settings::save(pool, "nav_links", &v).await.map_err(AppError::internal)?;
            bf_db_audit(pool, who, "nav_links", &v).await?;
        }
    }
    state.replace_settings(next);
    // Compiled into themes and plugin slots, so a new value needs them rebuilt.
    if matches!(
        key.as_str(),
        "default_theme_id" | "site_name" | "default_light_scheme" | "default_dark_scheme" | "builtin_theme_enabled"
    ) && state.pool.is_some()
    {
        crate::admin::themes::reload(&state).await?;
        crate::platform::plugins::reload(&state).await?;
    }
    state.microcache.clear();
    Ok(axum::http::StatusCode::NO_CONTENT)
}

pub(crate) async fn bf_db_audit(pool: &sqlx::PgPool, actor: Option<i64>, key: &str, value: &Value) -> AppResult<()> {
    let value = if is_secret(key) { Value::String("(changed)".into()) } else { value.clone() };
    crate::admin::api::audit_log(pool, actor, "setting.update", key, serde_json::json!({ "value": value })).await
}

/// Settings that hold a key or secret: the staff log says that they changed, not to what, and
/// the panel and the settings export are told only whether one is set.
pub(crate) fn is_secret(key: &str) -> bool {
    bf_db::settings::is_secret(key)
}
