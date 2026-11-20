//! Account security pages: password reset, email verification, account settings
//! (password, sessions, two-factor) and the second login step.
//! Letter avatars, follows, ignoring users, profile editing and notification preferences.

use crate::http::render::page;
use crate::http::viewer::{Viewer, csrf_for};
use crate::state::AppState;
use axum::Router;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use bf_web::pages::{FormField, FormPage, Layout, render};

mod account_security;
mod interface;
mod notification_prefs;
mod page;
mod privacy;
mod profile;

use self::account_security::{
    backup_codes, change_email, change_password, change_username, confirm_email, confirm_email_page, disable_2fa, enable_2fa,
    resend_verification, revoke, revoke_others,
};
use self::interface::{save_interface, set_language};
use self::notification_prefs::{save_notifications, set_dnd, set_quiet};
use self::page::{mask_email, settings, settings_page, settings_page_ex};
use self::privacy::{forget_devices, save_privacy, set_muted, unlink_identity};
use self::profile::{save_profile, save_signature};

pub(crate) use self::notification_prefs::{prefs_view, switchable};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/settings", get(settings))
        .route("/settings/password", post(change_password))
        .route("/settings/verify-email", post(resend_verification))
        .route("/settings/sessions/revoke", post(revoke))
        .route("/settings/sessions/revoke-others", post(revoke_others))
        .route("/settings/2fa/enable", post(enable_2fa))
        .route("/settings/2fa/disable", post(disable_2fa))
        .route("/settings/2fa/backup-codes", post(backup_codes))
        .route("/settings/email", post(change_email))
        .route("/confirm-email/{token}", get(confirm_email_page).post(confirm_email))
        .route("/settings/username", post(change_username))
        .route("/settings/interface", post(save_interface))
        .route("/language/{code}", get(set_language))
        .route("/settings/signature", post(save_signature))
        .route("/settings/dnd", post(set_dnd))
        .route("/settings/quiet", post(set_quiet))
        .route("/settings/profile", post(save_profile))
        .route("/settings/notifications", post(save_notifications))
        .route("/settings/identities/{provider}/unlink", post(unlink_identity))
        .route("/settings/privacy", post(save_privacy))
        .route("/settings/muted/{id}", post(set_muted))
        .route("/settings/2fa/forget-devices", post(forget_devices))
}

pub(crate) fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

pub(crate) fn field(
    name: &'static str,
    label: &'static str,
    kind: &'static str,
    autocomplete: &'static str,
) -> FormField {
    FormField { name, label, kind, autocomplete }
}

pub(crate) struct Form1 {
    pub(crate) heading: &'static str,
    pub(crate) action: String,
    pub(crate) fields: Vec<FormField>,
    pub(crate) button: &'static str,
    pub(crate) message: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) links: Vec<(&'static str, &'static str)>,
}

pub(crate) fn form_page(state: &AppState, v: &Viewer, f: Form1, status: StatusCode) -> Response {
    // The texts are marked English (`en`) where they are written, and translated here.
    let t = v.t();
    let heading = t.msg(f.heading);
    let mut l = Layout::new(&state.settings(), Some(&heading));
    l.t = t;
    l.set_nav(&state.settings(), &bf_core::ids::raw(&v.actor.groups), v.actor.is_admin);
    let (token, set) = csrf_for(v, true);
    l.csrf = token;
    l.noindex = true;
    l.user = v.user.as_ref().map(|u| u.username.clone());
    let body = FormPage {
        layout: l,
        heading,
        message: f.message.map(|m| t.msg(&m)),
        error: f.error.map(|e| t.msg(&e)),
        action: f.action,
        fields: f.fields,
        button: if f.button.is_empty() { String::new() } else { t.msg(f.button) },
        links: f.links,
    };
    page((status, render(&body)).into_response(), set, false)
}
