//! Registration and its checks.

use crate::auth::login::AuthForm;
use crate::auth::login::auth_page;
use crate::auth::login::start_session;
use crate::http::render::{DbResult, db_err};
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Form, Router};
use bf_core::{AppError, auth};
use bf_db::accounts::{invites, users};
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;

pub fn router() -> Router<AppState> {
    Router::new().route("/register", get(register_form).post(register))
}

#[derive(Deserialize)]
struct RegisterQuery {
    #[serde(default)]
    invite: String,
}

/// (provider, site key) when a registration challenge is fully configured.
pub(crate) fn captcha_config(state: &AppState) -> Option<(String, String)> {
    let s = state.settings();
    let provider = s.str("captcha_provider");
    let key = s.str("captcha_site_key");
    // ALTCHA runs on this site, so it has no site key.
    let keyed = matches!(provider, "turnstile" | "hcaptcha" | "recaptcha") && !key.is_empty();
    let ready = (keyed || provider == "altcha") && std::env::var("BF_CAPTCHA_SECRET").is_ok();
    ready.then(|| (provider.to_owned(), key.to_owned()))
}

static TS_KEY: std::sync::LazyLock<Vec<u8>> = std::sync::LazyLock::new(|| {
    use sha2::{Digest, Sha256};
    for var in ["BF_SECRET_KEY", "BF_CAPTCHA_SECRET"] {
        if let Ok(v) = std::env::var(var)
            && !v.trim().is_empty()
        {
            return Sha256::digest(format!("bf-register-ts:{v}")).to_vec();
        }
    }
    Sha256::digest(format!("bf-register-ts:{}", auth::random_token())).to_vec()
});

fn ts_mac(ts: i64) -> Option<hmac::Hmac<sha2::Sha256>> {
    use hmac::{KeyInit, Mac};
    let mut m = hmac::Hmac::<sha2::Sha256>::new_from_slice(&TS_KEY).ok()?;
    m.update(format!("bf-register-ts:{ts}").as_bytes());
    Some(m)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The hidden `ts` field of the registration form: the time it was served, signed, so a client
/// cannot backdate it to get past `min_register_seconds`.
pub(crate) fn signed_ts(now: i64) -> String {
    use hmac::Mac;
    let sig = ts_mac(now).map(|m| hex(&m.finalize().into_bytes())).unwrap_or_default();
    format!("{now}.{sig}")
}

/// The time in a `ts` field, when its signature is ours.
fn verified_ts(value: &str) -> Option<i64> {
    use hmac::Mac;
    let (ts, sig) = value.split_once('.')?;
    let ts: i64 = ts.parse().ok()?;
    let want = hex(&ts_mac(ts)?.finalize().into_bytes());
    auth::constant_eq(want.as_bytes(), sig.as_bytes()).then_some(ts)
}

/// Asks the challenge provider whether this registration passed. Fails closed when the
/// provider cannot be reached: an outage should not open registration to bots.
async fn captcha_passed(pool: &sqlx::PgPool, provider: &str, f: &AuthForm, ip: Option<std::net::IpAddr>) -> bool {
    if provider == "altcha" {
        let (Some(payload), Ok(secret)) = (f.extra.get("altcha"), std::env::var("BF_CAPTCHA_SECRET")) else {
            return false;
        };
        let Some((signature, expires)) = crate::auth::altcha::verify(&secret, payload, Utc::now().timestamp()) else {
            return false;
        };
        return bf_db::accounts::challenges::spend_captcha(pool, &signature, expires).await.unwrap_or_else(|e| {
            tracing::warn!(error = %e, "captcha check failed");
            false
        });
    }
    let (field, default_url) = match provider {
        "turnstile" => ("cf-turnstile-response", "https://challenges.cloudflare.com/turnstile/v0/siteverify"),
        "recaptcha" => ("g-recaptcha-response", "https://www.google.com/recaptcha/api/siteverify"),
        _ => ("h-captcha-response", "https://hcaptcha.com/siteverify"),
    };
    let Some(response) = f.extra.get(field).filter(|r| !r.is_empty()) else { return false };
    let Ok(secret) = std::env::var("BF_CAPTCHA_SECRET") else { return false };
    // The override exists for tests and only allows loopback.
    let url = std::env::var("BF_CAPTCHA_VERIFY_URL")
        .ok()
        .filter(|u| u.starts_with("http://127.0.0.1:"))
        .unwrap_or_else(|| default_url.to_owned());
    let ip = ip.map(|i| i.to_string()).unwrap_or_default();
    let form = [("secret", secret.as_str()), ("response", response.as_str()), ("remoteip", ip.as_str())];
    let Ok(client) = reqwest::Client::builder().timeout(std::time::Duration::from_secs(5)).build() else {
        return false;
    };
    match client.post(url).form(&form).send().await {
        Ok(r) => r.json::<serde_json::Value>().await.is_ok_and(|v| v["success"] == true),
        Err(e) => {
            tracing::warn!(error = %e, "captcha check failed");
            false
        }
    }
}

async fn register_form(State(state): State<AppState>, v: Viewer, Query(q): Query<RegisterQuery>) -> Response {
    if !state.settings().bool("allow_registration") {
        return AppError::Forbidden.into_response();
    }
    auth_page(&state, &v, true, None, String::new(), String::new(), q.invite).await
}

async fn register(
    State(state): State<AppState>,
    v: Viewer,
    headers: axum::http::HeaderMap,
    Form(f): Form<AuthForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    if !state.settings().bool("allow_registration") {
        return Err(AppError::Forbidden);
    }
    crate::http::ratelimit::REGISTER_BY_IP.enforce(&format!("register:{}", crate::http::ratelimit::ip_key(v.ip)))?;
    let pool = state.db()?;
    // NFKC first, so the name that is checked is the name that is stored.
    let username = auth::normalize_username(&f.username);
    // The canonical form is what is checked and stored; an invalid one is refused in `register_problem`.
    let email = auth::normalize_email(&f.email).unwrap_or_else(|| f.email.trim().to_owned());
    let mut problem = register_problem(&state, pool, &username, &email, &f, v.ip, v.t()).await?;
    if problem.is_none() {
        problem = crate::platform::plugins::registration_refused(&state, &username, &email).await;
    }
    if problem.is_none() {
        let answers: Vec<(i64, String)> =
            f.extra.iter().filter_map(|(k, v)| Some((k.strip_prefix("field_")?.parse().ok()?, v.clone()))).collect();
        problem = crate::platform::plugins::signup_refused(&state, &username, &email, &answers).await;
    }
    if let Some(msg) = problem {
        return Ok(auth_page(&state, &v, true, Some(msg), username, email, f.invite.clone()).await);
    }
    let password = f.password.clone();
    let hash = tokio::task::spawn_blocking(move || auth::hash_password(&password)).await.map_err(AppError::internal)?;
    // Another spelling of an address that already has an account (`a+b@`, Gmail dots) is treated
    // like that address: its owner is told by email and the form says nothing.
    let (local, domain) = auth::email_alias_key(&email);
    if let Some(owner_email) = users::email_alias_owner(pool, &local, &domain).await.db()? {
        return already_registered(&state, &v, pool, &owner_email).await;
    }
    let id = match users::create_user(pool, &username, &email, &hash).await {
        Ok(id) => id,
        Err(e) if users::is_skeleton_taken(&e) => {
            let msg = v.t().msg("That username looks too much like an existing member's.");
            return Ok(auth_page(&state, &v, true, Some(msg), username, email, f.invite.clone()).await);
        }
        // An address that is taken is not said on the form, or the form would tell anyone which
        // addresses have accounts; its owner is told by email. Usernames are public, so a taken
        // name is reported as such.
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() && e.constraint() == Some("users_email_lower_key") => {
            return already_registered(&state, &v, pool, &email).await;
        }
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            let msg = v.t().msg("That username is taken.");
            return Ok(auth_page(&state, &v, true, Some(msg), username, email, f.invite.clone()).await);
        }
        Err(e) => return Err(db_err(e)),
    };
    // The same definition of "first account" as the check before (no member existed): now it is the only one.
    let first_account = users::user_count(pool).await.db()? <= 1;
    if state.settings().bool("invite_only")
        && !first_account
        && !invites::use_invite(pool, &auth::token_digest(f.invite.trim()), id.get()).await.db()?
    {
        // Another registration took the code between the check and now.
        sqlx::query("DELETE FROM users WHERE id = $1").bind(id).execute(pool).await.db()?;
        return Ok(auth_page(
            &state,
            &v,
            true,
            Some(v.t().msg("That invite code is no longer valid.")),
            username,
            email,
            String::new(),
        )
        .await);
    }
    crate::platform::webhooks::emit(&state, "user.registered", json!({ "user_id": id, "username": username })).await;
    crate::community::referrals::credit(&state, &headers, id.get(), &username).await;
    if crate::platform::newsletter::enabled(&state) && f.extra.get("newsletter").is_some_and(|v| v == "1") {
        bf_db::platform::newsletter::set(pool, id.get(), true).await.db()?;
    }
    if let Ok(Some(owner)) = bf_db::accounts::invites::invite_owner(pool, id).await {
        let data = json!({ "by": username, "url": bf_core::slug::profile_path(id.get(), &username) });
        crate::community::notifications::send(&state, id.get(), &[owner.get()], "invite_accepted", &data).await;
    }
    // The first account (the admin) is never held for approval.
    if state.settings().bool("require_account_approval") && !first_account {
        users::set_approved(pool, id, false).await.db()?;
        crate::community::notifications::tell_staff(&state, id.get()).await;
    }
    let answers: Vec<(i64, Option<String>)> = bf_db::accounts::fields::fields(pool)
        .await
        .db()?
        .iter()
        .filter(|fd| fd.show_on_register)
        .map(|fd| (fd.id, fd.check(f.extra.get(&format!("field_{}", fd.id)).map_or("", String::as_str)).ok().flatten()))
        .collect();
    bf_db::accounts::fields::save_values(pool, id, &answers).await.db()?;
    let docs: Vec<i64> = bf_db::community::legal::current_required(pool).await.db()?.iter().map(|d| d.id).collect();
    bf_db::community::legal::accept(pool, id.get(), &docs).await.db()?;
    crate::auth::recovery::send_token_mail(&state, pool, bf_core::mail::Kind::Verify, id, &username, &email).await?;
    let mut res = start_session(pool, id, v.ip).await?;
    // New members land on the getting-started page, and find it again in their notifications.
    let welcome = json!({ "name": state.settings().str("site_name"), "url": "/welcome" });
    if let Err(e) = bf_db::community::notifications::notify(pool, &[id.get()], "welcome", &welcome).await {
        tracing::warn!(error = %e, "welcome notification failed");
    }
    res.headers_mut().insert(axum::http::header::LOCATION, axum::http::HeaderValue::from_static("/welcome"));
    crate::platform::webhooks::fire(&state, "user.login", json!({ "user_id": id, "method": "register" }));
    Ok(res)
}

/// Someone registered with an address that already has an account: its owner gets an email, and
/// the page says only that an email is on its way, whoever asked.
async fn already_registered(state: &AppState, v: &Viewer, pool: &PgPool, email: &str) -> Result<Response, AppError> {
    if let Some(a) = bf_db::accounts::security::account_by_email(pool, email).await.db()? {
        let site = state.settings().str("site_name").to_owned();
        let t = crate::http::i18n::user_t(state, a.id.get()).await;
        let body = t.msgf(
            "Hi {name},\n\nSomeone just tried to register on {site} with this email address, but it already belongs to your account.\n\nIf it was you, log in, or reset your password if you have forgotten it: {link}\n\nIf not, you can ignore this email.\n\n— {site}\n",
            &[("name", &a.username), ("site", &site), ("link", &format!("{}/forgot", state.config.base_url))],
        );
        bf_db::platform::mail::queue_mail(
            pool,
            &a.email,
            &t.msgf("You already have an account on {site}", &[("site", &site)]),
            &body,
        )
        .await
        .db()?;
    }
    let f = crate::account::settings::Form1 {
        heading: bf_core::i18n::en("Check your email"),
        action: String::new(),
        fields: vec![],
        button: "",
        message: Some(v.t().msg("If this address can be used, an email is on its way with what to do next.")),
        error: None,
        links: vec![(bf_core::i18n::en("Back to log in"), "/login")],
    };
    Ok(crate::account::settings::form_page(state, v, f, axum::http::StatusCode::OK))
}

/// Registration checks that produce a message on the form rather than an error page.
/// Whether site settings refuse this address: a listed domain or a temporary email service.
pub(crate) fn email_domain_refused(state: &AppState, email: &str) -> bool {
    let domain = email.rsplit_once('@').map(|(_, d)| d.to_ascii_lowercase()).unwrap_or_default();
    let settings = state.settings();
    settings
        .str("blocked_email_domains")
        .split(',')
        .map(|d| d.trim().to_ascii_lowercase())
        .any(|d| !d.is_empty() && d == domain)
        || {
            let allowed = settings.str("allowed_email_domains");
            let mut list =
                allowed.split(',').map(|d| d.trim().to_ascii_lowercase()).filter(|d| !d.is_empty()).peekable();
            list.peek().is_some() && !list.any(|d| d == domain)
        }
        || (settings.bool("block_disposable_emails") && bf_core::disposable::is_disposable(&domain))
}

/// Too many new accounts from one address: a sign-up wave from a farm. Addresses on the allow
/// list (an office, a school) and internal ones are exempt. Shared by every way of signing up.
pub(crate) async fn ip_cap_problem(
    state: &AppState,
    pool: &PgPool,
    ip: Option<std::net::IpAddr>,
    t: &bf_core::i18n::T,
) -> Result<Option<String>, AppError> {
    let cap = state.settings().int("max_new_accounts_per_ip");
    if cap > 0
        && let Some(addr) = ip
        && !crate::platform::outbound::forbidden_ip(addr)
        && !bf_db::moderation::bans::ip_allowed(pool, addr).await.unwrap_or(false)
    {
        let made: i64 = sqlx::query_scalar(
            "SELECT count(DISTINCT u.id) FROM users u JOIN sessions s ON s.user_id = u.id
             WHERE s.ip = $1::inet AND u.joined_at > now() - make_interval(hours => $2::int)",
        )
        .bind(addr.to_string())
        .bind(state.settings().int("new_account_ip_window_hours") as i32)
        .fetch_one(pool)
        .await
        .db()?;
        if made >= cap {
            return Ok(Some(t.msg("Too many accounts were created from your network lately. Please try again later.")));
        }
    }
    Ok(None)
}

async fn register_problem(
    state: &AppState,
    pool: &PgPool,
    username: &str,
    email: &str,
    f: &AuthForm,
    ip: Option<std::net::IpAddr>,
    t: bf_core::i18n::T,
) -> Result<Option<String>, AppError> {
    // Bots: the honeypot is invisible to people; a form submitted faster than a person
    // could type is refused. Both answers are deliberately vague.
    if !f.website_url.is_empty() {
        return Ok(Some(t.msg("Registration failed. Please try again.")));
    }
    if bf_db::accounts::users::name_reserved(pool, username).await.db()? {
        return Ok(Some(t.msg("That username is taken.")));
    }
    if !crate::moderation::gate::username_ok(&state.forum.get(pool).await?.words, username) {
        return Ok(Some(t.msg("That username is not allowed.")));
    }
    let first_account = users::user_count(pool).await.db()? == 0;
    if state.settings().bool("invite_only")
        && !first_account
        && !invites::invite_valid(pool, &auth::token_digest(f.invite.trim())).await.db()?
    {
        return Ok(Some(t.msg("That invite code is not valid.")));
    }
    if let Some((provider, _)) = captcha_config(state)
        && !captcha_passed(pool, &provider, f, ip).await
    {
        return Ok(Some(t.msg("Please complete the challenge and try again.")));
    }
    // A provider is chosen but cannot run (no secret, no site key): registration stays closed
    // and says so in the log, instead of opening to bots without anyone noticing.
    if captcha_config(state).is_none() && !state.settings().str("captcha_provider").is_empty() {
        tracing::error!(
            "captcha_provider is set but not usable (BF_CAPTCHA_SECRET or the site key is missing); registration refused"
        );
        return Ok(Some(t.msg("Sign-ups are not available right now. Please try again later.")));
    }
    if let Some(msg) = ip_cap_problem(state, pool, ip, &t).await? {
        return Ok(Some(msg));
    }
    let min = state.settings().int("min_register_seconds");
    if min > 0 {
        let now = Utc::now().timestamp();
        // Unsigned, tampered or unreadable counts as too quick.
        let age = verified_ts(&f.ts).map_or(0, |served| now - served);
        if age < min || age > 86_400 {
            return Ok(Some(t.msg("That was quick! Please wait a moment and submit again.")));
        }
    }
    let unicode = state.settings().bool("unicode_usernames");
    if let Err(e) = auth::validate_username_for(username, unicode) {
        return Ok(Some(e.into()));
    }
    if let Some(e) = crate::auth::username_problem(state, username) {
        return Ok(Some(e));
    }
    // Always, whatever `unicode_usernames` says: "Rik0DEV" must not sit next to "RikoDEV".
    if bf_db::accounts::users::skeleton_taken(pool, &auth::username_skeleton(username), None).await.db()? {
        return Ok(Some(t.msg("That username looks too much like an existing member's.")));
    }
    if auth::normalize_email(email).as_deref() != Some(email) {
        return Ok(Some(t.msg("Enter a valid email address.")));
    }
    if let Some(e) = crate::auth::password_problem(state, &f.password, username) {
        return Ok(Some(e));
    }
    if let Some(e) = crate::auth::breached::refuse(state, &f.password).await {
        return Ok(Some(t.msg(e)));
    }
    if email_domain_refused(state, email) || bf_db::moderation::bans::email_banned(pool, email).await.db()? {
        return Ok(Some(t.msg("That email address cannot be used here.")));
    }
    let min_age = state.settings().int("minimum_age");
    if min_age > 0 {
        let born = chrono::NaiveDate::parse_from_str(f.birth_date.trim(), "%Y-%m-%d").ok();
        let today = Utc::now().date_naive();
        let old_enough = born.and_then(|b| today.years_since(b)).is_some_and(|y| i64::from(y) >= min_age);
        if !old_enough {
            return Ok(Some(
                t.msgf("You must be at least {min_age} years old to register.", &[("min_age", &min_age.to_string())]),
            ));
        }
    }
    for field in bf_db::accounts::fields::fields(pool).await.db()?.iter().filter(|f| f.show_on_register) {
        if let Err(e) = field.check_in(&t, f.extra.get(&format!("field_{}", field.id)).map_or("", String::as_str)) {
            return Ok(Some(e));
        }
    }
    if !bf_db::community::legal::current_required(pool).await.db()?.is_empty() && f.accept != "1" {
        return Ok(Some(t.msg("Please accept the terms and privacy policy.")));
    }
    Ok(None)
}
