//! Password, sessions, two-factor, email address and username changes.

use crate::auth::mfa::CodeForm;
use crate::auth::mfa::rotated;
use crate::auth::recovery::send_token_mail;
use crate::auth::recovery::sign_in_to_confirm;
use crate::auth::recovery::valid_token_shape;
use crate::http::render::DbResult;
use crate::http::viewer::{CsrfForm, Viewer, is_staff};
use crate::state::AppState;
use axum::Form;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use bf_core::i18n::en;
use bf_core::{AppError, auth, mail, totp};
use bf_db::accounts::{security, sessions, users};
use bf_db::platform::mail as outbox;
use serde::Deserialize;

use super::{Form1, form_page, mask_email, now_unix, settings_page, settings_page_ex};

#[derive(Deserialize)]
pub(super) struct ChangePasswordForm {
    pub(super) csrf: String,
    pub(super) current: String,
    pub(super) new: String,
}

pub(super) async fn change_password(
    State(state): State<AppState>,
    v: Viewer,
    Form(f): Form<ChangePasswordForm>,
) -> Result<Response, AppError> {
    v.check_session_csrf(&f.csrf)?;
    if let Some(confirm) = v.sudo_redirect("/settings") {
        return Ok(confirm);
    }
    let user = v.require_user()?;
    let pool = state.db()?;
    let acct = security::account(pool, user.id).await.db()?.ok_or(AppError::NotFound)?;
    let (current, hash) = (f.current, acct.password_hash.unwrap_or_default());
    let ok = tokio::task::spawn_blocking(move || auth::verify_password(&current, &hash))
        .await
        .map_err(AppError::internal)?;
    if !ok {
        return settings_page(&state, &v, false, None, Some(v.t().msg("Your current password is not correct."))).await;
    }
    if let Some(e) = crate::auth::password_problem(&state, &f.new, &acct.username) {
        return settings_page(&state, &v, false, None, Some(e)).await;
    }
    if let Some(e) = crate::auth::breached::refuse(&state, &f.new).await {
        return settings_page(&state, &v, false, None, Some(e.into())).await;
    }
    let new = f.new;
    let hash = tokio::task::spawn_blocking(move || auth::hash_password(&new)).await.map_err(AppError::internal)?;
    security::set_password(pool, user.id, &hash, v.session_digest.as_deref()).await.db()?;
    crate::platform::webhooks::fire(
        &state,
        "user.password_changed",
        serde_json::json!({ "user_id": user.id, "username": acct.username }),
    );
    security_notice(&state, v.t(), &acct.email, &acct.username, &v.t().msg("Your password was changed")).await;
    let page =
        settings_page(&state, &v, false, Some(v.t().msg("Password changed. Other sessions were signed out.")), None)
            .await;
    rotated(&state, &v, page).await
}

pub(super) async fn resend_verification(
    State(state): State<AppState>,
    v: Viewer,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let pool = state.db()?;
    let acct = security::account(pool, user.id).await.db()?.ok_or(AppError::NotFound)?;
    if !acct.email_verified {
        crate::http::ratelimit::MAIL_BY_ACCOUNT.enforce(&format!("verify:{}", acct.id))?;
        send_token_mail(&state, pool, mail::Kind::Verify, acct.id, &acct.username, &acct.email).await?;
    }
    settings_page(
        &state,
        &v,
        false,
        Some(v.t().msgf("Verification email sent to {email}.", &[("email", &mask_email(&acct.email))])),
        None,
    )
    .await
}

#[derive(Deserialize)]
pub(super) struct RevokeForm {
    pub(super) csrf: String,
    pub(super) id: String,
}

pub(super) async fn revoke(
    State(state): State<AppState>,
    v: Viewer,
    Form(f): Form<RevokeForm>,
) -> Result<Response, AppError> {
    v.check_session_csrf(&f.csrf)?;
    let user = v.require_user()?;
    sessions::revoke_session(state.db()?, user.id, &f.id).await.db()?;
    crate::http::middleware::forget_sessions_of(&state, user.id.get());
    Ok(Redirect::to("/settings").into_response())
}

/// Signs out every session of this user except the one making the request.
pub(super) async fn revoke_others(
    State(state): State<AppState>,
    v: Viewer,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    v.check_session_csrf(&f.csrf)?;
    let user = v.require_user()?;
    if let Some(current) = v.session_digest.as_deref() {
        sessions::revoke_other_sessions(state.db()?, user.id, current).await.db()?;
        crate::http::middleware::forget_sessions_of(&state, user.id.get());
    }
    Ok(Redirect::to("/settings#s-sessions").into_response())
}

#[derive(Deserialize)]
pub(super) struct EnableForm {
    pub(super) csrf: String,
    pub(super) secret: String,
    pub(super) code: String,
}

pub(super) async fn enable_2fa(
    State(state): State<AppState>,
    v: Viewer,
    Form(f): Form<EnableForm>,
) -> Result<Response, AppError> {
    v.check_session_csrf(&f.csrf)?;
    if let Some(confirm) = v.sudo_redirect("/settings") {
        return Ok(confirm);
    }
    let user = v.require_user()?;
    let pool = state.db()?;
    // Replacing an active secret goes through disable, which demands a current code.
    let current = security::account(pool, user.id).await.db()?.ok_or(AppError::NotFound)?;
    if current.totp_secret.is_some() || current.totp_unreadable {
        return settings_page(&state, &v, false, None, Some(v.t().msg("Two-factor authentication is already on.")))
            .await;
    }
    // Until the address is confirmed the account may have been made by someone other than the
    // address's owner; a second factor of theirs would outlive the owner's password reset and
    // keep the owner out. An administrator is exempt: a new site may have no working mail yet.
    if !current.email_verified && !user.is_admin {
        return settings_page(
            &state,
            &v,
            false,
            None,
            Some(v.t().msg("Confirm your email address before turning on two-factor authentication.")),
        )
        .await;
    }
    // The secret round-trips through the form; it only takes effect once a code from
    // it is proven, so a tampered secret just locks the user out of a fresh setup.
    if totp::base32_decode(&f.secret).is_none_or(|k| k.len() < 16) {
        return Err(AppError::BadRequest("bad secret".into()));
    }
    let Some(step) = totp::verify(&f.secret, &f.code, now_unix(), None) else {
        return settings_page_ex(
            &state,
            &v,
            true,
            None,
            Some(v.t().msg("That code did not match. Try again.")),
            None,
            Some(f.secret),
            None,
        )
        .await;
    };
    security::set_totp(pool, user.id, Some(&f.secret), Some(step as i64)).await.db()?;
    // Turning 2FA on is the answer to "someone may have my session": the others end here.
    if let Some(current) = v.session_digest.as_deref() {
        sessions::revoke_other_sessions(pool, user.id, current).await.db()?;
        crate::http::middleware::forget_sessions_of(&state, user.id.get());
    }
    crate::admin::api::audit_log(pool, Some(user.id.get()), "account.2fa_on", &format!("user:{}", user.id), serde_json::json!({})).await?;
    let page = settings_page(&state, &v, false, Some(v.t().msg("Two-factor authentication is on.")), None).await;
    rotated(&state, &v, page).await
}

pub(super) async fn disable_2fa(
    State(state): State<AppState>,
    v: Viewer,
    Form(f): Form<CodeForm>,
) -> Result<Response, AppError> {
    v.check_session_csrf(&f.csrf)?;
    if let Some(confirm) = v.sudo_redirect("/settings") {
        return Ok(confirm);
    }
    let user = v.require_user()?;
    let pool = state.db()?;
    let acct = security::account(pool, user.id).await.db()?.ok_or(AppError::NotFound)?;
    let Some(secret) = acct.totp_secret else { return Ok(Redirect::to("/settings").into_response()) };
    // The same limits as signing in: attempts are counted and locked, a code works once, and a
    // backup code will do when the authenticator is lost.
    if !security::begin_mfa_attempt(pool, user.id).await.db()? {
        return settings_page(
            &state,
            &v,
            false,
            None,
            Some(v.t().msg("Too many wrong codes. Try again in 15 minutes.")),
        )
        .await;
    }
    if !crate::auth::mfa::code_accepted(pool, user.id, &secret, acct.totp_last_step, &f.code).await? {
        return settings_page(&state, &v, false, None, Some(v.t().msg("That code is not valid."))).await;
    }
    security::clear_mfa_failures(pool, user.id).await.db()?;
    security::set_totp(pool, user.id, None, None).await.db()?;
    if let Some(current) = v.session_digest.as_deref() {
        sessions::revoke_other_sessions(pool, user.id, current).await.db()?;
        crate::http::middleware::forget_sessions_of(&state, user.id.get());
    }
    crate::admin::api::audit_log(pool, Some(user.id.get()), "account.2fa_off", &format!("user:{}", user.id), serde_json::json!({})).await?;
    security_notice(&state, v.t(), &user.email, &user.username, &v.t().msg("Two-factor authentication was turned off"))
        .await;
    let page = settings_page(&state, &v, false, Some(v.t().msg("Two-factor authentication is off.")), None).await;
    rotated(&state, &v, page).await
}

/// Ten one-time backup codes (`xxxxx-xxxxx-xxxxx-xxxxx`, 80 bits), shown once; only their digests
/// are stored, in the form `auth::mfa::backup_code_form` gives what is typed at sign-in.
pub(super) async fn backup_codes(
    State(state): State<AppState>,
    v: Viewer,
    Form(f): Form<CodeForm>,
) -> Result<Response, AppError> {
    v.check_session_csrf(&f.csrf)?;
    if let Some(confirm) = v.sudo_redirect("/settings") {
        return Ok(confirm);
    }
    let user = v.require_user()?;
    let pool = state.db()?;
    let acct = security::account(pool, user.id).await.db()?.ok_or(AppError::NotFound)?;
    let Some(secret) = acct.totp_secret else {
        return Err(AppError::Validation("Turn on two-factor authentication first.".into()));
    };
    // Backup codes bypass the second factor, so minting them needs the second factor itself (sudo
    // may have been passed with the password alone): the same limits as `disable_2fa`.
    if !security::begin_mfa_attempt(pool, user.id).await.db()? {
        return settings_page(
            &state,
            &v,
            false,
            None,
            Some(v.t().msg("Too many wrong codes. Try again in 15 minutes.")),
        )
        .await;
    }
    if !crate::auth::mfa::code_accepted(pool, user.id, &secret, acct.totp_last_step, &f.code).await? {
        return settings_page(&state, &v, false, None, Some(v.t().msg("That code is not valid."))).await;
    }
    security::clear_mfa_failures(pool, user.id).await.db()?;
    let codes: Vec<String> = (0..10)
        .map(|_| {
            let t = auth::random_token();
            format!("{}-{}-{}-{}", &t[..5], &t[5..10], &t[10..15], &t[15..20])
        })
        .collect();
    let digests: Vec<String> = codes.iter().map(|c| auth::token_digest(c)).collect();
    security::set_backup_codes(pool, user.id, &digests).await.db()?;
    settings_page_ex(&state, &v, false, None, None, Some(codes), None, None).await
}

#[derive(Deserialize)]
pub(super) struct EmailChangeForm {
    pub(super) csrf: String,
    pub(super) email: String,
    pub(super) password: String,
}

/// Email change: confirmed by a link sent to the new address.
pub(super) async fn change_email(
    State(state): State<AppState>,
    v: Viewer,
    Form(f): Form<EmailChangeForm>,
) -> Result<Response, AppError> {
    v.check_session_csrf(&f.csrf)?;
    if let Some(confirm) = v.sudo_redirect("/settings") {
        return Ok(confirm);
    }
    let user = v.require_user()?;
    let pool = state.db()?;
    let acct = security::account(pool, user.id).await.db()?.ok_or(AppError::NotFound)?;
    let (pw, hash) = (f.password, acct.password_hash.clone().unwrap_or_default());
    if !tokio::task::spawn_blocking(move || auth::verify_password(&pw, &hash)).await.map_err(AppError::internal)? {
        return settings_page(&state, &v, false, None, Some(v.t().msg("Your password is not correct."))).await;
    }
    let Some(email) = auth::normalize_email(&f.email) else {
        return settings_page(&state, &v, false, None, Some(v.t().msg("Enter a valid email address."))).await;
    };
    if crate::auth::register::email_domain_refused(&state, &email)
        || bf_db::moderation::bans::email_banned(pool, &email).await.db()?
    {
        return settings_page(&state, &v, false, None, Some(v.t().msg("That email address cannot be used here.")))
            .await;
    }
    let token = auth::random_token();
    security::create_email_token(pool, &auth::token_digest(&token), user.id, "email_change", Some(&email), 24 * 60)
        .await
        .db()?;
    let s = state.settings();
    let site = s.str("site_name");
    let t = v.t();
    let link = format!("{}/confirm-email/{token}", state.config.base_url);
    let body = t.msgf(
        "Hi {name},\n\nConfirm {email} as the new address for your account:\n\n{link}\n\nIf you did not ask for this, ignore this email.\n\n— {site}\n",
        &[("name", &acct.username), ("email", &email), ("link", &link), ("site", site)],
    );
    let subject = t.msgf("Confirm your new email for {site}", &[("site", site)]);
    outbox::queue_mail(pool, &email, &subject, &body).await.db()?;
    settings_page(&state, &v, false, Some(t.msgf("Check {email} for a confirmation link.", &[("email", &email)])), None)
        .await
}

/// Tells a member about a change to their account's security, with what to do if it was not
/// them. A mail failure is logged: the change itself has already happened.
pub(super) async fn security_notice(state: &AppState, t: bf_core::i18n::T, to: &str, username: &str, what: &str) {
    let Some(pool) = &state.pool else { return };
    let site = state.settings().str("site_name").to_owned();
    let forgot = format!("{}/forgot", state.config.base_url);
    let vars = [("site", site.as_str()), ("username", username), ("what", what), ("link", forgot.as_str())];
    let (subject, body) = crate::admin::mail_templates::render(&t, "security", &vars);
    if let Err(e) = outbox::queue_mail(pool, to, &subject, &body).await {
        tracing::warn!(error = %e, "security notice not queued");
    }
}

/// The GET only shows a button: mail scanners follow links, and must not use up the token.
pub(super) async fn confirm_email_page(
    State(state): State<AppState>,
    v: Viewer,
    Path(token): Path<String>,
) -> Result<Response, AppError> {
    let owner = if valid_token_shape(&token) {
        security::email_token_owner(state.db()?, &auth::token_digest(&token), "email_change").await.db()?
    } else {
        None
    };
    // As with `/verify`: the new address is confirmed in a session of the account that asked.
    if owner.is_some_and(|o| !v.user.as_ref().is_some_and(|u| u.id == o)) {
        return Ok(sign_in_to_confirm(&state, &v));
    }
    let ok = owner.is_some();
    let f = if ok {
        Form1 {
            heading: en("Confirm your new email"),
            action: format!("/confirm-email/{token}"),
            fields: vec![],
            button: en("Confirm new email address"),
            message: None,
            error: None,
            links: vec![],
        }
    } else {
        Form1 {
            heading: en("Link expired"),
            action: String::new(),
            fields: vec![],
            button: "",
            message: Some(v.t().msg("This confirmation link was already used or has expired.")),
            error: None,
            links: vec![(en("Account settings"), "/settings")],
        }
    };
    let status = if ok { StatusCode::OK } else { StatusCode::GONE };
    Ok(form_page(&state, &v, f, status))
}

pub(super) async fn confirm_email(
    State(state): State<AppState>,
    v: Viewer,
    Path(token): Path<String>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&form.csrf)?;
    let pool = state.db()?;
    let digest = auth::token_digest(&token);
    let owner = if valid_token_shape(&token) {
        security::email_token_owner(pool, &digest, "email_change").await.db()?
    } else {
        None
    };
    // Checked before the link is used, so a click from anyone else does not burn it.
    if owner.is_some_and(|o| !v.user.as_ref().is_some_and(|u| u.id == o)) {
        return Ok(sign_in_to_confirm(&state, &v));
    }
    let used = if owner.is_some() {
        security::consume_email_token(pool, &digest, "email_change").await.db()?
    } else {
        None
    };
    let (heading, message, status) = match used {
        Some((user_id, Some(email))) => {
            let old = security::account(pool, user_id.into()).await.db()?;
            if users::change_email(pool, user_id.into(), &email).await.db()? {
                crate::platform::webhooks::fire(
                    &state,
                    "user.email_changed",
                    serde_json::json!({ "user_id": user_id }),
                );
                // The old address hears about it, so a taken-over account does not go unnoticed.
                if let Some(old) = old.filter(|o| !o.email.eq_ignore_ascii_case(&email)) {
                    let line = v.t().msgf(
                        "The email address of your account was changed to {email}",
                        &[("email", &mask_email(&email))],
                    );
                    security_notice(&state, v.t(), &old.email, &old.username, &line).await;
                }
                (
                    en("Email changed"),
                    v.t().msgf("Your account now uses {email}.", &[("email", &mask_email(&email))]),
                    StatusCode::OK,
                )
            } else {
                (en("Email in use"), v.t().msg("Another account now uses that address."), StatusCode::CONFLICT)
            }
        }
        _ => {
            (en("Link expired"), v.t().msg("This confirmation link was already used or has expired."), StatusCode::GONE)
        }
    };
    let f = Form1 {
        heading,
        action: String::new(),
        fields: vec![],
        button: "",
        message: Some(message),
        error: None,
        links: vec![(en("Account settings"), "/settings")],
    };
    Ok(form_page(&state, &v, f, status))
}

#[derive(Deserialize)]
pub(super) struct UsernameForm {
    pub(super) csrf: String,
    pub(super) username: String,
}

/// Username change, limited to once per `username_change_days`.
pub(super) async fn change_username(
    State(state): State<AppState>,
    v: Viewer,
    Form(f): Form<UsernameForm>,
) -> Result<Response, AppError> {
    v.check_session_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let pool = state.db()?;
    let days = state.settings().int("username_change_days");
    let normalized = auth::normalize_username(&f.username);
    let name = normalized.as_str();
    let err = |m: String| async { settings_page(&state, &v, false, None, Some(m)).await };
    if days == 0 {
        return err(v.t().msg("Usernames cannot be changed on this site.")).await;
    }
    if !is_staff(&v) && i64::from(user.trust_level) < state.settings().int("username_change_min_trust_level") {
        return err(v.t().msgf(
            "Changing your username needs trust level {need}. You are at level {have}. Follow your progress at /trust.",
            &[
                ("need", &state.settings().int("username_change_min_trust_level").to_string()),
                ("have", &user.trust_level.to_string()),
            ],
        ))
        .await;
    }
    if let Some(last) = users::last_rename(pool, user.id).await.db()?
        && chrono::Utc::now() - last < chrono::Duration::days(days)
    {
        return err(v.t().msgf("You can change your username once every {days} days.", &[("days", &days.to_string())]))
            .await;
    }
    let unicode = state.settings().bool("unicode_usernames");
    if let Err(e) = auth::validate_username_for(name, unicode) {
        return err(v.t().msg(e)).await;
    }
    if !is_staff(&v)
        && let Some(e) = crate::auth::username_problem(&state, name)
    {
        return err(e).await;
    }
    if users::skeleton_taken(pool, &auth::username_skeleton(name), Some(user.id.get())).await.db()? {
        return err(v.t().msg("That username looks too much like another member's.")).await;
    }
    if name.eq_ignore_ascii_case(&user.username) && name == user.username {
        return err(v.t().msg("That is already your username.")).await;
    }
    if !is_staff(&v) && !crate::moderation::gate::username_ok(&state.forum.get(pool).await?.words, name) {
        return err(v.t().msg("That username is not allowed.")).await;
    }
    match users::rename(pool, user.id, name).await {
        Ok(true) => {}
        Ok(false) => return err(v.t().msg("That username is taken.")).await,
        Err(e) if users::is_skeleton_taken(&e) => {
            return err(v.t().msg("That username looks too much like another member's.")).await;
        }
        Err(e) => return Err(crate::http::render::db_err(e)),
    }
    state.microcache.clear();
    Ok(Redirect::to("/settings").into_response())
}
