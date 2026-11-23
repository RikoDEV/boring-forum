//! Sign-in links by email.

use crate::account::settings::Form1;
use crate::account::settings::field;
use crate::account::settings::form_page;
use crate::auth::mfa::finish_password_login;
use crate::auth::recovery::EmailForm;
use crate::auth::recovery::expired_link;
use crate::auth::recovery::send_token_mail;
use crate::auth::recovery::valid_token_shape;
use crate::http::render::DbResult;
use crate::http::viewer::{CsrfForm, Viewer};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;
use axum::{Form, Router};
use bf_core::i18n::en;
use bf_core::{AppError, auth};
use bf_db::accounts::security;
use bf_db::platform::mail as outbox;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/login/magic", get(magic_form).post(magic_send))
        .route("/login/magic/{token}", get(magic_confirm).post(magic_use))
}

fn magic_on(state: &AppState) -> Result<(), AppError> {
    if state.settings().bool("allow_magic_link") { Ok(()) } else { Err(AppError::NotFound) }
}

fn magic_page(state: &AppState, v: &Viewer, message: Option<String>) -> Response {
    let f = Form1 {
        heading: en("Email me a log-in link"),
        action: "/login/magic".into(),
        fields: if message.is_some() { vec![] } else { vec![field("email", en("Email"), "email", "email")] },
        button: if message.is_some() { "" } else { en("Send link") },
        message,
        error: None,
        links: vec![(en("Back to log in"), "/login")],
    };
    form_page(state, v, f, StatusCode::OK)
}

async fn magic_form(State(state): State<AppState>, v: Viewer) -> Result<Response, AppError> {
    magic_on(&state)?;
    Ok(magic_page(&state, &v, None))
}

async fn magic_send(State(state): State<AppState>, v: Viewer, Form(f): Form<EmailForm>) -> Result<Response, AppError> {
    magic_on(&state)?;
    v.check_csrf(&f.csrf)?;
    let pool = state.db()?;
    // Throttled per address and per visitor; a refusal looks like a success, so the answer never
    // tells whether the address has an account.
    let by_ip = crate::http::ratelimit::MAIL_BY_IP.allow(&format!("magic:{}", crate::http::ratelimit::ip_key(v.ip)));
    let by_mail = crate::http::ratelimit::MAIL_BY_ACCOUNT.allow(&format!("magic:{}", f.email.trim().to_lowercase()));
    // Always sent: the newest links retire the oldest (see `create_email_token`).
    if by_ip
        && by_mail
        && let Some(a) = security::account_by_email(pool, f.email.trim()).await.db()?
    {
        // An address nobody has confirmed may be someone else's: whoever registered it knows the
        // account's password, and a sign-in link would put the address's owner into that account
        // beside them. They get a reset link instead, which ends every other way in.
        if !a.email_verified {
            send_token_mail(&state, pool, bf_core::mail::Kind::Reset, a.id, &a.username, &a.email).await?;
            return Ok(magic_page(
                &state,
                &v,
                Some(v.t().msg("If an account uses that address, a log-in link is on its way.")),
            ));
        }
        let token = auth::random_token();
        security::create_email_token(pool, &auth::token_digest(&token), a.id, "magic", None, 15).await.db()?;
        let s = state.settings();
        let site = s.str("site_name");
        // In the language of whoever owns the address, not of whoever asked for the link.
        let t = crate::http::i18n::user_t(&state, a.id.get()).await;
        let link = format!("{}/login/magic/{token}", state.config.base_url);
        let body = t.msgf(
            "Hi {name},\n\nLog in to {site} with this link within 15 minutes:\n\n{link}\n\nIf you did not ask for this, ignore this email.\n\n— {site}\n",
            &[("name", a.username.as_str()), ("site", site), ("link", link.as_str())],
        );
        let subject = t.msgf("Your log-in link - {site}", &[("site", site)]);
        outbox::queue_mail(pool, &a.email, &subject, &body).await.db()?;
    }
    Ok(magic_page(&state, &v, Some(v.t().msg("If an account uses that address, a log-in link is on its way."))))
}

/// The GET only shows a button: mail scanners follow links, and must not use up the token.
async fn magic_confirm(
    State(state): State<AppState>,
    v: Viewer,
    Path(token): Path<String>,
) -> Result<Response, AppError> {
    magic_on(&state)?;
    let ok = valid_token_shape(&token)
        && security::email_token_valid(state.db()?, &auth::token_digest(&token), "magic").await.db()?;
    if !ok {
        return Ok(expired_link(&state, &v));
    }
    let f = Form1 {
        heading: en("Log in"),
        action: format!("/login/magic/{token}"),
        fields: vec![],
        button: en("Log in"),
        message: None,
        error: None,
        links: vec![],
    };
    Ok(form_page(&state, &v, f, StatusCode::OK))
}

async fn magic_use(
    State(state): State<AppState>,
    v: Viewer,
    Path(token): Path<String>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    magic_on(&state)?;
    v.check_csrf(&f.csrf)?;
    let pool = state.db()?;
    let used = if valid_token_shape(&token) {
        security::consume_email_token(pool, &auth::token_digest(&token), "magic").await.db()?
    } else {
        None
    };
    let Some((user_id, _)) = used else { return Ok(expired_link(&state, &v)) };
    // The link proves the mailbox, not the second factor: two-factor still applies.
    security::mark_email_verified(pool, user_id.into()).await.db()?;
    security::clear_login_lockouts(pool, user_id.into()).await.db()?;
    let res = finish_password_login(pool, user_id.into(), &v).await?;
    Ok(crate::auth::mfa::logged_in(&state, user_id.into(), "magic_link", res))
}
