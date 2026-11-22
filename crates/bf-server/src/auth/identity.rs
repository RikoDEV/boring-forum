//! The forum as an identity provider (OAuth2 authorization code with PKCE) and as a client of
//! an external login site (DiscourseConnect-style SSO), plus the API key form.

use crate::admin::api::{Admin, AdminRead, AdminWrite};
use crate::http::render::{DbResult, layout, page};
use crate::http::viewer::{CsrfForm, Viewer, cookie};
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{delete, get, post};
use axum::{Form, Json, Router};
use base64::Engine;
use bf_core::AppError;
use bf_core::auth::{random_token, token_digest};
use bf_db::accounts::security;
use bf_db::admin::notices;
use bf_db::platform::{api, identity};
use bf_web::pages::{ConsentPage, ErrorPage, render};
use serde::Deserialize;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/oauth/authorize", get(authorize).post(approve))
        .route("/oauth/token", post(token))
        .route("/oauth/userinfo", get(userinfo))
        .route("/api/v1/admin/oauth-clients", get(list_clients).post(create_client))
        .route("/api/v1/admin/oauth-clients/{id}", delete(delete_client))
        .route("/api/v1/admin/notices", get(notices))
        .route("/api/v1/admin/notices/{id}", delete(dismiss))
        .route("/settings/api-keys", post(create_key_form))
        .route("/settings/api-keys/{id}/delete", post(delete_key_form))
        .route("/auth/sso", get(sso_visit).post(sso_start))
        .route("/auth/sso/callback", get(sso_callback))
}

#[derive(Deserialize)]
struct AuthorizeQuery {
    #[serde(default)]
    response_type: String,
    #[serde(default)]
    client_id: String,
    #[serde(default)]
    redirect_uri: String,
    #[serde(default)]
    scope: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    code_challenge: String,
    #[serde(default)]
    code_challenge_method: String,
}

/// Only read and write; a third-party app can never get admin rights.
fn clean_scope(scope: &str) -> &'static str {
    if scope.split_whitespace().any(|s| s == "write") { "write" } else { "read" }
}

/// Everything that must hold before showing consent or redirecting anywhere. The redirect
/// address must match a registered one exactly, or this would be an open redirect.
async fn check_request(state: &AppState, q: &AuthorizeQuery) -> Result<String, AppError> {
    let client = identity::client(state.db()?, &q.client_id)
        .await
        .db()?
        .ok_or_else(|| AppError::BadRequest("Unknown application.".into()))?;
    if !client.redirect_uris.contains(&q.redirect_uri) {
        return Err(AppError::BadRequest("That return address is not registered for this application.".into()));
    }
    if q.response_type != "code" || q.code_challenge_method != "S256" || q.code_challenge.len() < 43 {
        return Err(AppError::BadRequest("Use the code flow with an S256 PKCE challenge.".into()));
    }
    Ok(client.name)
}

async fn authorize(
    State(state): State<AppState>,
    v: Viewer,
    Query(q): Query<AuthorizeQuery>,
) -> Result<Response, AppError> {
    let name = check_request(&state, &q).await?;
    if v.user.is_none() {
        return Ok(Redirect::to("/login").into_response());
    }
    let (mut l, set) = layout(&state, &v, Some(&v.t().msg("Authorize application")));
    l.noindex = true;
    // The form's answer is a redirect back to the application, which the page's policy
    // (`form-action`) has to allow: this one registered address, nothing else off the site.
    let back = url::Url::parse(&q.redirect_uri)
        .ok()
        .filter(|u| matches!(u.scheme(), "http" | "https"))
        .map(|u| u.origin().ascii_serialization());
    let p = ConsentPage {
        layout: l,
        app: name,
        write: clean_scope(&q.scope) == "write",
        client_id: q.client_id,
        redirect_uri: q.redirect_uri,
        scope: clean_scope(&q.scope).to_owned(),
        state: q.state,
        code_challenge: q.code_challenge,
    };
    let mut res = page(render(&p), set, false);
    if let Some(origin) = back {
        res.extensions_mut().insert(crate::http::middleware::FormActionOrigin(origin));
    }
    Ok(res)
}

#[derive(Deserialize)]
struct ApproveForm {
    csrf: String,
    #[serde(flatten)]
    q: AuthorizeForm,
    #[serde(default)]
    deny: String,
}

#[derive(Deserialize)]
struct AuthorizeForm {
    client_id: String,
    redirect_uri: String,
    scope: String,
    #[serde(default)]
    state: String,
    code_challenge: String,
}

async fn approve(State(state): State<AppState>, v: Viewer, Form(f): Form<ApproveForm>) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    if v.api_scope.is_some() {
        return Err(AppError::Forbidden);
    }
    let q = f.q;
    let check = AuthorizeQuery {
        response_type: "code".into(),
        client_id: q.client_id.clone(),
        redirect_uri: q.redirect_uri.clone(),
        scope: q.scope.clone(),
        state: q.state.clone(),
        code_challenge: q.code_challenge.clone(),
        code_challenge_method: "S256".into(),
    };
    check_request(&state, &check).await?;
    let mut back = url::Url::parse(&q.redirect_uri).map_err(|_| AppError::BadRequest("bad return address".into()))?;
    if !f.deny.is_empty() {
        back.query_pairs_mut().append_pair("error", "access_denied").append_pair("state", &q.state);
        return Ok(Redirect::to(back.as_str()).into_response());
    }
    let code = random_token();
    let digest = token_digest(&code);
    identity::create_code(
        state.db()?,
        &identity::NewCode {
            digest: &digest,
            client_id: &q.client_id,
            user_id: user.id.get(),
            scope: clean_scope(&q.scope),
            redirect_uri: &q.redirect_uri,
            challenge: &q.code_challenge,
        },
    )
    .await
    .db()?;
    back.query_pairs_mut().append_pair("code", &code).append_pair("state", &q.state);
    Ok(Redirect::to(back.as_str()).into_response())
}

#[derive(Deserialize)]
struct TokenForm {
    #[serde(default)]
    grant_type: String,
    #[serde(default)]
    code: String,
    #[serde(default)]
    redirect_uri: String,
    #[serde(default)]
    client_id: String,
    #[serde(default)]
    client_secret: String,
    #[serde(default)]
    code_verifier: String,
}

fn oauth_error(code: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": code }))).into_response()
}

async fn token(State(state): State<AppState>, Form(f): Form<TokenForm>) -> Result<Response, AppError> {
    let pool = state.db()?;
    if f.grant_type != "authorization_code" {
        return Ok(oauth_error("unsupported_grant_type"));
    }
    let Some(stored) = identity::client_secret_digest(pool, &f.client_id).await.db()? else {
        return Ok(oauth_error("invalid_client"));
    };
    if !bf_core::auth::csrf_matches(Some(&stored), Some(&token_digest(&f.client_secret))) {
        return Ok(oauth_error("invalid_client"));
    }
    let Some((client, user, scope, redirect, challenge)) =
        identity::take_code(pool, &token_digest(&f.code)).await.db()?
    else {
        return Ok(oauth_error("invalid_grant"));
    };
    let pkce = bf_core::oauth::pkce_challenge(&f.code_verifier);
    if client != f.client_id || redirect != f.redirect_uri || pkce != challenge {
        return Ok(oauth_error("invalid_grant"));
    }
    let access = format!("bf_{}", random_token());
    identity::create_access_token(pool, user, &client, &token_digest(&access), &scope).await.db()?;
    let mut res = Json(json!({ "access_token": access, "token_type": "Bearer", "expires_in": 3600, "scope": scope }))
        .into_response();
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(res)
}

async fn userinfo(v: Viewer) -> Result<Response, AppError> {
    if v.api_scope.is_none() {
        return Err(AppError::Forbidden);
    }
    let u = v.require_user()?;
    Ok(Json(json!({
        "sub": u.id.to_string(), "preferred_username": u.username, "email": u.email, "email_verified": u.email_verified,
    }))
    .into_response())
}

#[derive(Deserialize)]
struct NewClient {
    name: String,
    redirect_uris: Vec<String>,
}

async fn create_client(
    State(state): State<AppState>,
    Admin(..): AdminWrite,
    Json(b): Json<NewClient>,
) -> Result<Response, AppError> {
    let ok = |u: &String| {
        u.starts_with("https://") || u.starts_with("http://127.0.0.1:") || u.starts_with("http://localhost:")
    };
    if b.name.trim().is_empty() || b.redirect_uris.is_empty() || !b.redirect_uris.iter().all(ok) {
        return Err(AppError::Validation("Give a name and https return addresses.".into()));
    }
    let (client_id, secret) = (random_token()[..24].to_owned(), random_token());
    identity::create_client(state.db()?, &client_id, &token_digest(&secret), b.name.trim(), &b.redirect_uris)
        .await
        .db()?;
    Ok((StatusCode::CREATED, Json(json!({ "client_id": client_id, "client_secret": secret }))).into_response())
}

async fn list_clients(State(state): State<AppState>, Admin(..): AdminRead) -> Result<Response, AppError> {
    Ok(Json(identity::clients(state.db()?).await.db()?).into_response())
}

async fn delete_client(
    State(state): State<AppState>,
    Admin(..): AdminWrite,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    identity::delete_client(state.db()?, &id).await.found()?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn notices(State(state): State<AppState>, Admin(..): AdminRead) -> Result<Response, AppError> {
    Ok(Json(notices::notices(state.db()?).await.db()?).into_response())
}

async fn dismiss(
    State(state): State<AppState>,
    Admin(..): AdminWrite,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    notices::dismiss_notice(state.db()?, id).await.found()?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize)]
struct KeyForm {
    csrf: String,
    name: String,
    scope: String,
    #[serde(default)]
    days: String,
    /// `scope_plugin:<id>:<key>=1` for each plugin scope ticked.
    #[serde(flatten)]
    extra: std::collections::HashMap<String, String>,
}

/// The same as the JSON endpoint, for the settings page; the token shows once, on this page.
async fn create_key_form(
    State(state): State<AppState>,
    v: Viewer,
    Form(f): Form<KeyForm>,
) -> Result<Response, AppError> {
    v.check_session_csrf(&f.csrf)?;
    if let Some(confirm) = v.sudo_redirect("/settings") {
        return Ok(confirm);
    }
    let u = v.require_user()?;
    let name = f.name.trim();
    if name.is_empty() || name.len() > 80 || !matches!(f.scope.as_str(), "read" | "write") {
        return Err(AppError::Validation("Give the key a name and choose read or write.".into()));
    }
    let token = format!("bf_{}", random_token());
    let key_id = api::create_key(
        state.db()?,
        u.id.get(),
        name,
        &token_digest(&token),
        &f.scope,
        api::key_days(f.days.parse().ok()),
    )
    .await
    .db()?;
    let wanted: Vec<String> =
        f.extra.keys().filter_map(|k| k.strip_prefix("scope_plugin:")).map(|s| format!("plugin:{s}")).collect();
    if !wanted.is_empty() {
        let held = bf_db::platform::registry_content::existing_scopes(state.db()?, &wanted).await.db()?;
        api::set_key_scopes(state.db()?, key_id, &held).await.db()?;
    }
    let (mut l, set) = layout(&state, &v, Some(&v.t().msg("API key created")));
    l.noindex = true;
    let p = ErrorPage {
        layout: l,
        status: 201,
        message: v.t().msgf("Copy this key now; it is not shown again: {token}", &[("token", &token)]),
    };
    Ok(page((StatusCode::CREATED, render(&p)).into_response(), set, false))
}

async fn delete_key_form(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    v.check_session_csrf(&f.csrf)?;
    let u = v.require_user()?;
    api::delete_key(state.db()?, u.id.get(), id).await.db()?;
    Ok(Redirect::to("/settings").into_response())
}

// --- DiscourseConnect-style SSO: BF_SSO_URL and BF_SSO_SECRET (32+ characters).

// `__Host-`: a sibling subdomain cannot plant one, as with the session cookie.
const SSO_COOKIE: &str = "__Host-bf_sso";

fn sso_config() -> Option<(String, String)> {
    let url =
        std::env::var("BF_SSO_URL").ok().filter(|u| u.starts_with("https://") || u.starts_with("http://127.0.0.1:"))?;
    let secret = std::env::var("BF_SSO_SECRET").ok().filter(|s| s.len() >= 32)?;
    Some((url, secret))
}

pub(crate) fn sso_enabled() -> bool {
    sso_config().is_some()
}

fn hex_signature(secret: &str, payload: &str) -> String {
    bf_core::auth::hmac_sha256_hex(secret, payload.as_bytes())
}

#[derive(Deserialize)]
struct SsoVisit {
    #[serde(default)]
    auto: String,
}

/// A visit to `/auth/sso`: with `auth_require_interaction` a page with a button, otherwise the login.
async fn sso_visit(State(state): State<AppState>, v: Viewer, Query(q): Query<SsoVisit>) -> Result<Response, AppError> {
    if !crate::auth::external::interaction_required(&state) {
        let (url, cookie) = sso_begin(&state).await?;
        let mut res = Redirect::to(&url).into_response();
        res.headers_mut().append(header::SET_COOKIE, cookie);
        return Ok(res);
    }
    sso_config().ok_or(AppError::NotFound)?;
    let label = std::env::var("BF_SSO_NAME").unwrap_or_else(|_| "Single sign-on".into());
    Ok(crate::auth::external::continue_page(&state, &v, "sso", &label, q.auto == "1"))
}

/// The click that starts the login (see `sso_visit`).
async fn sso_start(State(state): State<AppState>, v: Viewer, Form(f): Form<CsrfForm>) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let (url, cookie) = sso_begin(&state).await?;
    let label = std::env::var("BF_SSO_NAME").unwrap_or_else(|_| "Single sign-on".into());
    crate::auth::oauth::send_on(&state, &v, &label, &url, cookie, (bf_core::i18n::en("Log in"), "/login"))
}

/// Where to send the browser to sign in at the login site, and the cookie that ties the answer to it.
async fn sso_begin(state: &AppState) -> Result<(String, HeaderValue), AppError> {
    let (url, secret) = sso_config().ok_or(AppError::NotFound)?;
    let nonce = random_token();
    bf_db::accounts::security::remember_sso_nonce(state.db()?, &bf_core::auth::token_digest(&nonce)).await.db()?;
    let inner = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("nonce", &nonce)
        .append_pair("return_sso_url", &format!("{}/auth/sso/callback", state.config.base_url))
        .finish();
    let payload = base64::engine::general_purpose::STANDARD.encode(inner);
    let mut target = url::Url::parse(&url).map_err(AppError::internal)?;
    target.query_pairs_mut().append_pair("sso", &payload).append_pair("sig", &hex_signature(&secret, &payload));
    let c = format!("{SSO_COOKIE}={nonce}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=600");
    Ok((target.to_string(), HeaderValue::from_str(&c).map_err(AppError::internal)?))
}

#[derive(Deserialize)]
struct SsoReturn {
    sso: String,
    sig: String,
}

async fn sso_callback(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Query(q): Query<SsoReturn>,
) -> Result<Response, AppError> {
    let (_, secret) = sso_config().ok_or(AppError::NotFound)?;
    let refused = || AppError::BadRequest("That sign-in could not be verified. Please try again.".into());
    if !bf_core::auth::csrf_matches(Some(&hex_signature(&secret, &q.sso)), Some(&q.sig)) {
        return Err(refused());
    }
    let inner = base64::engine::general_purpose::STANDARD.decode(q.sso.as_bytes()).map_err(|_| refused())?;
    let fields: std::collections::HashMap<String, String> = url::form_urlencoded::parse(&inner).into_owned().collect();
    // The nonce ties the answer to this browser's request, so a signed payload cannot be replayed.
    let expected = cookie(&headers, SSO_COOKIE).ok_or_else(refused)?;
    if fields.get("nonce").map(String::as_str) != Some(expected) {
        return Err(refused());
    }
    let (Some(subject), Some(email)) = (fields.get("external_id"), fields.get("email")) else { return Err(refused()) };
    let pool = state.db()?;
    if !security::spend_sso_nonce(pool, &bf_core::auth::token_digest(expected)).await.db()? {
        return Err(refused());
    }
    // The same rules as any outside provider: a known identity signs in; an existing account is
    // joined only when its own address is verified too; a new account obeys the registration
    // settings (open or closed, invites, minimum age, approval, banned addresses).
    let ext = crate::auth::oauth::ExternalUser {
        subject: subject.clone(),
        email: Some(email.clone()),
        // DiscourseConnect says so when the login site has not confirmed the address.
        email_verified: fields.get("require_activation").map(String::as_str) != Some("true"),
        name: fields.get("username").cloned().unwrap_or_else(|| "member".to_owned()),
        username: fields.get("username").cloned(),
        full_name: fields.get("name").cloned().filter(|n| !n.trim().is_empty()),
        avatar_url: fields.get("avatar_url").cloned().filter(|u| !u.is_empty()),
    };
    let mut res = crate::auth::oauth::sign_in_external(&state, &v, "sso", "SSO", &ext, None).await?;
    let clear = format!("{SSO_COOKIE}=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0");
    res.headers_mut().append(header::SET_COOKIE, HeaderValue::from_str(&clear).map_err(AppError::internal)?);
    Ok(res)
}
