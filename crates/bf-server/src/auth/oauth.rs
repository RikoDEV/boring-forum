//! OAuth 2 / OIDC login: Google, GitHub, Discord and one generic OIDC provider.
//!
//! Configuration (each provider is enabled when its id and secret are set):
//! `BF_OAUTH_{GOOGLE,GITHUB,DISCORD,MICROSOFT}_ID` / `_SECRET`;
//! `BF_OIDC_NAME`, `BF_OIDC_AUTHORIZE_URL`, `BF_OIDC_TOKEN_URL`, `BF_OIDC_USERINFO_URL`,
//! `BF_OIDC_CLIENT_ID`, `BF_OIDC_CLIENT_SECRET`;
//! `BF_OAUTH_APPLE_ID` (Services ID), `_TEAM_ID`, `_KEY_ID`, `_KEY` (`.p8` contents).
//!
//! After the provider returns: a known identity signs in (two-factor still applies);
//! a member who started from "Connected logins" in Account settings links it; a
//! provider-verified email that matches an account links to it; otherwise a new account is created.

use crate::account::settings::{Form1, form_page};
use crate::http::render::DbResult;
use crate::http::viewer::{CsrfForm, Viewer};
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use bf_core::i18n::en;
use bf_core::oauth::Provider;
use bf_core::{AppError, oauth};
use bf_db::accounts::security;
use serde::Deserialize;
use serde_json::Value;
use std::sync::LazyLock;
use std::time::Duration;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/{provider}", get(start))
        .route("/auth/{provider}/link", post(link))
        .route("/auth/{provider}/callback", get(callback).post(form_post))
}

// `__Host-`: a sibling subdomain cannot plant one (with its own state and verifier, to have its
// own provider account answer in someone else's browser), as with the session cookie.
const COOKIE: &str = "__Host-bf_oauth";

struct Configured {
    provider: Provider,
    label: String,
    client_id: String,
    client_secret: String,
    apple: Option<AppleKey>,
}

struct AppleKey {
    team_id: String,
    key_id: String,
    pem: String,
    base: String,
}

impl Configured {
    fn secret(&self) -> Result<String, AppError> {
        match &self.apple {
            Some(a) => oauth::apple_client_secret(
                &a.team_id,
                &a.key_id,
                &self.client_id,
                &a.pem,
                &a.base,
                chrono::Utc::now().timestamp(),
            )
            .map_err(AppError::internal),
            None => Ok(self.client_secret.clone()),
        }
    }
}

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

static PROVIDERS: LazyLock<Vec<Configured>> = LazyLock::new(|| {
    let mut out = Vec::new();
    for (key, label) in [("google", "Google"), ("github", "GitHub"), ("discord", "Discord"), ("microsoft", "Microsoft")]
    {
        let up = key.to_uppercase();
        if let (Some(id), Some(secret)) = (env(&format!("BF_OAUTH_{up}_ID")), env(&format!("BF_OAUTH_{up}_SECRET"))) {
            let Some(provider) = Provider::builtin(key) else { continue };
            out.push(Configured { provider, label: label.into(), client_id: id, client_secret: secret, apple: None });
        }
    }
    if let (Some(a), Some(t), Some(u), Some(id), Some(secret)) = (
        env("BF_OIDC_AUTHORIZE_URL"),
        env("BF_OIDC_TOKEN_URL"),
        env("BF_OIDC_USERINFO_URL"),
        env("BF_OIDC_CLIENT_ID"),
        env("BF_OIDC_CLIENT_SECRET"),
    ) {
        match Provider::oidc(&a, &t, &u) {
            Ok(provider) => out.push(Configured {
                provider,
                label: env("BF_OIDC_NAME").unwrap_or_else(|| "Single sign-on".into()),
                client_id: id,
                client_secret: secret,
                apple: None,
            }),
            Err(e) => tracing::error!(error = e, "BF_OIDC_* ignored"),
        }
    }
    if let (Some(id), Some(team_id), Some(key_id), Some(pem)) = (
        env("BF_OAUTH_APPLE_ID"),
        env("BF_OAUTH_APPLE_TEAM_ID"),
        env("BF_OAUTH_APPLE_KEY_ID"),
        env("BF_OAUTH_APPLE_KEY"),
    ) {
        // The override exists for tests; `Provider::apple` allows http on loopback only.
        let base = env("BF_OAUTH_APPLE_BASE").unwrap_or_else(|| "https://appleid.apple.com".into());
        match Provider::apple(&base) {
            Ok(provider) => out.push(Configured {
                provider,
                label: "Apple".into(),
                client_id: id,
                client_secret: String::new(),
                apple: Some(AppleKey { team_id, key_id, pem, base }),
            }),
            Err(e) => tracing::error!(error = e, "BF_OAUTH_APPLE_BASE ignored"),
        }
    }
    out
});

#[expect(
    clippy::expect_used,
    reason = "startup: the builder options are constants; it fails only if the TLS backend cannot start"
)]
static HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent("boring-forum")
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("http client")
});

/// (key, label) of enabled providers, for the login page.
pub(crate) fn enabled() -> Vec<(&'static str, String)> {
    let mut all: Vec<_> = PROVIDERS.iter().map(|c| (c.provider.key, c.label.clone())).collect();
    if crate::auth::identity::sso_enabled() {
        all.push(("sso", std::env::var("BF_SSO_NAME").unwrap_or_else(|_| "Single sign-on".into())));
    }
    all
}

/// (key, label) of the providers a member can connect from Account settings. The login site of
/// `/auth/sso` signs in, but is not connected this way.
pub(crate) fn linkable() -> Vec<(&'static str, String)> {
    PROVIDERS.iter().map(|c| (c.provider.key, c.label.clone())).collect()
}

fn find(key: &str) -> Result<&'static Configured, AppError> {
    PROVIDERS.iter().find(|c| c.provider.key == key).ok_or(AppError::NotFound)
}

fn redirect_uri(state: &AppState, key: &str) -> String {
    format!("{}/auth/{key}/callback", state.config.base_url)
}

async fn start(State(state): State<AppState>, Path(key): Path<String>) -> Result<Response, AppError> {
    let (url, cookie) = begin(&state, find(&key)?, None)?;
    let mut res = Redirect::to(&url).into_response();
    res.headers_mut().append(header::SET_COOKIE, cookie);
    Ok(res)
}

/// "Connect" in Account settings. Only a flow begun here may link the provider's answer to the
/// member: the callback is a plain GET that any page can send a signed-in browser to, so being
/// signed in when it arrives says nothing about wanting a new way into the account.
async fn link(
    State(state): State<AppState>,
    v: Viewer,
    Path(key): Path<String>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    v.check_session_csrf(&f.csrf)?;
    let me = v.require_user()?.id.get();
    let c = find(&key)?;
    if let Some(confirm) = v.sudo_redirect("/settings") {
        return Ok(confirm);
    }
    let (url, cookie) = begin(&state, c, Some(me))?;
    // A page that sends the browser on, not a redirect: a browser holds the redirects answering
    // a form to the page's `form-action` policy, which allows this site only, and a provider's
    // own sign-in may pass through several of its hosts.
    let f = Form1 {
        heading: en("Connecting…"),
        action: String::new(),
        fields: vec![],
        button: "",
        message: Some(v.t().msgf("You are being sent to {n} to sign in there.", &[("n", c.label.as_str())])),
        error: None,
        links: vec![(en("Account settings"), "/settings")],
    };
    let mut res = form_page(&state, &v, f, StatusCode::OK);
    res.headers_mut().append(header::SET_COOKIE, cookie);
    res.headers_mut().insert("refresh", HeaderValue::from_str(&format!("0; url={url}")).map_err(AppError::internal)?);
    Ok(res)
}

/// Where to send the browser, and the cookie to send with it: the state, the PKCE verifier and,
/// for a flow that connects a provider, the member who asked for it.
fn begin(state: &AppState, c: &Configured, link_for: Option<i64>) -> Result<(String, HeaderValue), AppError> {
    let s = oauth::start(&c.provider, &c.client_id, &redirect_uri(state, c.provider.key));
    let link_for = link_for.map(|id| id.to_string()).unwrap_or_default();
    // SameSite=Lax is required: the provider sends the browser back with a top-level GET.
    let cookie = format!(
        "{COOKIE}={}.{}.{link_for}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=600",
        s.state, s.verifier
    );
    Ok((s.url, HeaderValue::from_str(&cookie).map_err(AppError::internal)?))
}

#[derive(Deserialize)]
struct Callback {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    /// Apple sends the name once, at first sign-in; see `form_post`.
    name: Option<String>,
}

#[derive(Deserialize)]
struct FormPost {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    /// JSON: `{"name":{"firstName":...,"lastName":...},"email":...}`.
    user: Option<String>,
}

/// Apple returns by cross-site POST, on which SameSite=Lax cookies are not sent. This hop
/// turns it into a top-level GET, so the normal callback sees the login and session cookies.
async fn form_post(Path(key): Path<String>, Form(f): Form<FormPost>) -> Result<Response, AppError> {
    let c = find(&key)?;
    if !c.provider.form_post {
        return Err(AppError::NotFound);
    }
    let name = f.user.as_deref().and_then(|u| serde_json::from_str::<Value>(u).ok()).and_then(|u| {
        let first = u["name"]["firstName"].as_str().unwrap_or_default().to_owned();
        (!first.is_empty()).then_some(first)
    });
    let mut q = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in [("code", f.code), ("state", f.state), ("error", f.error), ("name", name)] {
        if let Some(v) = v {
            q.append_pair(k, &v);
        }
    }
    Ok(Redirect::to(&format!("/auth/{}/callback?{}", c.provider.key, q.finish())).into_response())
}

/// The profile in Apple's `id_token`. `email_verified` may be a boolean or a string.
fn apple_user(c: &Configured, token: &Value, name: Option<String>) -> Result<ExternalUser, AppError> {
    let a = c.apple.as_ref().ok_or(AppError::NotFound)?;
    let id_token = token["id_token"].as_str().ok_or_else(|| AppError::BadRequest("No id_token from Apple".into()))?;
    let claims = oauth::id_token_claims(id_token, &a.base, &c.client_id, chrono::Utc::now().timestamp())
        .map_err(AppError::BadRequest)?;
    let subject = str_field(&claims, "sub").ok_or_else(|| AppError::BadRequest("No user id from Apple".into()))?;
    let email = str_field(&claims, "email");
    let email_verified = claims["email_verified"] == true || claims["email_verified"] == "true";
    let fallback = email.as_deref().and_then(|e| e.split('@').next()).unwrap_or("member").to_owned();
    Ok(ExternalUser { subject, email, email_verified, name: name.unwrap_or(fallback) })
}

pub(crate) struct ExternalUser {
    pub(crate) subject: String,
    pub(crate) email: Option<String>,
    pub(crate) email_verified: bool,
    pub(crate) name: String,
}

fn str_field(v: &Value, k: &str) -> Option<String> {
    match v.get(k)? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

async fn fetch_user(c: &Configured, access_token: &str) -> Result<ExternalUser, AppError> {
    let get = |url: String| HTTP.get(url).bearer_auth(access_token).header(header::ACCEPT, "application/json").send();
    let info: Value = get(c.provider.userinfo_url.clone())
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| AppError::invalid("The provider did not return your profile: {e}", &[("e", &e)]))?
        .json()
        .await
        .map_err(AppError::internal)?;
    let subject = str_field(&info, "sub")
        .or_else(|| str_field(&info, "id"))
        .ok_or_else(|| AppError::BadRequest("No user id from provider".into()))?;
    let mut email = str_field(&info, "email");
    let mut email_verified =
        info.get("email_verified").or_else(|| info.get("verified")).and_then(Value::as_bool).unwrap_or(false);
    if c.provider.key == "github" {
        // GitHub's profile omits private addresses; ask for the verified primary one.
        if let Ok(r) = get("https://api.github.com/user/emails".into()).await
            && let Ok(list) = r.json::<Vec<Value>>().await
            && let Some(e) =
                list.iter().find(|e| e["primary"].as_bool() == Some(true) && e["verified"].as_bool() == Some(true))
        {
            email = e["email"].as_str().map(str::to_owned);
            email_verified = true;
        }
    }
    let name = ["preferred_username", "login", "username", "nickname", "given_name", "name"]
        .iter()
        .find_map(|k| str_field(&info, k))
        .unwrap_or_else(|| "member".into());
    Ok(ExternalUser { subject, email, email_verified, name })
}

/// A valid username from a provider's display name.
pub(crate) fn username_base(name: &str) -> String {
    let mut s: String =
        name.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')).take(16).collect();
    s = s.trim_matches(|c: char| !c.is_ascii_alphanumeric()).to_owned();
    if s.len() < 3 || bf_core::auth::validate_username(&s).is_err() {
        s = format!("member{}", &bf_core::auth::random_token()[..6]);
    }
    s
}

async fn callback(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(key): Path<String>,
    Query(q): Query<Callback>,
) -> Result<Response, AppError> {
    let c = find(&key)?;
    if let Some(e) = q.error {
        return Err(AppError::invalid("The provider refused the login: {e}", &[("e", &e)]));
    }
    let cookie = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|h| h.to_str().ok())
        .flat_map(|h| h.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v.to_owned())
        .ok_or_else(|| AppError::BadRequest("Your login attempt expired. Please try again.".into()))?;
    let mut parts = cookie.splitn(3, '.');
    let (Some(issued), Some(verifier)) = (parts.next(), parts.next()) else {
        return Err(AppError::BadRequest("bad login state".into()));
    };
    // Set by `link` only: the member who asked to connect this provider.
    let link_for = parts.next().and_then(|id| id.parse::<i64>().ok());
    if !oauth::state_matches(issued, q.state.as_deref().unwrap_or_default()) {
        return Err(AppError::BadRequest("Your login attempt expired. Please try again.".into()));
    }
    let code = q.code.ok_or_else(|| AppError::BadRequest("missing code".into()))?;
    let body =
        oauth::token_request_body(&code, verifier, &c.client_id, &c.secret()?, &redirect_uri(&state, c.provider.key));
    let token: Value = HTTP
        .post(&c.provider.token_url)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::ACCEPT, "application/json")
        .body(body)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| AppError::invalid("The provider did not accept the login: {e}", &[("e", &e)]))?
        .json()
        .await
        .map_err(AppError::internal)?;
    let ext = if c.apple.is_some() {
        apple_user(c, &token, q.name)?
    } else {
        let access = token["access_token"]
            .as_str()
            .ok_or_else(|| AppError::BadRequest("No access token from provider".into()))?;
        fetch_user(c, access).await?
    };
    let pool = state.db()?;
    let provider = c.provider.key;

    let mut res = if let Some(user_id) = security::identity_user(pool, provider, &ext.subject).await.db()? {
        let res = crate::auth::mfa::finish_password_login(pool, user_id, &v).await?;
        crate::auth::mfa::logged_in(&state, user_id, provider, res)
    } else if let Some(me) = &v.user {
        // Connecting a provider is a way to sign in as this member later, so it takes the same
        // confirmation as a password or API key change; a stolen session alone must not plant one.
        if v.api_scope.is_some() {
            return Err(AppError::Forbidden);
        }
        // And it takes having asked for it, as this member, from Account settings.
        if link_for != Some(me.id.get()) {
            return Err(AppError::invalid(
                "You are already signed in. To connect {n} to this account, use Connected logins in Account settings.",
                &[("n", &c.label)],
            ));
        }
        if let Some(confirm) = v.sudo_redirect("/settings") {
            return Ok(confirm);
        }
        security::link_identity(pool, provider, &ext.subject, me.id, ext.email.as_deref()).await.db()?;
        Redirect::to("/settings").into_response()
    } else if link_for.is_some() {
        // Asked to connect, but no longer signed in: not a reason to sign in or register instead.
        return Err(AppError::BadRequest("Your login attempt expired. Please try again.".into()));
    } else {
        sign_in_external(&state, &v, provider, &c.label, &ext).await?
    };
    let clear = format!("{COOKIE}=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0");
    res.headers_mut()
        .append(header::SET_COOKIE, HeaderValue::from_str(&clear).unwrap_or_else(|_| HeaderValue::from_static("")));
    Ok(res)
}

/// Signs in someone an outside provider vouches for, creating or linking an account by the rules
/// below. `provider` is the stored identity's namespace; `label` is its name for people.
pub(crate) async fn sign_in_external(
    state: &AppState,
    v: &Viewer,
    provider: &str,
    label: &str,
    ext: &ExternalUser,
) -> Result<Response, AppError> {
    let pool = state.db()?;
    if let Some(user) = security::identity_user(pool, provider, &ext.subject).await.db()? {
        let res = crate::auth::mfa::finish_password_login(pool, user, &v).await?;
        return Ok(crate::auth::mfa::logged_in(state, user, provider, res));
    }
    let email = ext
        .email
        .as_deref()
        .ok_or_else(|| AppError::Validation("The provider did not share an email address.".into()))?;
    let email = bf_core::auth::normalize_email(email)
        .ok_or_else(|| AppError::Validation("Enter a valid email address.".into()))?;
    let existing = security::account_by_email(pool, &email).await.db()?;
    // A sign-up through the provider, as opposed to signing into an account that was there.
    let mut registered = false;
    let user_id = match existing {
        // Only an address the provider has verified may sign into an existing account.
        // The local address must be proven too, or whoever registered it first keeps a way in.
        Some(a) if ext.email_verified && a.email_verified => a.id,
        Some(_) => {
            return Err(AppError::invalid(
                "An account already uses {email}. Log in with your password, then connect {n} from Account settings.",
                &[("email", &email), ("n", &label)],
            ));
        }
        None => {
            let s = state.settings();
            // An unverified address would let this identity squat on someone else's email.
            if !ext.email_verified {
                return Err(AppError::Validation("Please verify your email address with the provider first.".into()));
            }
            if !s.bool("allow_registration") || s.bool("invite_only") {
                return Err(AppError::Forbidden);
            }
            if crate::auth::register::email_domain_refused(state, &email)
                || bf_db::moderation::bans::email_banned(pool, &email).await.db()?
            {
                return Err(AppError::Validation("That email address cannot be used here.".into()));
            }
            if s.int("minimum_age") > 0 {
                return Err(AppError::Validation(
                    "Please register with email first; this site checks your age at sign-up.".into(),
                ));
            }
            // The same caps as password registration: a sign-up farm must not skip them by
            // using a provider.
            crate::http::ratelimit::REGISTER_BY_IP.enforce(&format!("register:{}", crate::http::ratelimit::ip_key(v.ip)))?;
            if let Some(msg) = crate::auth::register::ip_cap_problem(state, pool, v.ip, &v.t()).await? {
                return Err(AppError::Validation(msg));
            }
            let (alias_local, alias_domain) = bf_core::auth::email_alias_key(&email);
            if bf_db::accounts::users::email_alias_owner(pool, &alias_local, &alias_domain).await.db()?.is_some() {
                return Err(AppError::Validation("That email address cannot be used here.".into()));
            }
            let mut base = username_base(&ext.name);
            if !crate::moderation::gate::username_ok(&state.forum.get(pool).await?.words, &base)
                || bf_db::accounts::users::skeleton_taken(pool, &bf_core::auth::username_skeleton(&base), None)
                    .await
                    .db()?
            {
                base = format!("member{}", &bf_core::auth::random_token()[..6]);
            }
            let id = security::create_external_user(pool, &base, &email, ext.email_verified).await.db()?;
            // The name is only final once the account exists (a taken base gets a number), so the
            // plugins' `user.before_register` sees that name; a refusal removes the new account.
            let final_name =
                bf_db::accounts::users::user_by_id(pool, id).await.db()?.map_or_else(|| base.clone(), |u| u.username);
            // The registration fields a provider cannot collect are skipped; plugins' sign-up
            // validation (`signup.validate`) still runs, with no answers.
            let mut refusal = crate::platform::plugins::registration_refused(state, &final_name, &email).await;
            if refusal.is_none() {
                refusal = crate::platform::plugins::signup_refused(state, &final_name, &email, &[]).await;
            }
            if let Some(why) = refusal {
                sqlx::query("DELETE FROM users WHERE id = $1").bind(id).execute(pool).await.db()?;
                return Err(AppError::Validation(why));
            }
            crate::platform::webhooks::emit(
                state,
                "user.registered",
                serde_json::json!({ "user_id": id, "username": final_name, "method": provider }),
            )
            .await;
            crate::community::referrals::credit(state, &v.theme_headers, id.get(), &final_name).await;
            registered = true;
            if s.bool("require_account_approval") && id.get() > 1 {
                bf_db::accounts::users::set_approved(pool, id, false).await.db()?;
                crate::community::notifications::tell_staff(state, id.get()).await;
            }
            id
        }
    };
    security::link_identity(pool, provider, &ext.subject, user_id, Some(&email)).await.db()?;
    let mut res = crate::auth::mfa::finish_password_login(pool, user_id, &v).await?;
    if registered {
        // New members land on the getting-started page, and find it again in their notifications.
        let welcome = serde_json::json!({ "name": state.settings().str("site_name"), "url": "/welcome" });
        if let Err(e) = bf_db::community::notifications::notify(pool, &[user_id.get()], "welcome", &welcome).await {
            tracing::warn!(error = %e, "welcome notification failed");
        }
        // Not when a second step is still to come: that redirect leads to it.
        if res.headers().get(header::LOCATION).is_none_or(|l| l.as_bytes() != b"/login/2fa") {
            res.headers_mut().insert(header::LOCATION, HeaderValue::from_static("/welcome"));
        }
    }
    Ok(crate::auth::mfa::logged_in(state, user_id, provider, res))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usernames_from_provider_names() {
        assert_eq!(username_base("Jane Doe"), "JaneDoe");
        assert_eq!(username_base("octo-cat"), "octo-cat");
        assert!(username_base("李").starts_with("member"));
        assert!(username_base("admin").starts_with("member"), "reserved names are not taken");
    }
}
