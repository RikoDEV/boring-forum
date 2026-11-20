//! Password reset and email verification.

use crate::account::settings::Form1;
use crate::account::settings::field;
use crate::account::settings::form_page;
use crate::http::render::DbResult;
use crate::http::viewer::{CsrfForm, Viewer};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;
use axum::{Form, Router};
use bf_core::i18n::en;
use bf_core::{AppError, auth, mail};
use bf_db::accounts::security;
use bf_db::platform::mail as outbox;
use serde::Deserialize;
use sqlx::PgPool;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/forgot", get(forgot_form).post(forgot))
        .route("/reset/{token}", get(reset_form).post(reset))
        .route("/verify/{token}", get(verify_confirm).post(verify))
}

/// Queues a verification or reset email with a fresh one-time token.
pub(crate) async fn send_token_mail(
    state: &AppState,
    pool: &PgPool,
    kind: mail::Kind,
    user_id: bf_core::ids::UserId,
    username: &str,
    email: &str,
) -> Result<(), AppError> {
    let token = auth::random_token();
    let (purpose, ttl) = match kind {
        mail::Kind::Verify => ("verify", 24 * 60),
        mail::Kind::Reset => ("reset", 60),
    };
    security::create_email_token(pool, &auth::token_digest(&token), user_id, purpose, None, ttl).await.db()?;
    let s = state.settings();
    let m = mail::account_email(kind, &s, &state.config.base_url, email, username, &token);
    // The mail in the member's language: the catalog text, or the one an administrator wrote.
    let key = if matches!(kind, mail::Kind::Verify) { "verify" } else { "reset" };
    let locale: Option<String> =
        sqlx::query_scalar("SELECT locale FROM users WHERE id = $1").bind(user_id).fetch_one(pool).await.db()?;
    let link = format!("{}/{key}/{token}", state.config.base_url);
    let vars = [("site", s.str("site_name")), ("username", username), ("link", link.as_str())];
    let locale = locale.unwrap_or_else(|| bf_core::i18n::site_language(s.str("default_locale")).to_owned());
    let (subject, body) = crate::admin::mail_templates::render(&bf_core::i18n::T::new(&locale), key, &vars);
    outbox::queue_mail(pool, &m.to, &subject, &body).await.db()
}

fn forgot_page(state: &AppState, v: &Viewer, message: Option<String>) -> Response {
    let f = Form1 {
        heading: en("Reset your password"),
        action: "/forgot".into(),
        fields: if message.is_some() { vec![] } else { vec![field("email", en("Email"), "email", "email")] },
        button: if message.is_some() { "" } else { en("Send reset link") },
        message,
        error: None,
        links: vec![(en("Back to log in"), "/login")],
    };
    form_page(state, v, f, StatusCode::OK)
}

async fn forgot_form(State(state): State<AppState>, v: Viewer) -> Response {
    forgot_page(&state, &v, None)
}

#[derive(Deserialize)]
pub(crate) struct EmailForm {
    pub(crate) csrf: String,
    pub(crate) email: String,
}

async fn forgot(State(state): State<AppState>, v: Viewer, Form(f): Form<EmailForm>) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let pool = state.db()?;
    // Throttled per address and per visitor, so many machines cannot flood one mailbox. A refusal
    // looks like a success: the answer must not tell whether the address has an account.
    let by_ip = crate::http::ratelimit::MAIL_BY_IP.allow(&format!("forgot:{}", crate::http::ratelimit::ip_key(v.ip)));
    let by_mail = crate::http::ratelimit::MAIL_BY_ACCOUNT.allow(&format!("reset:{}", f.email.trim().to_lowercase()));
    // Always sent: old links are retired by the newest ones, so the form cannot be used to
    // exhaust an account's links and stop its owner's mail.
    if by_ip
        && by_mail
        && let Some(a) = security::account_by_email(pool, f.email.trim()).await.db()?
    {
        send_token_mail(&state, pool, mail::Kind::Reset, a.id, &a.username, &a.email).await?;
    }
    // Same answer either way, so the form cannot be used to find accounts.
    Ok(forgot_page(&state, &v, Some(v.t().msg("If an account uses that address, a reset link is on its way."))))
}

fn reset_page(state: &AppState, v: &Viewer, token: &str, error: Option<String>) -> Response {
    let status = if error.is_some() { StatusCode::UNPROCESSABLE_ENTITY } else { StatusCode::OK };
    let f = Form1 {
        heading: en("Choose a new password"),
        action: format!("/reset/{token}"),
        fields: vec![field("password", en("New password"), "password", "new-password")],
        button: en("Save password"),
        message: None,
        error,
        links: vec![],
    };
    form_page(state, v, f, status)
}

pub(crate) fn expired_link(state: &AppState, v: &Viewer) -> Response {
    let f = Form1 {
        heading: en("Link expired"),
        action: String::new(),
        fields: vec![],
        button: "",
        message: Some(v.t().msg("This link was already used or has expired.")),
        error: None,
        links: vec![(en("Request a new reset link"), "/forgot")],
    };
    form_page(state, v, f, StatusCode::GONE)
}

pub(crate) fn valid_token_shape(token: &str) -> bool {
    token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit())
}

async fn reset_form(State(state): State<AppState>, v: Viewer, Path(token): Path<String>) -> Result<Response, AppError> {
    let ok = valid_token_shape(&token)
        && security::email_token_valid(state.db()?, &auth::token_digest(&token), "reset").await.db()?;
    Ok(if ok { reset_page(&state, &v, &token, None) } else { expired_link(&state, &v) })
}

#[derive(Deserialize)]
struct PasswordForm {
    csrf: String,
    password: String,
}

async fn reset(
    State(state): State<AppState>,
    v: Viewer,
    Path(token): Path<String>,
    Form(f): Form<PasswordForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let pool = state.db()?;
    if !valid_token_shape(&token) {
        return Ok(expired_link(&state, &v));
    }
    let digest = auth::token_digest(&token);
    // Validate before using the token, so a weak password does not burn the link.
    let Some(owner) = security::email_token_owner(pool, &digest, "reset").await.db()? else {
        return Ok(expired_link(&state, &v));
    };
    // The same rules as changing it in settings, which include not containing the username.
    let username = bf_db::accounts::users::user_by_id(pool, owner).await.db()?.map(|u| u.username).unwrap_or_default();
    if let Some(e) = crate::auth::password_problem(&state, &f.password, &username) {
        return Ok(reset_page(&state, &v, &token, Some(e)));
    }
    if let Some(e) = crate::auth::breached::refuse(&state, &f.password).await {
        return Ok(reset_page(&state, &v, &token, Some(e.into())));
    }
    let Some((user_id, _)) = security::consume_email_token(pool, &digest, "reset").await.db()? else {
        return Ok(expired_link(&state, &v));
    };
    let password = f.password;
    let hash = tokio::task::spawn_blocking(move || auth::hash_password(&password)).await.map_err(AppError::internal)?;
    security::set_password(pool, user_id.into(), &hash, None).await.db()?;
    crate::platform::webhooks::fire(&state, "user.password_changed", serde_json::json!({ "user_id": user_id }));
    // The reset link proves control of the mailbox, which also verifies it.
    security::mark_email_verified(pool, user_id.into()).await.db()?;
    // Whoever holds the mailbox is the owner: sign-in locks and failure counts end here.
    security::clear_login_lockouts(pool, user_id.into()).await.db()?;
    crate::auth::mfa::finish_password_login(pool, user_id.into(), &v).await
}

/// The GET only shows a button: mail scanners follow links, and must not use up the token.
async fn verify_confirm(
    State(state): State<AppState>,
    v: Viewer,
    Path(token): Path<String>,
) -> Result<Response, AppError> {
    let owner = if valid_token_shape(&token) {
        security::email_token_owner(state.db()?, &auth::token_digest(&token), "verify").await.db()?
    } else {
        None
    };
    let Some(owner) = owner else { return Ok(verify_expired(&state, &v)) };
    if !v.user.as_ref().is_some_and(|u| u.id == owner) {
        return Ok(sign_in_to_confirm(&state, &v));
    }
    let f = Form1 {
        heading: en("Confirm your email"),
        action: format!("/verify/{token}"),
        fields: vec![],
        button: en("Confirm email address"),
        message: None,
        error: None,
        links: vec![],
    };
    Ok(form_page(&state, &v, f, StatusCode::OK))
}

/// A link that confirms an address counts only in a session of the account it was sent for.
/// Otherwise someone who registered with another person's address would have it confirmed by
/// that person's click, and the confirmed address would then vouch for an account its owner
/// cannot sign in to: a provider login with the same address joins such an account.
pub(crate) fn sign_in_to_confirm(state: &AppState, v: &Viewer) -> Response {
    let f = Form1 {
        heading: en("Log in to confirm"),
        action: String::new(),
        fields: vec![],
        button: "",
        message: Some(v.t().msg("Log in to the account this link was sent for, then open the link again.")),
        error: None,
        links: vec![(en("Log in"), "/login")],
    };
    form_page(state, v, f, StatusCode::FORBIDDEN)
}

fn verify_expired(state: &AppState, v: &Viewer) -> Response {
    let f = Form1 {
        heading: en("Link expired"),
        action: String::new(),
        fields: vec![],
        button: "",
        message: Some(v.t().msg("This verification link was already used or has expired.")),
        error: None,
        links: vec![(en("Account settings"), "/settings")],
    };
    form_page(state, v, f, StatusCode::GONE)
}

async fn verify(
    State(state): State<AppState>,
    v: Viewer,
    Path(token): Path<String>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&form.csrf)?;
    let pool = state.db()?;
    if !valid_token_shape(&token) {
        return Ok(verify_expired(&state, &v));
    }
    let digest = auth::token_digest(&token);
    // Looked at before it is used, so a click from anyone else does not burn the link.
    let Some(owner) = security::email_token_owner(pool, &digest, "verify").await.db()? else {
        return Ok(verify_expired(&state, &v));
    };
    if !v.user.as_ref().is_some_and(|u| u.id == owner) {
        return Ok(sign_in_to_confirm(&state, &v));
    }
    let Some((user_id, _)) = security::consume_email_token(pool, &digest, "verify").await.db()? else {
        return Ok(verify_expired(&state, &v));
    };
    security::mark_email_verified(pool, user_id.into()).await.db()?;
    let f = Form1 {
        heading: en("Email verified"),
        action: String::new(),
        fields: vec![],
        button: "",
        message: Some(v.t().msg("Thanks, your email address is confirmed.")),
        error: None,
        links: vec![(en("Go to the forums"), "/")],
    };
    Ok(form_page(&state, &v, f, StatusCode::OK))
}
