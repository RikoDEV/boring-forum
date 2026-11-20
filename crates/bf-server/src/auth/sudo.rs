//! Sudo mode: confirming the password before sensitive changes. The `/sudo` page serves forms;
//! `/api/v1/sudo` serves the admin panel, which asks in a dialog and retries what it was doing.

use crate::account::settings::Form1;
use crate::account::settings::field;
use crate::account::settings::form_page;
use crate::account::settings::now_unix;
use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Form, Json, Router};
use bf_core::i18n::en;
use bf_core::{AppError, auth, totp};
use bf_db::accounts::security;
use serde::Deserialize;

pub fn router() -> Router<AppState> {
    Router::new().route("/sudo", get(sudo_form).post(sudo)).route("/api/v1/sudo", get(api_method).post(api_confirm))
}

#[derive(Deserialize)]
struct SudoQuery {
    #[serde(default)]
    next: String,
}

#[derive(Deserialize)]
struct SudoForm {
    csrf: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    code: String,
}

/// Only back into this site. Control characters are refused too: browsers strip a tab or newline
/// from a `Location`, so `/<TAB>/evil.example` would become `//evil.example`.
fn sudo_target(next: &str) -> &str {
    if crate::platform::plugins::local_path(next) { next } else { "/settings" }
}

async fn sudo_page(
    state: &AppState,
    v: &Viewer,
    next: &str,
    error: Option<String>,
    status: StatusCode,
) -> Result<Response, AppError> {
    let user = v.require_user()?;
    let acct = security::account(state.db()?, user.id).await.db()?.ok_or(AppError::NotFound)?;
    let mut fields = Vec::new();
    if acct.password_hash.is_some() {
        fields.push(field("password", en("Password"), "password", "current-password"));
    } else if acct.totp_secret.is_some() {
        fields.push(field("code", en("Authenticator code"), "text", "one-time-code"));
    }
    let can_confirm = !fields.is_empty();
    let action: String = url::form_urlencoded::byte_serialize(next.as_bytes()).collect();
    let f = Form1 {
        heading: en("Confirm it's you"),
        action: format!("/sudo?next={action}"),
        fields,
        button: if can_confirm { en("Confirm") } else { "" },
        // Signed in by passkey or a provider only: signing in again is the proof.
        message: (!can_confirm)
            .then(|| en("This account has no password. Sign out and sign in again to continue.").to_owned()),
        error,
        links: vec![],
    };
    Ok(form_page(state, v, f, status))
}

async fn sudo_form(State(state): State<AppState>, v: Viewer, Query(q): Query<SudoQuery>) -> Result<Response, AppError> {
    sudo_page(&state, &v, sudo_target(&q.next), None, StatusCode::OK).await
}

/// Checks the password (or the code, for an account without one). On success the session is
/// rotated into a confirmed one, and the cookie to send is returned; on failure, why.
async fn confirm(
    state: &AppState,
    v: &Viewer,
    password: String,
    code: &str,
) -> Result<Result<axum::http::HeaderValue, &'static str>, AppError> {
    // The refusals are marked English (`en`) where they are made; the page and the error layer
    // translate them for the viewer.
    let user = v.require_user()?;
    if v.impersonating {
        return Err(AppError::Forbidden);
    }
    let pool = state.db()?;
    let acct = security::account(pool, user.id).await.db()?.ok_or(AppError::NotFound)?;
    if security::login_blocked_until(pool, Some(user.id), &user.username, v.ip).await.db()?.is_some()
        || security::mfa_locked(pool, user.id).await.db()?
    {
        return Ok(Err(en("Too many failed attempts. Try again later.")));
    }
    let ok = match (&acct.password_hash, &acct.totp_secret) {
        (Some(hash), _) => {
            let hash = hash.clone();
            tokio::task::spawn_blocking(move || auth::verify_password(&password, &hash))
                .await
                .map_err(AppError::internal)?
        }
        (None, Some(secret)) => {
            // The attempt is counted before the code is looked at (see `begin_mfa_attempt`).
            if !security::begin_mfa_attempt(pool, user.id).await.db()? {
                return Ok(Err(en("Too many failed attempts. Try again later.")));
            }
            match totp::verify(secret, code, now_unix(), acct.totp_last_step.map(|s| s as u64)) {
                Some(step) => security::advance_totp_step(pool, user.id, step as i64).await.db()?,
                None => false,
            }
        }
        (None, None) => false,
    };
    if !ok {
        if acct.password_hash.is_some() {
            security::record_failed_login(pool, &user.username, Some(user.id), v.ip).await.db()?;
        }
        return Ok(Err(en("That is not correct.")));
    }
    if acct.password_hash.is_none() {
        security::clear_mfa_failures(pool, user.id).await.db()?;
    }
    let old = v.session_digest.as_deref().ok_or(AppError::Forbidden)?;
    let token = auth::random_token();
    bf_db::accounts::sessions::confirm_session(pool, old, &auth::token_digest(&token)).await.db()?;
    state.sessions.invalidate(old);
    Ok(Ok(crate::auth::login::session_cookie(&token, 30 * 86_400)))
}

/// Confirms the password (or the code, without one), then returns where the member was going.
async fn sudo(
    State(state): State<AppState>,
    v: Viewer,
    Query(q): Query<SudoQuery>,
    Form(f): Form<SudoForm>,
) -> Result<Response, AppError> {
    v.check_session_csrf(&f.csrf)?;
    let next = sudo_target(&q.next);
    match confirm(&state, &v, f.password, &f.code).await? {
        Ok(cookie) => {
            let mut res = Redirect::to(next).into_response();
            res.headers_mut().append(header::SET_COOKIE, cookie);
            Ok(res)
        }
        Err(msg) => sudo_page(&state, &v, next, Some(msg.to_owned()), StatusCode::UNPROCESSABLE_ENTITY).await,
    }
}

/// What the dialog should ask for: `password`, `code` (an account with only an authenticator),
/// or `none` (passkey or provider sign-in only: signing in again is the proof).
async fn api_method(State(state): State<AppState>, v: Viewer) -> Result<Response, AppError> {
    let user = v.require_user()?;
    let acct = security::account(state.db()?, user.id).await.db()?.ok_or(AppError::NotFound)?;
    let method = if acct.password_hash.is_some() {
        "password"
    } else if acct.totp_secret.is_some() {
        "code"
    } else {
        "none"
    };
    Ok(Json(serde_json::json!({ "method": method, "confirmed": v.has_sudo() })).into_response())
}

#[derive(Deserialize)]
struct ConfirmBody {
    #[serde(default)]
    password: String,
    #[serde(default)]
    code: String,
}

/// The dialog's confirmation: 204 with the confirmed session's cookie, or 422 with the reason.
async fn api_confirm(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Json(b): Json<ConfirmBody>,
) -> Result<Response, AppError> {
    v.check_session_csrf_headers(&headers)?;
    match confirm(&state, &v, b.password, b.code.trim()).await? {
        Ok(cookie) => {
            let mut res = StatusCode::NO_CONTENT.into_response();
            res.headers_mut().append(header::SET_COOKIE, cookie);
            Ok(res)
        }
        Err(msg) => Err(AppError::Validation(msg.into())),
    }
}

#[cfg(test)]
mod tests {
    use super::sudo_target;

    #[test]
    fn sudo_target_stays_on_this_site() {
        assert_eq!(sudo_target("/settings/keys"), "/settings/keys");
        for bad in ["//evil.example", "/\t/evil.example", "/\n/evil.example", "/\\evil", "https://evil.example", ""] {
            assert_eq!(sudo_target(bad), "/settings", "{bad:?}");
        }
    }
}
