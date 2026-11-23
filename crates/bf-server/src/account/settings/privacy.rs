//! Connected logins, who may message the member, the muted-members list and remembered devices.

use crate::http::render::DbResult;
use crate::http::viewer::{CsrfForm, Viewer};
use crate::state::AppState;
use axum::Form;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use bf_core::AppError;
use bf_db::accounts::{preferences, security};
use bf_db::community::follows;
use bf_web::pages::{LinkedLogin, LoginProvider};
use serde::Deserialize;

use super::settings_page;

/// Everything the settings page shows for these sections.
pub(super) struct PrivacyView {
    pub(super) identities: Vec<LinkedLogin>,
    /// False when the account has no other way to sign in: the last linked login stays.
    pub(super) identity_removable: bool,
    /// Providers not connected yet: (provider key, label).
    pub(super) connectable: Vec<LoginProvider>,
    pub(super) allow_pm_from: String,
    /// (id, username)
    pub(super) muted_users: Vec<(i64, String)>,
    pub(super) trusted_devices: i64,
}

pub(super) async fn view(
    state: &AppState,
    user: bf_core::ids::UserId,
    has_password: bool,
    passkeys: usize,
) -> Result<PrivacyView, AppError> {
    let pool = state.db()?;
    let labels = crate::auth::oauth::enabled(&state.settings());
    let rows = security::identity_rows(pool, user).await.db()?;
    let identity_removable = has_password || passkeys > 0 || rows.len() > 1;
    let connectable = crate::auth::oauth::linkable(&state.settings())
        .into_iter()
        .filter(|(key, _)| !rows.iter().any(|(linked, _, _)| linked.as_str() == *key))
        .map(|(key, label)| provider(key, label))
        .collect();
    let identities = rows
        .into_iter()
        .map(|(key, email, at)| {
            let label = labels.iter().find(|(k, _)| *k == key).map_or_else(|| known_label(&key), |(_, l)| l.clone());
            LinkedLogin { provider: provider(&key, label), email, linked: crate::http::render::ago(at) }
        })
        .collect();
    Ok(PrivacyView {
        identities,
        identity_removable,
        connectable,
        allow_pm_from: preferences::allow_pm_from(pool, user).await.db()?,
        muted_users: follows::ignored_list(pool, user.get()).await.db()?,
        trusted_devices: security::trusted_device_count(pool, user).await.db()?,
    })
}

/// A provider's name when the site no longer lists it among the enabled ones (it was switched off
/// after this member linked it).
fn known_label(key: &str) -> String {
    match key {
        "google" => "Google",
        "github" => "GitHub",
        "discord" => "Discord",
        "microsoft" => "Microsoft",
        "twitch" => "Twitch",
        "x" => "X",
        other => other,
    }
    .to_owned()
}

/// The logo the sprite has for a provider, or a key for the ones it has none for (a generic OpenID
/// provider, the site's own single sign-on).
fn provider(key: &str, label: String) -> LoginProvider {
    let icon = match key {
        "google" | "github" | "discord" | "microsoft" | "twitch" | "x" => format!("brand-{key}"),
        _ => "key".to_owned(),
    };
    LoginProvider { key: key.to_owned(), label, icon }
}

/// Unlinks a provider, unless it is the last way to sign in.
pub(super) async fn unlink_identity(
    State(state): State<AppState>,
    v: Viewer,
    Path(provider): Path<String>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    v.check_session_csrf(&f.csrf)?;
    if let Some(confirm) = v.sudo_redirect("/settings") {
        return Ok(confirm);
    }
    let user = v.require_user()?;
    match security::unlink_identity(state.db()?, user.id, &provider).await.db()? {
        security::Unlinked::Done => {
            settings_page(&state, &v, false, Some(v.t().msg("That login was unlinked.")), None).await
        }
        security::Unlinked::Missing => Err(AppError::NotFound),
        security::Unlinked::LastMethod => {
            settings_page(
                &state,
                &v,
                false,
                None,
                Some(v.t().msg("That is your only way to sign in. Set a password or add a passkey first.")),
            )
            .await
        }
    }
}

#[derive(Deserialize)]
pub(super) struct PrivacyForm {
    csrf: String,
    allow_pm_from: String,
}

pub(super) async fn save_privacy(
    State(state): State<AppState>,
    v: Viewer,
    Form(f): Form<PrivacyForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    if !["everyone", "following", "nobody"].contains(&f.allow_pm_from.as_str()) {
        return Err(AppError::BadRequest("unknown choice".into()));
    }
    preferences::set_allow_pm_from(state.db()?, user.id, &f.allow_pm_from).await.db()?;
    settings_page(&state, &v, false, Some(v.t().msg("Your privacy settings were saved.")), None).await
}

#[derive(Deserialize)]
pub(super) struct MuteForm {
    csrf: String,
    on: String,
}

/// Mutes or unmutes a member from the list. Unmuting comes back to the settings with an undo.
pub(super) async fn set_muted(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<MuteForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    if id == user.id {
        return Err(AppError::Validation("You cannot ignore yourself.".into()));
    }
    let pool = state.db()?;
    bf_db::accounts::users::user_by_id(pool, id.into()).await.db()?.ok_or(AppError::NotFound)?;
    let on = f.on == "1";
    follows::set_ignored(pool, user.id.get(), id, on).await.db()?;
    state.microcache.clear();
    let to = if on { "/settings#s-privacy".to_owned() } else { format!("/settings?unmuted={id}#s-privacy") };
    Ok(Redirect::to(&to).into_response())
}

/// Forgets every remembered device, so the next sign-in on each asks for the code again.
pub(super) async fn forget_devices(
    State(state): State<AppState>,
    v: Viewer,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    v.check_session_csrf(&f.csrf)?;
    let user = v.require_user()?;
    security::forget_devices(state.db()?, user.id).await.db()?;
    settings_page(&state, &v, false, Some(v.t().msg("All remembered devices were forgotten.")), None).await
}
