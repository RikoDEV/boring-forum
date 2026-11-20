//! Pages, sign-in providers, member filters and API routes that plugins provide.

use crate::admin::api::admin;
use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bf_core::AppError;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;

use super::extras::strings_for;
use super::{ASSETS, CLIENTS, Loaded, call, listening};

/// A plugin's page (`account_settings_tab`, `modcp_panel`, `profile_tab`, `admin_page`).
/// The area decides who may open it; the plugin's output is sanitized like a slot.
pub(super) async fn panel_page(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path((id, key)): Path<(String, String)>,
    axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let p = listening("panel.render").into_iter().find(|p| p.id == id).ok_or(AppError::NotFound)?;
    let panel = p.panels.iter().find(|x| x.key == key).cloned().ok_or(AppError::NotFound)?;
    match panel.area.as_str() {
        "settings" => {
            v.require_user()?;
        }
        "modcp" => {
            v.require_user()?;
            let tree = state.forum.get(state.db()?).await?;
            if !tree.nodes.iter().any(|n| tree.perms(&v.actor, n.id).has(bf_core::perms::Perms::MODERATE)) {
                return Err(AppError::Forbidden);
            }
        }
        "admin" => {
            admin(&state, &v, &headers, false)?;
        }
        _ => {}
    }
    let data = json!({
        "area": panel.area,
        "key": key,
        "viewer_id": v.user.as_ref().map(|u| u.id),
        "profile_id": q.get("user").and_then(|u| u.parse::<i64>().ok()),
        "locale": v.t().locale,
        "dir": v.t().dir(),
        "strings": strings_for(&id, v.t().locale),
    });
    let out = call(&state, p, "panel.render", &data).await.ok_or_else(|| AppError::internal("plugin failed"))?;
    let html = out.get("html").and_then(Value::as_str).map(bf_web::theme::clean_html).unwrap_or_default();
    let title = bf_web::theme::plugin_label(v.t().locale, &panel.label);
    Ok(crate::http::ui::simple_page(&state, &v, &title, &html, panel.area == "content"))
}

pub(super) const LOGIN_COOKIE: &str = "bf_plugin_login";

/// A plugin's sign-in provider (`login_provider`): `login.begin` names where to send the
/// browser, and `login.callback` says who came back. This grants sign-in to any account whose
/// verified address the plugin reports, which is why it takes its own approved capability.
pub(super) async fn login_begin(
    State(state): State<AppState>,
    Path((id, key)): Path<(String, String)>,
) -> Result<Response, AppError> {
    let p = login_plugin(&id, "login.begin")?;
    let token: String = (0..32).map(|_| format!("{:02x}", rand::random::<u8>())).collect();
    let callback = format!("{}/auth/p/{id}/{key}/callback", state.config.base_url);
    let data = json!({ "key": key, "state": token, "callback_url": callback });
    let out = call(&state, p, "login.begin", &data).await.ok_or_else(|| AppError::internal("plugin failed"))?;
    let target = out.get("redirect").and_then(Value::as_str).unwrap_or_default();
    if !target.starts_with("https://") {
        return Err(AppError::Validation("This sign-in option is not working.".into()));
    }
    let mut res = axum::response::Redirect::to(target).into_response();
    let cookie = format!("{LOGIN_COOKIE}={token}; Path=/auth/p/; HttpOnly; Secure; SameSite=Lax; Max-Age=600");
    res.headers_mut().append(header::SET_COOKIE, cookie.parse().map_err(AppError::internal)?);
    Ok(res)
}

pub(super) async fn login_callback(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path((id, key)): Path<(String, String)>,
    axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let p = login_plugin(&id, "login.callback")?;
    let cookie = crate::http::viewer::cookie(&headers, LOGIN_COOKIE).unwrap_or_default().to_owned();
    let given = q.get("state").map_or("", String::as_str);
    if cookie.len() != 64 || !crate::admin::api::constant_time_eq(cookie.as_bytes(), given.as_bytes()) {
        return Err(AppError::BadRequest("This sign-in link has expired. Please try again.".into()));
    }
    let out = call(&state, p, "login.callback", &json!({ "key": key, "query": q }))
        .await
        .ok_or_else(|| AppError::internal("plugin failed"))?;
    let subject = out.get("subject").and_then(Value::as_str).filter(|s| !s.is_empty());
    let Some(subject) = subject else {
        return Err(AppError::Validation("That sign-in did not succeed.".into()));
    };
    let ext = crate::auth::oauth::ExternalUser {
        subject: subject.to_owned(),
        email: out.get("email").and_then(Value::as_str).map(str::to_owned),
        email_verified: out.get("email_verified").and_then(Value::as_bool).unwrap_or(false),
        name: out.get("name").and_then(Value::as_str).unwrap_or("member").to_owned(),
    };
    let provider = format!("plugin-{id}-{key}");
    crate::auth::oauth::sign_in_external(&state, &v, &provider, &id, &ext).await
}

pub(super) fn login_plugin(id: &str, hook: &str) -> Result<Arc<Loaded>, AppError> {
    listening(hook)
        .into_iter()
        .find(|p| p.id == id && p.capabilities.iter().any(|c| c == "login"))
        .ok_or(AppError::NotFound)
}

#[derive(Deserialize)]
pub(super) struct MemberFilter {
    pub(super) filter: String,
    pub(super) group: i64,
}

/// `member.filter` (`member_filter`): a plugin picks members, and they join a group.
pub(super) async fn member_filter(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(b): Json<MemberFilter>,
) -> Result<Response, AppError> {
    admin(&state, &v, &headers, true)?;
    if b.group == 3 || b.group == 4 || b.group <= 0 {
        return Err(AppError::Validation("Staff groups cannot be granted in bulk.".into()));
    }
    let p = listening("member.filter").into_iter().find(|p| p.id == id).ok_or(AppError::NotFound)?;
    let out = call(&state, p, "member.filter", &json!({ "filter": b.filter }))
        .await
        .ok_or_else(|| AppError::internal("plugin failed"))?;
    let ids: Vec<i64> = out.get("user_ids").and_then(|u| serde_json::from_value(u.clone()).ok()).unwrap_or_default();
    let added = bf_db::community::groups::add_group_to(state.db()?, &ids, b.group).await.db()?;
    Ok(Json(json!({ "matched": ids.len(), "added": added })).into_response())
}

/// `/api/p/{plugin}/…`: needs the `api_route` capability; writes follow the usual CSRF rule.
pub(super) async fn api_route(
    State(state): State<AppState>,
    v: Viewer,
    method: Method,
    headers: HeaderMap,
    Path((id, path)): Path<(String, String)>,
    body: axum::body::Bytes,
) -> Result<Response, AppError> {
    if !matches!(method, Method::GET | Method::HEAD) {
        v.check_csrf_headers(&headers)?;
    }
    let p = listening("api.request")
        .into_iter()
        .find(|p| p.id == id && p.capabilities.iter().any(|c| c == "api_route"))
        .ok_or(AppError::NotFound)?;
    let parsed: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    // The scopes of this plugin that the calling API key holds, by their own key.
    let prefix = format!("plugin:{id}:");
    let scopes: Vec<&str> = v.plugin_scopes.iter().filter_map(|s| s.strip_prefix(prefix.as_str())).collect();
    let data = json!({
        "method": method.as_str(), "path": path, "body": parsed, "user_id": v.user.as_ref().map(|u| u.id),
        "api_key": v.api_scope.is_some(), "scopes": scopes,
    });
    let out = call(&state, p, "api.request", &data).await.ok_or_else(|| AppError::internal("plugin failed"))?;
    let status = out
        .get("status")
        .and_then(Value::as_u64)
        .and_then(|s| StatusCode::from_u16(s as u16).ok())
        .unwrap_or(StatusCode::OK);
    Ok((status, Json(out.get("json").cloned().unwrap_or(Value::Null))).into_response())
}

pub(super) async fn client_file(Path(file): Path<String>) -> Result<Response, AppError> {
    let js = CLIENTS
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&file)
        .cloned()
        .ok_or(AppError::NotFound)?;
    Ok((
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
        ],
        (*js).clone(),
    )
        .into_response())
}

/// Files under `/plugins/<id>.<hash>/…`: style sheets and assets. The folder name carries a hash
/// of what is in it, so a file never changes under its address and may be kept forever.
pub(super) async fn asset_file(Path((dir, path)): Path<(String, String)>) -> Result<Response, AppError> {
    let (mime, bytes) = ASSETS
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&dir)
        .and_then(|files| files.get(&path))
        .cloned()
        .ok_or(AppError::NotFound)?;
    let mut res = (
        [
            (header::CONTENT_TYPE, mime.as_str()),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            // An SVG opened on its own address runs with no scripts and no access to the site.
            (header::CONTENT_SECURITY_POLICY, "default-src 'none'; style-src 'unsafe-inline'; sandbox"),
        ],
        (*bytes).clone(),
    )
        .into_response();
    res.headers_mut().insert(
        header::HeaderName::from_static("cross-origin-resource-policy"),
        header::HeaderValue::from_static("same-origin"),
    );
    Ok(res)
}

/// Rendered slots for a while, by slot, viewer, language, page and post.
static SLOT_CACHE: std::sync::LazyLock<moka::sync::Cache<String, String>> = std::sync::LazyLock::new(|| {
    moka::sync::Cache::builder().max_capacity(2000).time_to_live(std::time::Duration::from_secs(30)).build()
});

pub(super) fn forget_slots() {
    SLOT_CACHE.invalidate_all();
}

/// `slot.render`: the plugins that draw a slot for this viewer. The page script asks for it,
/// because a page is cached for everyone and a slot may depend on who is looking.
pub(super) async fn slot_render(
    State(state): State<AppState>,
    v: Viewer,
    Path(slot): Path<String>,
    axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    if !bf_web::theme::plugin_output().extras.dynamic_slots.contains(&slot) {
        return Err(AppError::NotFound);
    }
    let page = q.get("page").filter(|p| super::local_path(p) && p.len() <= 200).cloned().unwrap_or_default();
    let post = q.get("post").and_then(|p| p.parse::<i64>().ok());
    let subject = super::subject::of_page(&state, &page, v.user.is_some()).await;
    let html = draw(&state, &v, &slot, &page, post, subject).await;
    Ok(([(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "private, max-age=15")], html)
        .into_response())
}

/// The plugins' HTML for one slot, for this viewer. `subject` is what the slot sits on (see
/// `subject.rs`); the answer is cached briefly by viewer, language, page, post and subject.
pub(crate) async fn draw(
    state: &AppState,
    v: &Viewer,
    slot: &str,
    page: &str,
    post: Option<i64>,
    subject: Option<Value>,
) -> String {
    let t = v.t();
    let viewer = v.user.as_ref().map(|u| u.id);
    let who = subject.as_ref().map(|s| (s["kind"].clone(), s["id"].clone()));
    let key = format!("{slot}\u{1}{viewer:?}\u{1}{}\u{1}{page}\u{1}{post:?}\u{1}{who:?}", t.locale);
    if let Some(html) = SLOT_CACHE.get(&key) {
        return html;
    }
    let mut html = String::new();
    for p in listening("slot.render") {
        let data = json!({
            "slot": slot, "page": page, "post_id": post, "viewer_id": viewer, "subject": subject,
            "locale": t.locale, "dir": t.dir(), "strings": strings_for(&p.id, t.locale),
        });
        if let Some(out) = call(state, p.clone(), "slot.render", &data).await
            && let Some(part) = out.get("html").and_then(Value::as_str)
        {
            let part = bf_web::theme::clean_html(part);
            if !part.trim().is_empty() {
                html.push_str(&format!(
                    "<div class=\"bf-plugin-slot-item\" data-plugin=\"{}\">{part}</div>",
                    bf_core::html::esc(&p.id)
                ));
            }
        }
    }
    SLOT_CACHE.insert(key, html.clone());
    html
}

/// The `user_card` slot of a member's hover card: HTML for its body, or empty.
pub(crate) async fn user_card_html(state: &AppState, v: &Viewer, user_id: i64) -> String {
    if !bf_web::theme::plugin_output().extras.dynamic_slots.contains(bf_web::theme::CARD_SLOT) {
        return String::new();
    }
    let subject = super::subject::user(state, user_id, v.user.is_some()).await;
    draw(state, v, bf_web::theme::CARD_SLOT, "", None, subject).await
}

/// Presses per member and plugin in the current minute, for the limit on `action.run`.
#[expect(clippy::type_complexity, reason = "a private counter table")]
static PRESSES: std::sync::LazyLock<std::sync::Mutex<HashMap<(String, i64), (i64, i64)>>> =
    std::sync::LazyLock::new(Default::default);

/// Counts a press and says whether it is within `limit` for this minute.
pub(super) fn press_allowed(plugin: &str, user: i64, limit: i64) -> bool {
    let minute = chrono::Utc::now().timestamp() / 60;
    let mut all = PRESSES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if all.len() > 20_000 {
        all.retain(|_, (m, _)| *m == minute);
    }
    let slot = all.entry((plugin.to_owned(), user)).or_insert((minute, 0));
    if slot.0 != minute {
        *slot = (minute, 0);
    }
    if slot.1 >= limit {
        return false;
    }
    slot.1 += 1;
    true
}

/// `/plugin-emoji/<plugin>/assets/<file>`: an emoji image. The address does not change when the
/// plugin is upgraded, which is why it is what posts store; the answer is kept for an hour.
pub(super) async fn emoji_file(Path((id, path)): Path<(String, String)>) -> Result<Response, AppError> {
    let dir = super::extras::dir_of(&id).ok_or(AppError::NotFound)?;
    if !super::extras::emoji_entries().iter().any(|(_, url, ..)| url == &format!("/plugin-emoji/{id}/{path}")) {
        return Err(AppError::NotFound);
    }
    let (mime, bytes) = ASSETS
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&dir)
        .and_then(|files| files.get(&path))
        .cloned()
        .ok_or(AppError::NotFound)?;
    Ok((
        [
            (header::CONTENT_TYPE, mime.as_str()),
            (header::CACHE_CONTROL, "public, max-age=3600"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::CONTENT_SECURITY_POLICY, "default-src 'none'; style-src 'unsafe-inline'; sandbox"),
        ],
        (*bytes).clone(),
    )
        .into_response())
}

#[derive(Deserialize, Default)]
pub(super) struct ActionBody {
    #[serde(default)]
    post_id: Option<i64>,
    #[serde(default)]
    selection: Option<String>,
}

/// A button from `[[post_actions]]` or `[[composer_buttons]]`: the plugin's `action.run` hook
/// answers, and the declared `result` decides how the answer is used: a message (`toast`), text
/// for the composer (`insert`) or sanitized HTML in a dialog (`panel`).
pub(super) async fn action_run(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path((id, key)): Path<(String, String)>,
    body: Option<Json<ActionBody>>,
) -> Result<Response, AppError> {
    v.check_csrf_headers(&headers)?;
    let user = v.require_user()?;
    if !press_allowed(&id, user.id.get(), state.settings().int("plugin_actions_per_minute")) {
        return Err(AppError::RateLimited { retry_after_secs: 30 });
    }
    let def = bf_web::theme::plugin_output()
        .extras
        .actions
        .iter()
        .find(|a| a["plugin"] == id.as_str() && a["key"] == key.as_str())
        .cloned()
        .ok_or(AppError::NotFound)?;
    let p = listening("action.run").into_iter().find(|p| p.id == id).ok_or(AppError::NotFound)?;
    let Json(b) = body.unwrap_or_default();
    let t = v.t();
    let selection: String = b.selection.unwrap_or_default().chars().take(2000).collect();
    let data = json!({
        "key": key, "kind": def["kind"], "post_id": b.post_id, "selection": selection, "viewer_id": user.id,
        "locale": t.locale, "dir": t.dir(), "strings": strings_for(&id, t.locale),
    });
    let out = call(&state, p, "action.run", &data).await.ok_or_else(|| AppError::internal("plugin failed"))?;
    if let Some(problem) = out.get("error").and_then(Value::as_str) {
        return Err(AppError::Validation(problem.chars().take(300).collect()));
    }
    let text =
        |k: &str, max: usize| out.get(k).and_then(Value::as_str).map(|s| s.chars().take(max).collect::<String>());
    let answer = match def["result"].as_str() {
        Some("insert") => json!({ "insert": text("insert", 20_000).unwrap_or_default() }),
        Some("panel") => json!({ "html": bf_web::theme::clean_html(&text("html", 50_000).unwrap_or_default()) }),
        _ => json!({ "toast": text("toast", 300).unwrap_or_default() }),
    };
    Ok(Json(answer).into_response())
}

/// `onebox.resolve`: a plugin's card for a link. Only plugins approved for the link's own host
/// (`http:<host>`) are asked, so a provider is limited to the sites it was approved to read, and
/// the text it answers with is shown as text, never as markup.
pub(crate) async fn onebox_card(state: &AppState, url: &str) -> Option<bf_db::forum::links::Preview> {
    for p in listening("onebox.resolve") {
        let hosts: Vec<String> =
            p.capabilities.iter().filter_map(|c| c.strip_prefix(super::HTTP_PREFIX)).map(str::to_owned).collect();
        if !super::http_allowed(&hosts, url) {
            continue;
        }
        let Some(out) = call(state, p, "onebox.resolve", &json!({ "url": url })).await else { continue };
        let text = |k: &str, max: usize| {
            out.get(k).and_then(Value::as_str).unwrap_or_default().chars().take(max).collect::<String>()
        };
        let title = text("title", 200);
        if title.trim().is_empty() {
            continue;
        }
        let image = text("image", 500);
        return Some(bf_db::forum::links::Preview {
            ok: true,
            title,
            description: text("description", 400),
            image: if image.starts_with("https://") { image } else { String::new() },
            site: text("site", 100),
        });
    }
    None
}
