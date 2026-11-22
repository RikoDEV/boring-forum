//! OAuth 2 / OIDC login: Google, GitHub, Discord and one generic OIDC provider.
//!
//! Configuration: Settings, Outside logins (`login_*`). A provider is on when its id and secret are
//! set and its `login_<provider>_enabled` switch is on, so an administrator can turn each off and on
//! without deleting its credentials.
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
use bf_core::settings::Settings;
use bf_core::{AppError, oauth};
use bf_db::accounts::security;
use serde::Deserialize;
use serde_json::Value;
use std::sync::LazyLock;
use std::time::Duration;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/{provider}", get(start).post(start_login))
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

/// A login's text setting, when it is filled in.
fn value(s: &Settings, setting: &str) -> Option<String> {
    Some(s.str(setting).trim().to_owned()).filter(|v| !v.is_empty())
}

/// The outside logins that are set up and switched on, read from the site settings (Settings, Outside
/// logins). Built on every use, so a change made in the admin panel applies at once.
fn providers(s: &Settings) -> Vec<Configured> {
    let mut out = Vec::new();
    for (key, label) in [
        ("google", "Google"),
        ("github", "GitHub"),
        ("discord", "Discord"),
        ("microsoft", "Microsoft"),
        ("twitch", "Twitch"),
        ("x", "X"),
    ] {
        if !s.bool(&format!("login_{key}_enabled")) {
            continue;
        }
        if let (Some(id), Some(secret)) =
            (value(s, &format!("login_{key}_id")), value(s, &format!("login_{key}_secret")))
        {
            let Some(provider) = Provider::builtin(key) else { continue };
            out.push(Configured { provider, label: label.into(), client_id: id, client_secret: secret, apple: None });
        }
    }
    if s.bool("login_oidc_enabled")
        && let (Some(a), Some(t), Some(u), Some(id), Some(secret)) = (
            value(s, "login_oidc_authorize_url"),
            value(s, "login_oidc_token_url"),
            value(s, "login_oidc_userinfo_url"),
            value(s, "login_oidc_client_id"),
            value(s, "login_oidc_client_secret"),
        )
    {
        match Provider::oidc(&a, &t, &u) {
            Ok(provider) => out.push(Configured {
                provider,
                label: value(s, "login_oidc_name").unwrap_or_else(|| "Single sign-on".into()),
                client_id: id,
                client_secret: secret,
                apple: None,
            }),
            Err(e) => tracing::error!(error = e, "the OpenID Connect login settings are ignored"),
        }
    }
    if s.bool("login_apple_enabled")
        && let (Some(id), Some(team_id), Some(key_id), Some(pem)) = (
            value(s, "login_apple_id"),
            value(s, "login_apple_team_id"),
            value(s, "login_apple_key_id"),
            value(s, "login_apple_key"),
        )
    {
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
}

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
pub(crate) fn enabled(s: &Settings) -> Vec<(&'static str, String)> {
    let mut all: Vec<_> = providers(s).iter().map(|c| (c.provider.key, c.label.clone())).collect();
    if crate::auth::identity::sso_enabled(s) {
        all.push(("sso", crate::auth::identity::sso_name(s)));
    }
    all
}

/// (key, label) of the providers a member can connect from Account settings. The login site of
/// `/auth/sso` signs in, but is not connected this way.
pub(crate) fn linkable(s: &Settings) -> Vec<(&'static str, String)> {
    providers(s).iter().map(|c| (c.provider.key, c.label.clone())).collect()
}

fn find(s: &Settings, key: &str) -> Result<Configured, AppError> {
    providers(s).into_iter().find(|c| c.provider.key == key).ok_or(AppError::NotFound)
}

fn redirect_uri(state: &AppState, key: &str) -> String {
    format!("{}/auth/{key}/callback", state.config.base_url)
}

#[derive(Deserialize)]
struct StartQuery {
    /// A script presses the button on the page it gets (`auth_immediately`).
    #[serde(default)]
    auto: String,
}

/// A visit to `/auth/{provider}`: with `auth_require_interaction` a page with a button, otherwise
/// the login itself.
async fn start(
    State(state): State<AppState>,
    v: Viewer,
    Path(key): Path<String>,
    Query(q): Query<StartQuery>,
) -> Result<Response, AppError> {
    let c = find(&state.settings(), &key)?;
    if crate::auth::external::interaction_required(&state) {
        return Ok(crate::auth::external::continue_page(&state, &v, &key, &c.label, q.auto == "1"));
    }
    begin_login(&state, &c)
}

/// The click that starts a login: the login page's buttons and the button on the page above.
async fn start_login(
    State(state): State<AppState>,
    v: Viewer,
    Path(key): Path<String>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let c = find(&state.settings(), &key)?;
    let (url, cookie) = begin(&state, &c, None)?;
    send_on(&state, &v, &c.label, &url, cookie, (en("Log in"), "/login"))
}

fn begin_login(state: &AppState, c: &Configured) -> Result<Response, AppError> {
    let (url, cookie) = begin(state, c, None)?;
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
    let c = find(&state.settings(), &key)?;
    if let Some(confirm) = v.sudo_redirect("/settings") {
        return Ok(confirm);
    }
    let (url, cookie) = begin(&state, &c, Some(me))?;
    send_on(&state, &v, &c.label, &url, cookie, (en("Account settings"), "/settings"))
}

/// A page that sends the browser on, not a redirect: a browser holds the redirects answering
/// a form to the page's `form-action` policy, which allows this site only, and a provider's
/// own sign-in may pass through several of its hosts. `back` is the way out of the page.
pub(crate) fn send_on(
    state: &AppState,
    v: &Viewer,
    label: &str,
    url: &str,
    cookie: HeaderValue,
    back: (&'static str, &'static str),
) -> Result<Response, AppError> {
    let f = Form1 {
        heading: en("Connecting…"),
        action: String::new(),
        fields: vec![],
        button: "",
        message: Some(v.t().msgf("You are being sent to {n} to sign in there.", &[("n", label)])),
        error: None,
        links: vec![back],
    };
    let mut res = form_page(state, v, f, StatusCode::OK);
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
    let cookie =
        format!("{COOKIE}={}.{}.{link_for}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=600", s.state, s.verifier);
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
async fn form_post(
    State(state): State<AppState>,
    Path(key): Path<String>,
    Form(f): Form<FormPost>,
) -> Result<Response, AppError> {
    let c = find(&state.settings(), &key)?;
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
    Ok(ExternalUser {
        subject,
        email,
        email_verified,
        name: name.clone().unwrap_or(fallback),
        username: None,
        full_name: name,
        avatar_url: None,
    })
}

pub(crate) struct ExternalUser {
    pub(crate) subject: String,
    pub(crate) email: Option<String>,
    pub(crate) email_verified: bool,
    /// What a new account's username is made from: the provider's handle, or failing that a name.
    pub(crate) name: String,
    /// The provider's own handle for the member, when it has one (`auth_overrides_username`).
    pub(crate) username: Option<String>,
    /// The member's full name (`auth_overrides_name`).
    pub(crate) full_name: Option<String>,
    /// An https address of the member's picture (`auth_overrides_avatar`).
    pub(crate) avatar_url: Option<String>,
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
    let mut profile = HTTP.get(c.provider.userinfo_url.clone()).bearer_auth(access_token);
    if c.provider.key == "twitch" {
        // Twitch's API wants the application's client ID with every call.
        profile = profile.header("Client-Id", &c.client_id);
    }
    let mut info: Value = profile
        .header(header::ACCEPT, "application/json")
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| AppError::invalid("The provider did not return your profile: {e}", &[("e", &e)]))?
        .json()
        .await
        .map_err(AppError::internal)?;
    // Twitch answers `{"data": [user]}` and X `{"data": user}`; the rest send the user itself.
    if matches!(c.provider.key, "twitch" | "x")
        && let Some(user) = info.get("data").and_then(|d| d.as_array().and_then(|a| a.first()).or(Some(d))).cloned()
    {
        info = user;
    }
    let subject = str_field(&info, "sub")
        .or_else(|| str_field(&info, "id"))
        .ok_or_else(|| AppError::BadRequest("No user id from provider".into()))?;
    let mut email = str_field(&info, "email");
    let mut email_verified =
        info.get("email_verified").or_else(|| info.get("verified")).and_then(Value::as_bool).unwrap_or(false);
    // Twitch only lets a member keep a confirmed address, and shares it only with the email scope.
    if c.provider.key == "twitch" && email.is_some() {
        email_verified = true;
    }
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
    let username = ["preferred_username", "login", "username", "nickname"].iter().find_map(|k| str_field(&info, k));
    let full_name = str_field(&info, "name").or_else(|| str_field(&info, "display_name")).or_else(|| {
        let joined = [str_field(&info, "given_name"), str_field(&info, "family_name")]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        (!joined.is_empty()).then_some(joined)
    });
    let name = username
        .clone()
        .or_else(|| str_field(&info, "given_name"))
        .or_else(|| full_name.clone())
        .unwrap_or_else(|| "member".into());
    // Google and OIDC send `picture`, GitHub `avatar_url`, Discord a hash to build the address from.
    let avatar_url = str_field(&info, "picture")
        .or_else(|| str_field(&info, "avatar_url"))
        // Twitch and X; X's is a 48 px thumbnail unless asked for the larger one.
        .or_else(|| str_field(&info, "profile_image_url").map(|u| u.replace("_normal.", "_400x400.")))
        .or_else(|| {
            let (id, hash) = (str_field(&info, "id")?, str_field(&info, "avatar")?);
            (c.provider.key == "discord")
                .then(|| format!("https://cdn.discordapp.com/avatars/{id}/{hash}.png?size=256"))
        });
    Ok(ExternalUser { subject, email, email_verified, name, username, full_name, avatar_url })
}

/// A valid username made from a provider's name, or `None` when nothing usable is left of it.
pub(crate) fn clean_username(name: &str) -> Option<String> {
    let s: String =
        name.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')).take(16).collect();
    let s = s.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    (s.len() >= 3 && bf_core::auth::validate_username(s).is_ok()).then(|| s.to_owned())
}

/// A valid username from a provider's display name.
pub(crate) fn username_base(name: &str) -> String {
    clean_username(name).unwrap_or_else(|| format!("member{}", &bf_core::auth::random_token()[..6]))
}

async fn callback(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(key): Path<String>,
    Query(q): Query<Callback>,
) -> Result<Response, AppError> {
    let c = find(&state.settings(), &key)?;
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
    let secret = c.secret()?;
    let redirect = redirect_uri(&state, c.provider.key);
    // X takes the client's credentials as HTTP Basic, and its ID in the body; the others in the body.
    let (body, basic) = if c.provider.basic_auth {
        (oauth::token_request_body(&code, verifier, &c.client_id, "", &redirect).replace("&client_secret=", ""), true)
    } else {
        (oauth::token_request_body(&code, verifier, &c.client_id, &secret, &redirect), false)
    };
    let mut request = HTTP
        .post(&c.provider.token_url)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::ACCEPT, "application/json")
        .body(body);
    if basic {
        request = request.basic_auth(&c.client_id, Some(&secret));
    }
    let token: Value = request
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| AppError::invalid("The provider did not accept the login: {e}", &[("e", &e)]))?
        .json()
        .await
        .map_err(AppError::internal)?;
    let ext = if c.apple.is_some() {
        apple_user(&c, &token, q.name)?
    } else {
        let access = token["access_token"]
            .as_str()
            .ok_or_else(|| AppError::BadRequest("No access token from provider".into()))?;
        fetch_user(&c, access).await?
    };
    let pool = state.db()?;
    let provider = c.provider.key;

    let mut res = if let Some(user_id) = security::identity_user(pool, provider, &ext.subject).await.db()? {
        crate::auth::external::sync_profile(&state, user_id, provider, &ext, false).await;
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
        sign_in_external(&state, &v, provider, &c.label, &ext, None).await?
    };
    let clear = format!("{COOKIE}=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0");
    res.headers_mut()
        .append(header::SET_COOKIE, HeaderValue::from_str(&clear).unwrap_or_else(|_| HeaderValue::from_static("")));
    Ok(res)
}

/// Signs in someone an outside provider vouches for, creating or linking an account by the rules
/// below. `provider` is the stored identity's namespace; `label` is its name for people. `chosen` is
/// the username a new member picked on the confirmation page.
pub(crate) async fn sign_in_external(
    state: &AppState,
    v: &Viewer,
    provider: &str,
    label: &str,
    ext: &ExternalUser,
    chosen: Option<&str>,
) -> Result<Response, AppError> {
    let pool = state.db()?;
    if let Some(user) = security::identity_user(pool, provider, &ext.subject).await.db()? {
        crate::auth::external::sync_profile(state, user, provider, ext, false).await;
        let res = crate::auth::mfa::finish_password_login(pool, user, v).await?;
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
            // A new member confirms the account first, unless the site skips that. Everything above
            // refuses before this page, and the confirmation runs it all again.
            if chosen.is_none() && !s.bool("auth_skip_create_confirm") {
                return Ok(crate::auth::external::ask_to_confirm(provider, label, ext));
            }
            // The same caps as password registration: a sign-up farm must not skip them by
            // using a provider.
            crate::http::ratelimit::REGISTER_BY_IP
                .enforce(&format!("register:{}", crate::http::ratelimit::ip_key(v.ip)))?;
            if let Some(msg) = crate::auth::register::ip_cap_problem(state, pool, v.ip, &v.t()).await? {
                return Err(AppError::Validation(msg));
            }
            let (alias_local, alias_domain) = bf_core::auth::email_alias_key(&email);
            if bf_db::accounts::users::email_alias_owner(pool, &alias_local, &alias_domain).await.db()?.is_some() {
                return Err(AppError::Validation("That email address cannot be used here.".into()));
            }
            let mut base = chosen.map_or_else(|| username_base(&ext.name), str::to_owned);
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
    crate::auth::external::sync_profile(state, user_id, provider, ext, registered).await;
    let mut res = crate::auth::mfa::finish_password_login(pool, user_id, v).await?;
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
