//! Theme and widget administration.

use crate::admin::api::{Admin, AdminRead, AdminWrite};
use crate::http::render::DbResult;
use crate::http::viewer::{Viewer, cookie};
use crate::state::AppState;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, patch, post, put};
use axum::{Form, Json, Router};
use bf_core::AppError;
use bf_db::admin::themes;
use serde::Deserialize;
use serde_json::{Value, json};

mod package;
mod watch;
mod widgets;

use self::package::{ThemeToml, install_url, install_zip};
use self::widgets::{add_widget, arrange, delete_widget, list_widgets, update_widget};

pub use self::watch::watch;
pub(crate) use self::widgets::{home_widgets, page_area, page_widgets};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/theme/{file}", get(stylesheet))
        .route("/safe-mode", get(safe_mode))
        .route("/settings/theme", post(choose))
        .route("/api/v1/admin/themes", get(list).post(create))
        .route("/api/v1/admin/themes/install", post(install_zip).layer(DefaultBodyLimit::max(MAX_ZIP)))
        .route("/api/v1/admin/themes/install-url", post(install_url))
        .route("/api/v1/admin/themes/{id}", patch(update).delete(remove))
        .route("/api/v1/admin/themes/{id}/preview", post(preview))
        .route("/api/v1/admin/themes/preview/stop", post(stop_preview))
        .route("/api/v1/admin/nodes/{id}/theme", put(node_theme))
        .route("/api/v1/admin/widgets", get(list_widgets).post(add_widget))
        .route("/api/v1/admin/widgets/arrange", put(arrange))
        .route("/api/v1/admin/widgets/{id}", patch(update_widget).delete(delete_widget))
}

pub const PREVIEW_COOKIE: &str = "bf_theme_preview";

pub const SAFE_COOKIE: &str = "bf_safe_mode";

const MAX_ZIP: usize = 5 * 1024 * 1024;

const MAX_UNPACKED: u64 = 20 * 1024 * 1024;

/// Loads themes into the page renderer. Called at start and after every change.
pub async fn reload(state: &AppState) -> Result<(), AppError> {
    let pool = state.db()?;
    let rows = themes::all(pool).await.db()?;
    // The schemes first: a theme carries the palettes it chose.
    crate::admin::schemes::install(state).await?;
    let sources: Vec<_> = rows
        .into_iter()
        .map(|t| bf_web::theme::Source {
            palettes: bf_web::schemes::pair(t.light_scheme.as_deref(), t.dark_scheme.as_deref()),
            groups: t.group_ids.clone(),
            id: t.id,
            name: t.name,
            component: t.kind == "component",
            enabled: t.enabled,
            selectable: t.selectable,
            tokens: t.tokens,
            css: t.css,
            slots: t.slots,
        })
        .collect();
    let s = state.settings();
    let default = Some(s.int("default_theme_id")).filter(|d| *d > 0);
    bf_web::theme::install(&sources, default, s.str("site_name"), !s.bool("builtin_theme_enabled"));
    state.microcache.clear();
    Ok(())
}

/// Hash-named, so it can be cached forever.
async fn stylesheet(Path(file): Path<String>) -> Result<Response, AppError> {
    let c = bf_web::theme::stylesheet(&format!("/theme/{file}")).ok_or(AppError::NotFound)?;
    Ok((
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
        ],
        c.css.clone(),
    )
        .into_response())
}

fn cookie_header(name: &str, value: &str, max_age: i64) -> Result<HeaderValue, AppError> {
    HeaderValue::from_str(&format!("{name}={value}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age={max_age}"))
        .map_err(AppError::internal)
}

#[derive(Deserialize)]
struct SafeQuery {
    #[serde(default)]
    off: String,
}

/// Recovery from a broken theme: admins only, for this browser only. Reachable by URL
/// because a broken page may have no working button.
async fn safe_mode(v: Viewer, Query(q): Query<SafeQuery>) -> Result<Response, AppError> {
    if !v.actor.is_admin {
        return Err(AppError::Forbidden);
    }
    let mut res = Redirect::to("/").into_response();
    let c = if q.off == "1" { cookie_header(SAFE_COOKIE, "", 0)? } else { cookie_header(SAFE_COOKIE, "1", 3600)? };
    res.headers_mut().append(header::SET_COOKIE, c);
    Ok(res)
}

#[derive(Deserialize)]
struct ChooseForm {
    csrf: String,
    #[serde(default)]
    theme: String,
    #[serde(default)]
    scheme: String,
}

async fn choose(State(state): State<AppState>, v: Viewer, Form(f): Form<ChooseForm>) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let u = v.require_user()?;
    let id = f.theme.parse::<i64>().ok();
    if let Some(id) = id
        && !bf_web::theme::choices(&v.actor.groups.iter().map(|g| g.get()).collect::<Vec<_>>())
            .iter()
            .any(|(c, _)| *c == id)
    {
        return Err(AppError::Validation("That theme is not available.".into()));
    }
    themes::set_user_theme(state.db()?, u.id.get(), id).await.db()?;
    let scheme = f.scheme.trim();
    if !scheme.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') || scheme.len() > 40 {
        return Err(AppError::Validation("That colour scheme is not available.".into()));
    }
    themes::set_appearance(state.db()?, u.id.get(), Some(scheme).filter(|x| !x.is_empty())).await.db()?;
    Ok(Redirect::to("/settings").into_response())
}

async fn list(State(state): State<AppState>, Admin(..): AdminRead) -> Result<Response, AppError> {
    let rows = themes::all(state.db()?).await.db()?;
    Ok(Json(json!({
        "themes": rows,
        "default": state.settings().int("default_theme_id"),
        "builtin_enabled": state.settings().bool("builtin_theme_enabled"),
        "slots": bf_web::theme::SLOTS,
    }))
    .into_response())
}

fn valid_slug(s: &str) -> bool {
    !s.is_empty() && s.len() <= 60 && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn check_parts(tokens: &Value, css: &str, slots: &Value) -> Result<(), AppError> {
    bf_web::theme::check_tokens(tokens).map_err(AppError::Validation)?;
    bf_web::theme::check_css(css).map_err(AppError::Validation)?;
    for (name, src) in slots.as_object().into_iter().flatten() {
        let partial = name.strip_prefix("partial:").is_some_and(|p| bf_web::theme::PARTIALS.contains(&p));
        if !partial && !bf_web::theme::SLOTS.contains(&name.as_str()) {
            return Err(AppError::invalid("Unknown slot {name}.", &[("name", &name)]));
        }
        let src = src.as_str().ok_or_else(|| AppError::invalid("Slot {name} must be text.", &[("name", &name)]))?;
        bf_web::theme::render_slot(src, &json!({ "site_name": "x" }))
            .map_err(|e| AppError::invalid("Slot {name}: {e}", &[("name", &name), ("e", &e)]))?;
    }
    Ok(())
}

#[derive(Deserialize)]
struct NewTheme {
    slug: String,
    name: String,
    #[serde(default = "default_kind")]
    kind: String,
    #[serde(default)]
    tokens: Value,
    #[serde(default)]
    css: String,
    #[serde(default)]
    slots: Value,
}

fn default_kind() -> String {
    "theme".into()
}

/// The names of the slots and template parts a theme fills: what the staff log says about them.
fn slot_names(slots: &Value) -> Vec<&String> {
    slots.as_object().map(|o| o.keys().collect()).unwrap_or_default()
}

async fn save_new(state: &AppState, t: NewTheme, version: &str, who: Option<i64>) -> Result<i64, AppError> {
    if !valid_slug(&t.slug) || t.name.trim().is_empty() || !matches!(t.kind.as_str(), "theme" | "component") {
        return Err(AppError::Validation(
            "Use a lowercase id with dashes, a name, and type theme or component.".into(),
        ));
    }
    check_parts(&t.tokens, &t.css, &t.slots)?;
    let empty = json!({});
    let new = themes::NewTheme {
        slug: &t.slug,
        name: t.name.trim(),
        version,
        kind: &t.kind,
        tokens: if t.tokens.is_null() { &empty } else { &t.tokens },
        css: &t.css,
        slots: if t.slots.is_null() { &empty } else { &t.slots },
    };
    let id = themes::upsert(state.db()?, &new).await.db()?;
    // A theme's slots are HTML on every page: who installed or replaced it is on record.
    let details = json!({
        "slug": t.slug, "name": t.name.trim(), "version": version, "kind": t.kind,
        "css_bytes": t.css.len(), "slots": slot_names(&t.slots),
    });
    crate::admin::api::audit_log(state.db()?, who, "theme.save", &format!("theme:{id}"), details).await?;
    reload(state).await?;
    Ok(id)
}

async fn create(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Json(t): Json<NewTheme>,
) -> Result<Response, AppError> {
    let id = save_new(&state, t, "1.0.0", who).await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))).into_response())
}

#[derive(Deserialize)]
struct ThemePatch {
    tokens: Option<Value>,
    css: Option<String>,
    slots: Option<Value>,
    enabled: Option<bool>,
    selectable: Option<bool>,
    position: Option<i32>,
    /// A scheme's address name for light mode; empty follows the site.
    light_scheme: Option<String>,
    dark_scheme: Option<String>,
    /// Groups the theme is limited to; none means everyone.
    group_ids: Option<Vec<i64>>,
}

async fn update(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path(id): Path<i64>,
    Json(p): Json<ThemePatch>,
) -> Result<Response, AppError> {
    check_parts(
        p.tokens.as_ref().unwrap_or(&Value::Null),
        p.css.as_deref().unwrap_or(""),
        p.slots.as_ref().unwrap_or(&Value::Null),
    )?;
    let pool = state.db()?;
    let known = bf_db::admin::schemes::all(pool).await.db()?;
    for slug in [&p.light_scheme, &p.dark_scheme].into_iter().flatten().filter(|s| !s.is_empty()) {
        if !known.iter().any(|s| &s.slug == slug) {
            return Err(AppError::Validation("That colour scheme does not exist.".into()));
        }
    }
    if p.group_ids.as_ref().is_some_and(|g| g.iter().any(|g| *g <= 0) || g.len() > 50) {
        return Err(AppError::Validation("Pick groups from the list.".into()));
    }
    // With the built-in look off, the site's default theme has to stay available.
    let s = state.settings();
    if !s.bool("builtin_theme_enabled") && s.int("default_theme_id") == id && p.enabled == Some(false) {
        return Err(AppError::Validation(
            "The built-in look is off: choose another default theme before turning this one off.".into(),
        ));
    }
    // What changed, without the styles and markup themselves (they can be megabytes).
    let details = json!({
        "enabled": p.enabled, "selectable": p.selectable, "position": p.position,
        "light_scheme": p.light_scheme, "dark_scheme": p.dark_scheme, "group_ids": p.group_ids,
        "tokens": p.tokens.is_some(), "css_bytes": p.css.as_ref().map(String::len),
        "slots": p.slots.as_ref().map(slot_names),
    });
    let change = themes::ThemeChange {
        light_scheme: p.light_scheme,
        dark_scheme: p.dark_scheme,
        group_ids: p.group_ids,
        tokens: p.tokens,
        css: p.css,
        slots: p.slots,
        enabled: p.enabled,
        selectable: p.selectable,
        position: p.position,
    };
    themes::update(pool, id, &change).await.found()?;
    crate::admin::api::audit_log(pool, who, "theme.update", &format!("theme:{id}"), details).await?;
    reload(&state).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn remove(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let s = state.settings();
    if !s.bool("builtin_theme_enabled") && s.int("default_theme_id") == id {
        return Err(AppError::Validation(
            "The built-in look is off: choose another default theme before deleting this one.".into(),
        ));
    }
    themes::delete(state.db()?, id).await.found()?;
    crate::admin::api::audit_log(state.db()?, who, "theme.delete", &format!("theme:{id}"), json!({})).await?;
    state.forum.invalidate();
    reload(&state).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Shows a theme to this admin only, until stopped.
async fn preview(
    State(_state): State<AppState>,
    Admin(..): AdminWrite,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let mut res = StatusCode::NO_CONTENT.into_response();
    res.headers_mut().append(header::SET_COOKIE, cookie_header(PREVIEW_COOKIE, &id.to_string(), 3600)?);
    Ok(res)
}

async fn stop_preview(State(_state): State<AppState>, Admin(..): AdminWrite) -> Result<Response, AppError> {
    let mut res = StatusCode::NO_CONTENT.into_response();
    res.headers_mut().append(header::SET_COOKIE, cookie_header(PREVIEW_COOKIE, "", 0)?);
    Ok(res)
}

#[derive(Deserialize)]
struct NodeTheme {
    theme: Option<i64>,
}

async fn node_theme(
    State(state): State<AppState>,
    Admin(..): AdminWrite,
    Path(id): Path<i64>,
    Json(b): Json<NodeTheme>,
) -> Result<Response, AppError> {
    themes::set_node_theme(state.db()?, id, b.theme).await.found()?;
    state.forum.invalidate();
    state.microcache.clear();
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// The safe-mode cookie is present. The policy header is built before the viewer is known, so
/// this does not check for an admin: anyone else sending the cookie only gets a policy without
/// the custom code, which their pages then still carry (see `apply`), in their own browser.
pub(crate) fn safe_mode_cookie(headers: &HeaderMap) -> bool {
    cookie(headers, SAFE_COOKIE).is_some()
}

/// The theme for a viewer: safe mode beats all; then preview, the forum's own, and their choice.
pub(crate) fn apply(l: &mut bf_web::pages::Layout, v: &Viewer, headers: &HeaderMap, node_theme: Option<i64>) {
    if v.actor.is_admin && cookie(headers, SAFE_COOKIE).is_some() {
        l.theme = None;
        l.plugins = None;
        l.custom = None;
        return;
    }
    let preview = if v.actor.is_admin { cookie(headers, PREVIEW_COOKIE).and_then(|c| c.parse().ok()) } else { None };
    l.theme = preview.and_then(bf_web::theme::preview).or_else(|| {
        let groups: Vec<i64> = v.actor.groups.iter().map(|g| g.get()).collect();
        bf_web::theme::resolve(&[node_theme, v.user.as_ref().and_then(|u| u.theme_id)], &groups)
    });
    l.scheme = v.user.as_ref().and_then(|u| u.color_scheme.clone()).unwrap_or_default();
}

// --- Widgets
