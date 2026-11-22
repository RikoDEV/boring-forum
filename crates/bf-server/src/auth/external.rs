//! What an outside login (a provider, single sign-on) adds to an account, by the `auth_*` settings:
//! the provider's profile copied in at sign-in, the page that asks a new member to confirm their
//! account, and the click that starts a login.

use crate::auth::oauth::{self, ExternalUser};
use crate::http::render::{DbResult, page};
use crate::http::viewer::{Viewer, csrf_for};
use crate::state::AppState;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Form, Router};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bf_core::ids::UserId;
use bf_core::{AppError, auth};
use bf_db::accounts::{preferences, security, users};
use bf_web::pages::{AuthConfirmPage, AuthContinuePage, Layout, render};
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub fn router() -> Router<AppState> {
    Router::new().route("/auth/confirm", get(confirm_page).post(confirm))
}

/// A field of the account that a provider can own, by the `auth_overrides_*` settings.
#[derive(Clone, Copy)]
pub(crate) enum Managed {
    Email,
    Username,
    Name,
    Avatar,
}

impl Managed {
    fn setting(self) -> &'static str {
        match self {
            Self::Email => "auth_overrides_email",
            Self::Username => "auth_overrides_username",
            Self::Name => "auth_overrides_name",
            Self::Avatar => "auth_overrides_avatar",
        }
    }
}

/// Whether the provider owns this field of the member's account: the site turned the override on
/// and the member signs in through at least one outside provider.
pub(crate) async fn managed(state: &AppState, user_id: UserId, what: Managed) -> Result<bool, AppError> {
    if !state.settings().bool(what.setting()) {
        return Ok(false);
    }
    security::has_identity(state.db()?, user_id).await.db()
}

/// The message for a refused change to a field the provider owns.
pub(crate) fn managed_message(v: &Viewer) -> String {
    v.t().msg("This is set by your outside login and cannot be changed here.")
}

/// The provider's profile, copied into the account after a sign-in. `created` is a sign-up just now:
/// the name and picture fill an account that has neither, whatever the overrides say. On a later
/// sign-in each field follows the provider only when its override is on. A failure here is logged
/// and never stops the sign-in.
pub(crate) async fn sync_profile(state: &AppState, user_id: UserId, provider: &str, ext: &ExternalUser, created: bool) {
    if let Err(e) = sync(state, user_id, provider, ext, created).await {
        tracing::warn!(error = %e, provider, "copying the provider's profile failed");
    }
}

async fn sync(
    state: &AppState,
    user_id: UserId,
    provider: &str,
    ext: &ExternalUser,
    created: bool,
) -> Result<(), AppError> {
    let pool = state.db()?;
    let s = state.settings();
    let Some(acct) = security::account(pool, user_id).await.db()? else { return Ok(()) };
    let mut changed = false;

    if !created
        && s.bool("auth_overrides_email")
        && ext.email_verified
        && let Some(email) = ext.email.as_deref().and_then(auth::normalize_email)
        && !email.eq_ignore_ascii_case(&acct.email)
        && users::change_email(pool, user_id, &email).await.db()?
    {
        crate::platform::webhooks::fire(state, "user.email_changed", serde_json::json!({ "user_id": user_id }));
        changed = true;
    }

    if !created
        && s.bool("auth_overrides_username")
        && let Some(name) = ext.username.as_deref().and_then(oauth::clean_username)
        && name != acct.username
        && auth::validate_username_for(&name, s.bool("unicode_usernames")).is_ok()
        && !users::skeleton_taken(pool, &auth::username_skeleton(&name), Some(user_id.get())).await.db()?
        && crate::moderation::gate::username_ok(&state.forum.get(pool).await?.words, &name)
    {
        match users::rename(pool, user_id, &name).await {
            Ok(true) => changed = true,
            Ok(false) => {}
            Err(e) if users::is_skeleton_taken(&e) => {}
            Err(e) => return Err(crate::http::render::db_err(e)),
        }
    }

    if (created || s.bool("auth_overrides_name"))
        && let Some(full) = ext.full_name.as_deref().map(|n| n.trim().chars().take(40).collect::<String>())
        && !full.is_empty()
    {
        let current = preferences::profile_fields(pool, user_id).await.db()?.display_name;
        // A name that passes for staff is refused as it is when a member types it.
        if current.as_deref() != Some(full.as_str())
            && !users::display_name_impersonates(pool, &full, Some(user_id.get())).await.db()?
        {
            users::set_display_name(pool, user_id, &full).await.db()?;
            changed = true;
        }
    }

    if (created || s.bool("auth_overrides_avatar"))
        && let Some(url) = ext.avatar_url.as_deref().filter(|u| u.starts_with("https://") && u.len() <= 1000)
        && security::identity_avatar(pool, provider, &ext.subject).await.db()?.as_deref() != Some(url)
        && import_avatar(state, user_id, url).await
    {
        security::set_identity_avatar(pool, provider, &ext.subject, url).await.db()?;
        changed = true;
    }
    if changed {
        state.microcache.clear();
    }
    Ok(())
}

/// Fetches the provider's picture and makes it the member's avatar. It goes through the checks any
/// upload does (type, size, scans, metadata), but not the daily upload allowance, which is for what a
/// member chooses to send. True when it was set.
async fn import_avatar(state: &AppState, user_id: UserId, url: &str) -> bool {
    use crate::forum::uploads::{Accept, Origin, store_from};
    let fetched =
        tokio::time::timeout(Duration::from_secs(5), crate::platform::outbound::fetch_public(url, 1 << 20)).await;
    let Ok(Ok(bytes)) = fetched else { return false };
    let Ok(pool) = state.db() else { return false };
    let Ok(stored) = store_from(state, user_id.get(), "avatar", &bytes, Accept::Images, 1024, Origin::Import).await
    else {
        return false;
    };
    bf_db::forum::uploads::set_avatar(pool, user_id.get(), Some(stored.upload.id)).await.is_ok()
}

// --- Starting a login

/// `auth_require_interaction`: a visit to `/auth/*` shows a button and only its click starts the login.
pub(crate) fn interaction_required(state: &AppState) -> bool {
    state.settings().bool("auth_require_interaction")
}

/// `auth_immediately`: the one outside login a guest is sent to straight away, when the site needs a
/// login to read anything and no other method is on offer.
pub(crate) fn immediate_login(state: &AppState) -> Option<&'static str> {
    let s = state.settings();
    if !(s.bool("auth_immediately") && s.bool("login_required")) {
        return None;
    }
    match oauth::enabled(&s).as_slice() {
        [(key, _)] => Some(key),
        _ => None,
    }
}

fn layout_for(state: &AppState, v: &Viewer, title: &str) -> (Layout, Option<HeaderValue>) {
    let mut l = Layout::new(&state.settings(), Some(title));
    l.t = v.t();
    l.set_nav(&state.settings(), &bf_core::ids::raw(&v.actor.groups), v.actor.is_admin);
    let (token, set) = csrf_for(v, true);
    l.csrf = token;
    l.noindex = true;
    l.user = v.user.as_ref().map(|u| u.username.clone());
    (l, set)
}

/// The page with the button that starts a login. With `auto` the page presses it itself (a script
/// does; without one the button is there), which is how `auth_immediately` works together with
/// `auth_require_interaction`.
pub(crate) fn continue_page(state: &AppState, v: &Viewer, key: &str, label: &str, auto: bool) -> Response {
    let t = v.t();
    let heading = t.f("auth.continue_with", &[("provider", label)]);
    let (l, set) = layout_for(state, v, &heading);
    let body = AuthContinuePage {
        layout: l,
        message: t.msgf("You are being sent to {n} to sign in there.", &[("n", label)]),
        heading,
        action: format!("/auth/{key}"),
        button: t.msg("Continue"),
        auto,
    };
    page((StatusCode::OK, render(&body)).into_response(), set, false)
}

// --- Confirming a new account

const SIGNUP_COOKIE: &str = "__Host-bf_signup";
const SEAL: &str = "bf-external-signup";

/// A new member's details, held in a signed cookie while they confirm the account.
#[derive(Serialize, Deserialize)]
struct Pending {
    provider: String,
    label: String,
    subject: String,
    email: String,
    verified: bool,
    name: String,
    username: Option<String>,
    full_name: Option<String>,
    avatar_url: Option<String>,
    exp: i64,
}

impl Pending {
    fn external(&self) -> ExternalUser {
        ExternalUser {
            subject: self.subject.clone(),
            email: Some(self.email.clone()),
            email_verified: self.verified,
            name: self.name.clone(),
            username: self.username.clone(),
            full_name: self.full_name.clone(),
            avatar_url: self.avatar_url.clone(),
        }
    }

    fn suggested_username(&self) -> String {
        oauth::username_base(self.username.as_deref().unwrap_or(&self.name))
    }
}

fn set_cookie(res: &mut Response, value: &str, max_age: i64) {
    let c = format!("{SIGNUP_COOKIE}={value}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age={max_age}");
    if let Ok(c) = HeaderValue::from_str(&c) {
        res.headers_mut().append(header::SET_COOKIE, c);
    }
}

/// Sends a new member to confirm the account before it is made (`auth_skip_create_confirm` off).
pub(crate) fn ask_to_confirm(provider: &str, label: &str, ext: &ExternalUser) -> Response {
    let pending = Pending {
        provider: provider.to_owned(),
        label: label.to_owned(),
        subject: ext.subject.clone(),
        email: ext.email.clone().unwrap_or_default(),
        verified: ext.email_verified,
        name: ext.name.clone(),
        username: ext.username.clone(),
        full_name: ext.full_name.clone(),
        avatar_url: ext.avatar_url.clone(),
        exp: chrono::Utc::now().timestamp() + 600,
    };
    let json = serde_json::to_string(&pending).unwrap_or_default();
    let mut res = Redirect::to("/auth/confirm").into_response();
    set_cookie(&mut res, &crate::auth::register::seal(SEAL, &URL_SAFE_NO_PAD.encode(json)), 600);
    res
}

fn pending(headers: &HeaderMap) -> Result<Pending, AppError> {
    let expired = || AppError::BadRequest("Your login attempt expired. Please try again.".into());
    let raw = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|h| h.to_str().ok())
        .flat_map(|h| h.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == SIGNUP_COOKIE)
        .map(|(_, v)| v.to_owned())
        .ok_or_else(expired)?;
    let data = crate::auth::register::unseal(SEAL, &raw).ok_or_else(expired)?;
    let json = URL_SAFE_NO_PAD.decode(data).map_err(|_| expired())?;
    let p: Pending = serde_json::from_slice(&json).map_err(|_| expired())?;
    if p.exp < chrono::Utc::now().timestamp() {
        return Err(expired());
    }
    Ok(p)
}

fn confirm_form(state: &AppState, v: &Viewer, p: &Pending, username: &str, error: Option<String>) -> Response {
    let t = v.t();
    let title = t.msg("Create your account");
    let (l, set) = layout_for(state, v, &title);
    let status = if error.is_some() { StatusCode::UNPROCESSABLE_ENTITY } else { StatusCode::OK };
    let body = AuthConfirmPage {
        layout: l,
        heading: title,
        message: t.msgf("You are signing up with {n}. Check the details and create your account.", &[("n", &p.label)]),
        error,
        username: username.to_owned(),
        email: p.email.clone(),
        full_name: p.full_name.clone().unwrap_or_default(),
    };
    page((status, render(&body)).into_response(), set, false)
}

async fn confirm_page(State(state): State<AppState>, v: Viewer, headers: HeaderMap) -> Result<Response, AppError> {
    let p = pending(&headers)?;
    Ok(confirm_form(&state, &v, &p, &p.suggested_username(), None))
}

#[derive(Deserialize)]
struct ConfirmForm {
    csrf: String,
    username: String,
}

async fn confirm(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Form(f): Form<ConfirmForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let p = pending(&headers)?;
    let pool = state.db()?;
    let name = auth::normalize_username(&f.username);
    let problem = match auth::validate_username_for(&name, state.settings().bool("unicode_usernames")) {
        Err(e) => Some(v.t().msg(e)),
        Ok(_) => match crate::auth::username_problem(&state, &name) {
            Some(m) => Some(m),
            None if users::skeleton_taken(pool, &auth::username_skeleton(&name), None).await.db()? => {
                Some(v.t().msg("That username is taken."))
            }
            None if !crate::moderation::gate::username_ok(&state.forum.get(pool).await?.words, &name) => {
                Some(v.t().msg("That username is not allowed."))
            }
            None => None,
        },
    };
    if problem.is_some() {
        return Ok(confirm_form(&state, &v, &p, &f.username, problem));
    }
    let mut res = oauth::sign_in_external(&state, &v, &p.provider, &p.label, &p.external(), Some(&name)).await?;
    set_cookie(&mut res, "", 0);
    Ok(res)
}
