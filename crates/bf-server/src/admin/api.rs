//! Admin control panel API (`/api/v1/admin/*`), used by the SvelteKit admin SPA.
//!
//! Access: a signed-in administrator (writes also need the CSRF token in
//! `X-CSRF-Token`), or the `BF_ADMIN_TOKEN` bearer token for bootstrapping and scripts.

use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, put};
use axum::{Json, Router};
use bf_core::AppError;
use serde_json::json;

/// A read-only handler: the administrator check, then `$body` as JSON. `$q` is the query string.
macro_rules! admin_get {
    ($(#[$m:meta])* $name:ident($s:ident $(, $q:ident: $qt:ty)?) $body:expr) => {
        $(#[$m])*
        pub(super) async fn $name(
            axum::extract::State($s): axum::extract::State<crate::state::AppState>,
            super::Admin(..): super::AdminRead,
            $(axum::extract::Query($q): axum::extract::Query<$qt>,)?
        ) -> Result<axum::response::Response, bf_core::AppError> {
            Ok(axum::response::IntoResponse::into_response(axum::Json($body)))
        }
    };
}

mod accounts;
mod fields;
mod groups;
mod member;
mod monitoring;
mod nodes;
mod security;
mod tags;
mod trust;

use self::accounts::{double_option, end_sessions, update_user, users};
use self::fields::{create_field, delete_field, list_fields, reorder_fields, update_field};
use self::groups::{create_group, delete_group, groups, set_group_limits, update_group};
use self::monitoring::{
    analytics, audit, bad_emails, bounce_webhook, clear_bad_email, failed_logins, mail_log, mail_status, search_report,
    stats, traffic,
};
use self::nodes::{
    copy_permissions, delete_node, moderators, node_text, reorder_nodes, set_moderators, set_node_icon,
    set_permissions, set_prefixes, set_rules, tree, update_node,
};
use self::security::require_2fa;
use self::tags::{create_tag, delete_tag, list_tags, rename_tag};
use self::trust::{member as member_trust, overview as trust_overview, preview as trust_preview, save as trust_save};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/admin/stats", get(stats))
        .route("/api/v1/admin/analytics", get(analytics))
        .route("/api/v1/admin/traffic", get(traffic))
        .route("/api/v1/admin/users", get(users))
        .route("/api/v1/admin/users/{id}", patch(update_user))
        .route("/api/v1/admin/nodes/reorder", put(reorder_nodes))
        .route("/api/v1/admin/nodes/{id}/icon", put(set_node_icon))
        .route("/api/v1/admin/users/{id}/logout", axum::routing::post(end_sessions))
        .route("/api/v1/admin/groups", get(groups).post(create_group))
        .route("/api/v1/admin/groups/{id}", put(update_group).delete(delete_group))
        .route("/api/v1/admin/tree", get(tree))
        .route("/api/v1/admin/nodes/{id}", get(node_text).patch(update_node).delete(delete_node))
        .route("/api/v1/admin/nodes/{id}/permissions", put(set_permissions))
        .route("/api/v1/admin/nodes/{id}/moderators", get(moderators).put(set_moderators))
        .route("/api/v1/admin/audit", get(audit))
        .route("/api/v1/admin/trust", get(trust_overview).put(trust_save))
        .route("/api/v1/admin/trust/preview", axum::routing::post(trust_preview))
        .route("/api/v1/admin/users/{id}/trust", get(member_trust))
        .route("/api/v1/admin/users/{id}/overview", get(member::overview))
        .route("/api/v1/admin/users/{id}/identity", patch(member::set_identity))
        .route("/api/v1/admin/users/{id}/send-password-reset", axum::routing::post(member::send_reset))
        .route("/api/v1/admin/users/{id}/clear-profile", axum::routing::post(member::clear_profile))
        .route("/api/v1/admin/users/{id}/warn", axum::routing::post(member::warn))
        .route("/api/v1/admin/users/{id}/notes", axum::routing::post(member::add_note))
        .route("/api/v1/admin/users/{id}/notes/{note}", axum::routing::delete(member::delete_note))
        .route("/api/v1/admin/users/{id}/ban", axum::routing::post(member::ban))
        .route("/api/v1/admin/users/{id}/api-keys/{key}", axum::routing::delete(member::revoke_key))
        .route("/api/v1/admin/users/{id}/identities/{provider}", axum::routing::delete(member::unlink_identity))
        .route("/api/v1/admin/users/{id}/export", axum::routing::post(member::export))
        .route("/api/v1/admin/tags", get(list_tags).post(create_tag))
        .route("/api/v1/admin/tags/{id}", patch(rename_tag).delete(delete_tag))
        .route("/api/v1/admin/search-report", get(search_report))
        .route("/api/v1/admin/mail", get(mail_log))
        .route("/api/v1/admin/mail/status", get(mail_status))
        .route("/api/v1/admin/mail/bad", get(bad_emails))
        .route("/api/v1/admin/mail/bad/{id}", axum::routing::delete(clear_bad_email))
        .route("/api/v1/webhooks/bounce", axum::routing::post(bounce_webhook))
        .route("/api/v1/admin/nodes/{id}/rules", put(set_rules))
        .route("/api/v1/admin/nodes/{id}/permissions/copy", axum::routing::post(copy_permissions))
        .route("/api/v1/admin/shell", get(shell))
        .route("/api/v1/admin/failed-logins", get(failed_logins))
        .route("/api/v1/admin/groups/{id}/require-2fa", put(require_2fa))
        .route("/api/v1/admin/profile-fields", get(list_fields).post(create_field))
        .route("/api/v1/admin/profile-fields/order", put(reorder_fields))
        .route("/api/v1/admin/profile-fields/{id}", put(update_field).delete(delete_field))
        .route("/api/v1/admin/groups/{id}/limits", put(set_group_limits))
        .route("/api/v1/admin/nodes/{id}/prefixes", put(set_prefixes))
}

pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A signed-in administrator, checked by `admin` before the body is read: the acting user id
/// (`None` for the bootstrap token) and the viewer. `WRITE` also needs CSRF and a recent sudo.
pub struct Admin<const WRITE: bool>(pub Option<i64>, pub Viewer);
pub type AdminRead = Admin<false>;
pub type AdminWrite = Admin<true>;

impl<const WRITE: bool> FromRequestParts<AppState> for Admin<WRITE> {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, AppError> {
        let v = Viewer::from_request_parts(parts, state).await?;
        Ok(Admin(admin(state, &v, &parts.headers, WRITE)?, v))
    }
}

/// Returns the acting user id (`None` for the bootstrap token) or 403.
pub(crate) fn admin(state: &AppState, v: &Viewer, headers: &HeaderMap, write: bool) -> Result<Option<i64>, AppError> {
    let bearer =
        headers.get(header::AUTHORIZATION).and_then(|h| h.to_str().ok()).and_then(|h| h.strip_prefix("Bearer "));
    // `bf_` tokens are API keys, already resolved by the viewer; they need the admin scope.
    if bearer.is_some_and(|b| b.starts_with("bf_")) {
        return match (&v.user, v.api_scope.as_deref()) {
            // The same two-factor rule as the panel: a key does not get round it.
            (Some(u), Some("admin")) if u.is_admin && u.needs_2fa => {
                Err(AppError::Validation("Set up two-factor authentication before using the admin panel.".into()))
            }
            (Some(u), Some("admin")) if u.is_admin => Ok(Some(u.id.get())),
            _ => Err(AppError::Forbidden),
        };
    }
    if let (Some(given), Some(expected)) = (bearer, state.config.admin_token.as_deref()) {
        return if constant_time_eq(given.as_bytes(), expected.as_bytes()) {
            Ok(None)
        } else {
            Err(AppError::Forbidden)
        };
    }
    let user = v.require_user()?;
    if !user.is_admin {
        return Err(AppError::Forbidden);
    }
    if user.needs_2fa {
        return Err(AppError::Validation("Set up two-factor authentication before using the admin panel.".into()));
    }
    if write {
        v.check_csrf_headers(headers)?;
        v.require_sudo()?;
    }
    Ok(Some(user.id.get()))
}

/// One line of the staff log: who did what to what, with the details as JSON.
pub(crate) async fn audit_log(
    pool: &sqlx::PgPool,
    who: Option<i64>,
    action: &str,
    target: &str,
    details: serde_json::Value,
) -> Result<(), AppError> {
    bf_db::admin::audit::record(pool, bf_db::admin::audit::Entry::new(action).by(who).on(target).details(details))
        .await
        .map_err(AppError::internal)
}

/// The audit log is append-only and outlives accounts, so personal values go into it masked:
/// `a***@example.com`.
pub(crate) fn mask_email(email: &str) -> String {
    email
        .split_once('@')
        .map_or_else(|| "***".to_owned(), |(a, d)| format!("{}***@{d}", a.chars().next().unwrap_or('*')))
}

/// `203.0.113.x` / `2001:db8:x`: enough to tell a range apart, not a person.
pub(crate) fn mask_ip(ip: &str) -> String {
    if let Some((head, _)) = ip.rsplit_once('.') {
        format!("{head}.x")
    } else if let Some((head, _)) = ip.rsplit_once(':') {
        format!("{head}:x")
    } else {
        "***".to_owned()
    }
}

/// A username or display name for the audit log: first letter and length only.
pub(crate) fn mask_name(name: &str) -> String {
    format!("{}***", name.chars().next().map(String::from).unwrap_or_default())
}

/// A ban's value by its kind (`email` or `ip`; anything else is kept as is).
pub(crate) fn mask_ban_value(kind: &str, value: &str) -> String {
    match kind {
        "email" => mask_email(value),
        "ip" => mask_ip(value),
        _ => value.to_owned(),
    }
}

/// Token calls have no user; the audit log records them with actor 0.
fn actor(a: Option<i64>) -> i64 {
    a.unwrap_or(0)
}

/// What the panel needs to show the site's own header: its HTML, styles and island loader.
async fn shell(State(state): State<AppState>, Admin(_, v): AdminRead) -> Result<Response, AppError> {
    let (l, set) = crate::http::render::layout(&state, &v, None);
    let css: Vec<String> = std::iter::once(l.css_url.clone()).chain(l.theme.as_ref().map(|t| t.url.clone())).collect();
    let loader = l.loader.clone();
    // The site's scripts (the header's menus, the Markdown editor) read the CSRF token from a meta
    // tag the panel's own page does not have, so it comes with the shell and the panel adds the tag.
    let csrf = l.csrf.clone();
    // The full Markdown editor, for the panel's own long texts (legal documents).
    let editor = l.editor_url.clone();
    // The header's own layout is in the critical styles, which pages inline rather than link.
    let critical = l.critical_css;
    // The islands' messages (`js.` keys) and the panel's own (`admin.`), in the admin's language.
    let messages = l.client_messages();
    // The panel's own texts, and the counted phrases ("{n} members") it shares with the site.
    let mut admin_messages = l.t.messages("admin.");
    admin_messages.extend(l.t.messages("count."));
    let (locale, dir) = (l.t.locale, l.t.dir());
    let html = bf_web::pages::render(&bf_web::pages::HeaderFragment { layout: l }).into_response();
    let html = axum::body::to_bytes(html.into_body(), usize::MAX).await.map_err(AppError::internal)?;
    let html = String::from_utf8_lossy(&html).into_owned();
    Ok(crate::http::render::page(
        Json(json!({ "html": html, "critical": critical, "css": css, "loader": loader, "editor": editor, "csrf": csrf,
            "messages": messages, "admin_messages": admin_messages, "locale": locale, "dir": dir }))
        .into_response(),
        set,
        false,
    ))
}
