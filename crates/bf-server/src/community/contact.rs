//! The contact form.

use crate::http::i18n::trh;
use crate::http::render::DbResult;
use crate::http::ui::hidden_csrf;
use crate::http::ui::{Shell, begin, finish, panel, simple_page};
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use axum::{Form, Router};
use bf_core::AppError;
use bf_core::html::esc;
use bf_db::platform::mail;
use serde::Deserialize;

pub fn router() -> Router<AppState> {
    Router::new().route("/contact", get(contact_form).post(contact))
}

static CONTACT_BY_IP: std::sync::LazyLock<crate::http::ratelimit::RateLimiter> =
    std::sync::LazyLock::new(|| crate::http::ratelimit::RateLimiter::new(5, std::time::Duration::from_secs(3600)));

async fn contact_form(State(state): State<AppState>, v: Viewer) -> Result<Response, AppError> {
    if state.settings().str("contact_email").is_empty() {
        return Err(AppError::NotFound);
    }
    let ctx = begin(&state, &v);
    let email = v.user.as_ref().map_or(String::new(), |u| esc(&u.email));
    let html = format!(
        "<form method=\"post\" action=\"/contact\" class=\"stack\">{}<label class=\"bf-field\"><input id=\"c-email\" type=\"email\" name=\"email\" value=\"{email}\" placeholder=\" \" required><span class=\"bf-field-label\">{tr1}</span></label><label class=\"bf-field\"><textarea id=\"c-body\" name=\"body\" rows=\"6\" placeholder=\" \" required></textarea><span class=\"bf-field-label\">{tr2}</span></label><div hidden><label>{tr3} <input name=\"website\" tabindex=\"-1\" autocomplete=\"off\"></label></div><button class=\"btn primary\">{tr4}</button></form>",
        hidden_csrf(&ctx.token),
        tr1 = trh("Your email"),
        tr2 = trh("Message"),
        tr3 = trh("Leave empty"),
        tr4 = trh("Send")
    );
    let shell = Shell { title: v.t().msg("Contact us"), noindex: true, ..Shell::default() };
    Ok(finish(&state, &v, ctx, shell, panel(&html)))
}

#[derive(Deserialize)]
struct ContactForm {
    csrf: String,
    email: String,
    body: String,
    #[serde(default)]
    website: String,
}

/// The sender's address goes in the text, never in mail headers, so it cannot inject any.
async fn contact(State(state): State<AppState>, v: Viewer, Form(f): Form<ContactForm>) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let to = state.settings().str("contact_email").to_owned();
    if to.is_empty() {
        return Err(AppError::NotFound);
    }
    let email = f.email.trim();
    let good_email = email.contains('@') && email.len() <= 254 && !email.chars().any(|c| c.is_control() || c == ' ');
    if !good_email || f.body.trim().is_empty() || f.body.chars().count() > 5000 {
        return Err(AppError::Validation("Give your email and a message of up to 5000 characters.".into()));
    }
    // Per address first, so one sender cannot use up the site-wide cap for everyone.
    if let Some(ip) = v.ip
        && CONTACT_BY_IP.check(ip, std::time::Instant::now()).is_err()
    {
        return Err(AppError::Validation("You have sent several messages recently. Try again later.".into()));
    }
    // A filled honeypot gets the same answer, and nothing is sent.
    if f.website.is_empty() {
        let pool = state.db()?;
        // A site-wide cap: this form can mail staff without an account.
        if mail::contact_mails_last_hour(pool).await.db()? >= state.settings().int("contact_per_hour") {
            return Err(AppError::Validation("Too many messages were sent recently. Try again later.".into()));
        }
        let member = v.user.as_ref().map_or("guest", |u| u.username.as_str());
        let text = format!("From: {email} ({member})\n\n{}\n", f.body);
        // For the site's staff: in the site's language.
        let t = bf_core::i18n::T::new(bf_core::i18n::site_language(state.settings().str("default_locale")));
        let subject = t.msgf("Contact form message - {site}", &[("site", state.settings().str("site_name"))]);
        bf_db::platform::mail::queue_contact_mail(pool, &to, &subject, &text).await.db()?;
    }
    Ok(simple_page(
        &state,
        &v,
        &v.t().msg("Thank you"),
        &format!("<p>{tr1}</p>", tr1 = trh("Your message was sent.")),
        false,
    ))
}
