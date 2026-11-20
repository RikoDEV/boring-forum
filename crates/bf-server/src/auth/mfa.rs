//! Two-factor sign-in.

use crate::account::settings::Form1;
use crate::account::settings::field;
use crate::account::settings::form_page;
use crate::account::settings::now_unix;
use crate::auth::login::start_session;
use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Form, Router};
use bf_core::i18n::en;
use bf_core::{AppError, auth, totp};
use bf_db::accounts::security;
use serde::Deserialize;
use sqlx::PgPool;

pub fn router() -> Router<AppState> {
    Router::new().route("/login/2fa", get(mfa_form).post(mfa))
}

// `__Host-`: a sibling subdomain cannot plant one, as with the session cookie.
const MFA_COOKIE: &str = "__Host-bf_mfa";
/// Names a device that proved the second factor and may skip it for 30 days.
const DEVICE_COOKIE: &str = "__Host-bf_device";

/// A session's rights changed: the same session goes on under a new token.
pub(crate) async fn rotated(
    state: &AppState,
    v: &Viewer,
    res: Result<Response, AppError>,
) -> Result<Response, AppError> {
    let mut res = res?;
    if let Some(old) = v.session_digest.as_deref() {
        let token = auth::random_token();
        bf_db::accounts::sessions::rotate_session(state.db()?, old, &auth::token_digest(&token)).await.db()?;
        state.sessions.invalidate(old);
        res.headers_mut().append(header::SET_COOKIE, crate::auth::login::session_cookie(&token, 30 * 86_400));
    }
    Ok(res)
}

/// After a correct password: either a full session, or the two-factor step.
pub(crate) async fn finish_password_login(
    pool: &PgPool,
    user_id: bf_core::ids::UserId,
    v: &Viewer,
) -> Result<Response, AppError> {
    let ip = v.ip;
    let acct = security::account(pool, user_id).await.db()?.ok_or(AppError::NotFound)?;
    if acct.totp_unreadable {
        return Err(AppError::Validation(
            "Two-factor sign-in is unavailable right now. Please contact the site administrator.".into(),
        ));
    }
    if acct.totp_secret.is_none() {
        return start_session(pool, user_id, ip).await;
    }
    // A device that passed the second step in the last 30 days goes straight in.
    if let Some(device) = crate::http::viewer::cookie(&v.theme_headers, DEVICE_COOKIE)
        && security::device_trusted(pool, user_id, &auth::token_digest(device)).await.db()?
    {
        return start_session(pool, user_id, ip).await;
    }
    let token = auth::random_token();
    security::create_pending_login(pool, &auth::token_digest(&token), user_id).await.db()?;
    let mut res = Redirect::to("/login/2fa").into_response();
    let cookie = format!("{MFA_COOKIE}={token}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=300");
    res.headers_mut()
        .append(header::SET_COOKIE, HeaderValue::from_str(&cookie).unwrap_or_else(|_| HeaderValue::from_static("")));
    Ok(res)
}

/// Tells plugins (`user.login`: `{user_id, method}`) that a session started. A response that
/// only sends the person on to the two-factor step is not a login yet, so it is passed through.
pub(crate) fn logged_in(
    state: &crate::state::AppState,
    user_id: bf_core::ids::UserId,
    method: &str,
    res: Response,
) -> Response {
    let to_2fa = res.headers().get(header::LOCATION).is_some_and(|l| l.as_bytes() == b"/login/2fa");
    if !to_2fa {
        crate::platform::webhooks::fire(
            state,
            "user.login",
            serde_json::json!({ "user_id": user_id, "method": method }),
        );
    }
    res
}

fn mfa_cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == MFA_COOKIE)
        .map(|(_, v)| v.to_owned())
}

fn mfa_page(state: &AppState, v: &Viewer, error: Option<String>) -> Response {
    let status = if error.is_some() { StatusCode::UNPROCESSABLE_ENTITY } else { StatusCode::OK };
    let f = Form1 {
        heading: en("Two-factor code"),
        action: "/login/2fa".into(),
        fields: vec![
            field("code", en("6-digit code, or a backup code"), "text", "one-time-code"),
            field("remember", en("Remember this device for 30 days"), "checkbox", "off"),
        ],
        button: en("Continue"),
        message: None,
        error,
        links: vec![(en("Start over"), "/login")],
    };
    form_page(state, v, f, status)
}

async fn mfa_form(State(state): State<AppState>, v: Viewer, headers: HeaderMap) -> Response {
    if mfa_cookie(&headers).is_none() {
        return Redirect::to("/login").into_response();
    }
    mfa_page(&state, &v, None)
}

/// Is this an authenticator code that has not been used, or a backup code that has not been
/// spent? Either is used up by the check. Callers count the attempt first (`begin_mfa_attempt`).
pub(crate) async fn code_accepted(
    pool: &PgPool,
    user_id: bf_core::ids::UserId,
    secret: &str,
    last_step: Option<i64>,
    code: &str,
) -> Result<bool, AppError> {
    match totp::verify(secret, code, now_unix(), last_step.map(|s| s as u64)) {
        Some(s) => security::advance_totp_step(pool, user_id, s as i64).await.db(),
        // A backup code works once when the authenticator is not at hand.
        None => match backup_code_form(code) {
            Some(c) => security::use_backup_code(pool, user_id, &auth::token_digest(&c)).await.db(),
            None => Ok(false),
        },
    }
}

/// A backup code in the form its digest is stored for: lower-case hex in groups, whatever case,
/// spaces or dashes it was typed with. Twenty characters in fives (80 bits) are the codes handed
/// out now; eight in fours are older ones, which still work. `None` for anything else.
pub(crate) fn backup_code_form(typed: &str) -> Option<String> {
    let hex: String =
        typed.chars().filter(|c| !c.is_whitespace() && *c != '-').map(|c| c.to_ascii_lowercase()).collect();
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let group = match hex.len() {
        20 => 5,
        8 => 4,
        _ => return None,
    };
    let groups: Vec<&str> = (0..hex.len()).step_by(group).map(|i| &hex[i..i + group]).collect();
    Some(groups.join("-"))
}

/// Whether this browser holds the member's "remember this device" cookie.
pub(crate) async fn device_remembered(
    pool: &PgPool,
    user_id: bf_core::ids::UserId,
    v: &Viewer,
) -> Result<bool, AppError> {
    match crate::http::viewer::cookie(&v.theme_headers, DEVICE_COOKIE) {
        Some(device) => security::device_trusted(pool, user_id, &auth::token_digest(device)).await.db(),
        None => Ok(false),
    }
}

#[derive(Deserialize)]
pub(crate) struct CodeForm {
    pub(crate) csrf: String,
    pub(crate) code: String,
    /// "1" when the box "remember this device" was ticked.
    #[serde(default)]
    pub(crate) remember: String,
}

async fn mfa(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Form(f): Form<CodeForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let pool = state.db()?;
    let Some(token) = mfa_cookie(&headers) else { return Ok(Redirect::to("/login").into_response()) };
    let digest = auth::token_digest(&token);
    let Some(user_id) = security::pending_login_user(pool, &digest).await.db()? else {
        return Ok(Redirect::to("/login").into_response());
    };
    // Decided and counted in one statement, so parallel guesses cannot all slip past the lock.
    if !security::begin_mfa_attempt(pool, user_id).await.db()? {
        return Ok(mfa_page(&state, &v, Some(v.t().msg("Too many wrong codes. Try again in 15 minutes."))));
    }
    let acct = security::account(pool, user_id).await.db()?.ok_or(AppError::NotFound)?;
    // Never check against an empty key: anyone could compute those codes.
    let Some(secret) = acct.totp_secret.as_deref().filter(|s| !s.is_empty()) else {
        return Ok(Redirect::to("/login").into_response());
    };
    if !code_accepted(pool, user_id, secret, acct.totp_last_step, &f.code).await? {
        return Ok(mfa_page(&state, &v, Some(v.t().msg("That code is not valid. Try the current one."))));
    }
    security::clear_mfa_failures(pool, user_id).await.db()?;
    security::delete_pending_login(pool, &digest).await.db()?;
    let mut res = start_session(pool, user_id, v.ip).await?;
    if f.remember == "1" {
        let device = auth::random_token();
        let agent = headers.get(header::USER_AGENT).and_then(|h| h.to_str().ok());
        security::trust_device(pool, user_id, &auth::token_digest(&device), agent).await.db()?;
        let days = i64::from(security::TRUSTED_DEVICE_DAYS);
        let cookie =
            format!("{DEVICE_COOKIE}={device}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age={}", days * 86_400);
        if let Ok(c) = HeaderValue::from_str(&cookie) {
            res.headers_mut().append(header::SET_COOKIE, c);
        }
    }
    crate::platform::webhooks::fire(&state, "user.login", serde_json::json!({ "user_id": user_id, "method": "2fa" }));
    let clear = format!("{MFA_COOKIE}=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0");
    res.headers_mut()
        .append(header::SET_COOKIE, HeaderValue::from_str(&clear).unwrap_or_else(|_| HeaderValue::from_static("")));
    Ok(res)
}
