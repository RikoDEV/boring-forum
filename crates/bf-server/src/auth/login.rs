//! Signing in and out, sessions and impersonation.

use crate::auth::register::captcha_config;
use crate::http::render::DbResult;
use crate::http::render::page;
use crate::http::viewer::CsrfForm;
use crate::http::viewer::ORIGINAL_SESSION_COOKIE;
use crate::http::viewer::SESSION_COOKIE;
use crate::http::viewer::Viewer;
use crate::http::viewer::cookie;
use crate::http::viewer::csrf_for;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use bf_core::{AppError, auth};
use bf_db::accounts::{sessions, users};
use bf_db::community::notifications;
use bf_web::pages::{AuthPage, FieldInput, Layout, render};
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;
use std::collections::HashMap;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/login", get(login_form).post(login))
        .route("/logout", post(logout))
        .route("/api/v1/admin/users/{id}/impersonate", post(impersonate))
        .route("/impersonate/stop", post(stop_impersonating))
}

#[derive(Deserialize)]
pub(crate) struct AuthForm {
    pub(crate) csrf: String,
    #[serde(default)]
    login: String,
    #[serde(default)]
    pub(crate) username: String,
    #[serde(default)]
    pub(crate) email: String,
    pub(crate) password: String,
    #[serde(default)]
    pub(crate) birth_date: String,
    #[serde(default)]
    pub(crate) invite: String,
    #[serde(default)]
    pub(crate) accept: String,
    /// Honeypot: hidden from people, filled in by naive bots.
    #[serde(default)]
    pub(crate) website_url: String,
    /// Unix time the form was served, for the time-to-submit check.
    #[serde(default)]
    pub(crate) ts: String,
    /// Registration questions arrive as `field_{id}`.
    #[serde(flatten)]
    pub(crate) extra: HashMap<String, String>,
}

pub(crate) async fn auth_page(
    state: &AppState,
    v: &Viewer,
    register: bool,
    error: Option<String>,
    username: String,
    email: String,
    invite: String,
) -> Response {
    let title = if register { v.t().msg("Register") } else { v.t().msg("Log in") };
    let mut l = Layout::new(&state.settings(), Some(&title));
    l.t = v.t();
    // `Layout::new` built the menu in English; build it again in the viewer's language.
    l.set_nav(&state.settings(), &bf_core::ids::raw(&v.actor.groups), v.actor.is_admin);
    let (token, set) = csrf_for(v, true);
    l.csrf = token;
    l.noindex = true;
    let status = if error.is_some() { StatusCode::UNPROCESSABLE_ENTITY } else { StatusCode::OK };
    let legal = match (&state.pool, register) {
        (Some(pool), true) => bf_db::community::legal::current_required(pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|d| {
                let t = v.t().msg(crate::community::legal::legal_title(&d.kind).unwrap_or(""));
                (d.kind, t)
            })
            .collect(),
        _ => vec![],
    };
    let min_age = state.settings().int("minimum_age");
    let questions = match (&state.pool, register) {
        (Some(pool), true) => bf_db::accounts::fields::fields(pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|f| f.show_on_register)
            .map(|f| FieldInput {
                hint: f.hint(&v.t()),
                shorthand: f.url_shorthand,
                id: f.id,
                label: f.label,
                kind: f.kind,
                options: f.options,
                required: f.required,
                value: String::new(),
            })
            .collect(),
        _ => vec![],
    };
    let body = (
        status,
        render(&AuthPage {
            layout: l,
            register,
            legal,
            min_age,
            magic: state.settings().bool("allow_magic_link"),
            providers: crate::auth::oauth::enabled(&state.settings()),
            questions,
            ts: crate::auth::register::signed_ts(Utc::now().timestamp()),
            invite_required: register && state.settings().bool("invite_only"),
            newsletter: register && crate::platform::newsletter::enabled(state),
            invite,
            captcha: captcha_config(state),
            error,
            username,
            email,
        }),
    )
        .into_response();
    page(body, set, false)
}

#[derive(Deserialize)]
struct LoginQuery {
    /// `?local=1` reaches the password form on a site that sends guests to an outside login.
    #[serde(default)]
    local: String,
}

async fn login_form(State(state): State<AppState>, v: Viewer, Query(q): Query<LoginQuery>) -> Response {
    if v.user.is_none()
        && q.local.is_empty()
        && let Some(key) = crate::auth::external::immediate_login(&state)
    {
        return axum::response::Redirect::to(&format!("/auth/{key}?auto=1")).into_response();
    }
    auth_page(&state, &v, false, None, String::new(), String::new(), String::new()).await
}

pub(crate) fn session_cookie(token: &str, max_age_secs: i64) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age={max_age_secs}"
    ))
    // The token is hex, so this cannot fail; if it ever did, no cookie is better than a crash.
    .unwrap_or_else(|_| HeaderValue::from_static(""))
}

pub(crate) async fn start_session(
    pool: &PgPool,
    user_id: bf_core::ids::UserId,
    ip: Option<std::net::IpAddr>,
) -> Result<Response, AppError> {
    if let Some(ip) = ip {
        if bf_db::moderation::bans::ip_banned(pool, ip).await.db()? {
            return Err(AppError::Forbidden);
        }
        // A sign-in from an address this account never used gets an email.
        let signed_in_before: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM audit_log WHERE actor_id = $1 AND action = 'login')")
                .bind(user_id)
                .fetch_one(pool)
                .await
                .db()?;
        if signed_in_before && !bf_db::accounts::sessions::known_ip(pool, user_id, ip).await.db()? {
            let (name, email, locale): (String, String, Option<String>) =
                sqlx::query_as("SELECT username, email, locale FROM users WHERE id = $1")
                    .bind(user_id)
                    .fetch_one(pool)
                    .await
                    .db()?;
            // The site's default language is not at hand here; a member without one reads English.
            let t = bf_core::i18n::T::new(locale.as_deref().unwrap_or("en"));
            let body = t.msgf(
                "Hi {name},\n\nYour account was just signed in to from a new address ({ip}).\n\nIf this was not you, change your password and sign out other sessions under Account settings.\n",
                &[("name", &name), ("ip", &ip.to_string())],
            );
            bf_db::platform::mail::queue_mail(pool, &email, &t.msg("New sign-in to your account"), &body).await.db()?;
            let data = json!({ "message": ip.to_string(), "url": "/settings" });
            notifications::notify(pool, &[user_id.get()], "new_login", &data).await.db()?;
        }
    }
    bf_db::accounts::sessions::record_login(pool, user_id, ip).await.db()?;
    let token = auth::random_token();
    sessions::create_session(pool, &auth::token_digest(&token), user_id, ip).await.db()?;
    let mut res = Redirect::to("/").into_response();
    res.headers_mut().append(header::SET_COOKIE, session_cookie(&token, 30 * 86400));
    Ok(res)
}

async fn login(State(state): State<AppState>, v: Viewer, Form(f): Form<AuthForm>) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let pool = state.db()?;
    if let Some(ip) = v.ip
        && bf_db::accounts::security::ip_login_blocked(pool, ip).await.db()?
    {
        return Err(AppError::RateLimited { retry_after_secs: 900 });
    }
    // What was typed is looked up and recorded in a bounded form only.
    let typed: String = f.login.trim().chars().take(bf_db::accounts::security::LOGIN_MAX_CHARS).collect();
    let user = users::user_by_login(pool, &typed).await.db()?;
    let password = f.password;
    // Hash even for unknown users so response time does not reveal which accounts exist.
    let hash = user.as_ref().and_then(|u| u.password_hash.clone());
    let legacy = hash.as_deref().is_some_and(auth::is_legacy_hash);
    let upgrade = legacy.then(|| password.clone());
    let ok = tokio::task::spawn_blocking(move || {
        if let Some(h) = hash {
            auth::verify_password(&password, &h)
        } else {
            auth::verify_password(
                &password,
                "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHQ$7JmXcVfg1VhQ0hFw0gRk2ZbKC4k7wKu0EdLPkx7u2r8",
            );
            false
        }
    })
    .await
    .map_err(AppError::internal)?;
    if let (true, Some(u), Some(plain)) = (ok, &user, upgrade) {
        let new = tokio::task::spawn_blocking(move || auth::hash_password(&plain)).await.map_err(AppError::internal)?;
        bf_db::accounts::security::upgrade_hash(pool, u.id.get(), &new).await.db()?;
    }
    // Asked the same way whether or not the account exists: an unknown name is limited by what
    // was typed, so the lock message does not tell which accounts there are.
    // The lock many addresses can put on an account does not shut out a device its owner had
    // remembered, nor an address the owner signed in from before (see `login_blocked_until_for`).
    let remembered = match &user {
        Some(u) => crate::auth::mfa::device_remembered(pool, u.id, &v).await?,
        None => false,
    };
    if let Some(until) =
        bf_db::accounts::security::login_blocked_until_for(pool, user.as_ref().map(|u| u.id), &typed, v.ip, remembered)
            .await
            .db()?
    {
        let msg = v.t().msgf(
            "Too many failed attempts. Try again after {time} UTC, or reset your password.",
            &[("time", &until.format("%H:%M").to_string())],
        );
        return Ok(auth_page(&state, &v, false, Some(msg), f.login, String::new(), String::new()).await);
    }
    if !ok {
        let locked = bf_db::accounts::security::record_failed_login(pool, &typed, user.as_ref().map(|u| u.id), v.ip)
            .await
            .db()?;
        if let (Some(minutes), Some(u)) = (locked, &user) {
            let site = state.settings().str("site_name").to_owned();
            let t = crate::http::i18n::user_t(&state, u.id.get()).await;
            let forgot = format!("{}/forgot", state.config.base_url);
            let body = t.msgf(
                "Hi {name},\n\nThere were 5 failed attempts to sign in to your account, so it is locked for {minutes} minutes.\n\nIf this was not you, consider changing your password: {link}\n\n— {site}\n",
                &[("name", &u.username), ("minutes", &minutes.to_string()), ("link", &forgot), ("site", &site)],
            );
            bf_db::platform::mail::queue_mail(
                pool,
                &u.email,
                &t.msgf("Sign-in attempts on {site}", &[("site", &site)]),
                &body,
            )
            .await
            .db()?;
        }
    }
    match user.filter(|_| ok) {
        Some(u) => {
            bf_db::accounts::security::clear_login_lockouts(pool, u.id).await.db()?;
            let res = crate::auth::mfa::finish_password_login(pool, u.id, &v).await?;
            Ok(crate::auth::mfa::logged_in(&state, u.id, "password", res))
        }
        None => Ok(auth_page(
            &state,
            &v,
            false,
            Some(v.t().msg("Wrong username or password.")),
            f.login,
            String::new(),
            String::new(),
        )
        .await),
    }
}

/// "View as user": a separate one-hour session, logged, with a banner. Admins cannot
/// be impersonated, and the admin's own session is kept in a cookie for coming back.
async fn impersonate(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let who = crate::admin::api::admin(&state, &v, &headers, true)?.ok_or(AppError::Forbidden)?;
    if v.api_scope.is_some() || v.impersonating {
        return Err(AppError::Forbidden);
    }
    let original = cookie(&headers, SESSION_COOKIE).ok_or(AppError::Forbidden)?.to_owned();
    let pool = state.db()?;
    let target = users::user_by_id(pool, id.into()).await.db()?.ok_or(AppError::NotFound)?;
    if target.is_admin {
        return Err(AppError::Validation("Administrators cannot be impersonated.".into()));
    }
    let token = auth::random_token();
    let digest = auth::token_digest(&token);
    sessions::create_impersonation_session(pool, &digest, id.into(), who, v.ip).await.db()?;
    bf_db::moderation::tools::mark_impersonation(pool, &digest, who, id).await.db()?;
    let mut res = StatusCode::NO_CONTENT.into_response();
    res.headers_mut().append(header::SET_COOKIE, session_cookie(&token, 3600));
    let keep = format!("{ORIGINAL_SESSION_COOKIE}={original}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=3600");
    res.headers_mut().append(header::SET_COOKIE, HeaderValue::from_str(&keep).map_err(AppError::internal)?);
    Ok(res)
}

async fn stop_impersonating(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    if !v.impersonating {
        return Err(AppError::Forbidden);
    }
    let original = cookie(&headers, ORIGINAL_SESSION_COOKIE).ok_or(AppError::Forbidden)?.to_owned();
    let pool = state.db()?;
    if let Some(d) = &v.session_digest {
        sessions::delete_session(pool, d).await.db()?;
        state.sessions.invalidate(d);
    }
    // The administrator's own session may have ended meanwhile: do not hand back a dead cookie.
    if !sessions::session_alive(pool, &auth::token_digest(&original)).await.db()? {
        let mut res = Redirect::to("/login").into_response();
        res.headers_mut().append(header::SET_COOKIE, session_cookie("", 0));
        let clear = format!("{ORIGINAL_SESSION_COOKIE}=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0");
        res.headers_mut().append(header::SET_COOKIE, HeaderValue::from_str(&clear).map_err(AppError::internal)?);
        return Ok(res);
    }
    let mut res = Redirect::to("/admin/users/").into_response();
    res.headers_mut().append(header::SET_COOKIE, session_cookie(&original, 30 * 86400));
    let clear = format!("{ORIGINAL_SESSION_COOKIE}=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0");
    res.headers_mut().append(header::SET_COOKIE, HeaderValue::from_str(&clear).map_err(AppError::internal)?);
    Ok(res)
}

async fn logout(State(state): State<AppState>, v: Viewer, Form(f): Form<CsrfForm>) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    if let (Some(d), Some(pool)) = (&v.session_digest, &state.pool) {
        sessions::delete_session(pool, d).await.db()?;
        // Not for up to five more seconds from the cache.
        state.sessions.invalidate(d);
    }
    if let Some(u) = &v.user {
        crate::platform::webhooks::fire(
            &state,
            "user.logout",
            serde_json::json!({ "user_id": u.id, "username": u.username }),
        );
    }
    let mut res = Redirect::to("/").into_response();
    res.headers_mut().append(header::SET_COOKIE, session_cookie("", 0));
    // Signing out of an impersonated session must not leave the administrator's way back behind.
    let clear = format!("{ORIGINAL_SESSION_COOKIE}=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0");
    res.headers_mut().append(header::SET_COOKIE, HeaderValue::from_str(&clear).map_err(AppError::internal)?);
    Ok(res)
}
