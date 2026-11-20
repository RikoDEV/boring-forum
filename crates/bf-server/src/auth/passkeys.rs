//! Passkey endpoints. The browser side is the `passkey-register` and
//! `passkey-login` islands; verification is `bf_core::webauthn`.
//!
//! Each ceremony is two calls: `.../options` stores a fresh challenge server-side
//! (keyed by an HttpOnly cookie) and returns the WebAuthn options; the second call
//! takes that challenge (once) and verifies the authenticator's response.

use crate::auth::login::start_session;
use crate::http::render::{DbResult, db_err};
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use bf_core::i18n::en;
use bf_core::webauthn::{RelyingParty, b64, unb64};
use bf_core::{AppError, auth, webauthn as wa};
use bf_db::accounts::security;
use rand::TryRng;
use rand::rngs::SysRng;
use serde::Deserialize;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/passkeys/register/options", post(register_options))
        .route("/api/v1/passkeys/register", post(register))
        .route("/api/v1/passkeys/login/options", post(login_options))
        .route("/api/v1/passkeys/login", post(login))
        .route("/api/v1/passkeys/signal", get(signal))
        .route("/settings/passkeys/{id}/delete", post(delete))
}

// `__Host-`: a sibling subdomain cannot plant one, as with the session cookie.
const COOKIE: &str = "__Host-bf_wa";

fn rp(state: &AppState) -> Result<RelyingParty<'_>, AppError> {
    RelyingParty::from_base_url(&state.config.base_url).ok_or_else(|| AppError::internal("BF_BASE_URL has no host"))
}

fn cookie_value(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|h| h.to_str().ok())
        .flat_map(|h| h.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v.to_owned())
}

/// Stores a new challenge and returns it with the cookie that names it.
async fn new_challenge(
    state: &AppState,
    user_id: Option<bf_core::ids::UserId>,
) -> Result<(Vec<u8>, HeaderValue), AppError> {
    let mut challenge = vec![0u8; 32];
    #[expect(
        clippy::expect_used,
        reason = "the operating system's random source is always present; without it nothing secure can run"
    )]
    SysRng.try_fill_bytes(&mut challenge).expect("the system random source is available");
    let token = auth::random_token();
    security::save_challenge(state.db()?, &auth::token_digest(&token), &challenge, user_id).await.db()?;
    let cookie = format!("{COOKIE}={token}; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=300");
    Ok((challenge, HeaderValue::from_str(&cookie).map_err(AppError::internal)?))
}

async fn take_challenge(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(Vec<u8>, Option<bf_core::ids::UserId>), AppError> {
    let token =
        cookie_value(headers).ok_or_else(|| AppError::BadRequest("The passkey request expired. Try again.".into()))?;
    security::take_challenge(state.db()?, &auth::token_digest(&token))
        .await
        .db()?
        .ok_or_else(|| AppError::BadRequest("The passkey request expired. Try again.".into()))
}

fn with_cookie(mut res: Response, cookie: HeaderValue) -> Response {
    res.headers_mut().append(header::SET_COOKIE, cookie);
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

async fn register_options(State(state): State<AppState>, v: Viewer, headers: HeaderMap) -> Result<Response, AppError> {
    v.check_session_csrf_headers(&headers)?;
    v.require_sudo()?;
    let user = v.require_user()?;
    let rp = rp(&state)?;
    let existing = security::passkeys(state.db()?, user.id).await.db()?;
    let (challenge, cookie) = new_challenge(&state, Some(user.id)).await?;
    let options = json!({
        "challenge": b64(&challenge),
        "rp": { "id": rp.id, "name": state.settings().str("site_name") },
        "user": { "id": b64(&user.id.get().to_be_bytes()), "name": user.username, "displayName": user.username },
        "pubKeyCredParams": [{ "type": "public-key", "alg": -7 }],
        "authenticatorSelection": { "residentKey": "preferred", "userVerification": "required" },
        "attestation": "none",
        "timeout": 120_000,
        "excludeCredentials": existing.iter().map(|p| json!({ "type": "public-key", "id": b64(&p.id) })).collect::<Vec<_>>(),
    });
    Ok(with_cookie(Json(options).into_response(), cookie))
}

#[derive(Deserialize)]
struct RegisterBody {
    #[serde(rename = "clientDataJSON")]
    client_data_json: String,
    #[serde(rename = "attestationObject")]
    attestation_object: String,
    #[serde(default)]
    name: String,
}

async fn register(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Json(b): Json<RegisterBody>,
) -> Result<Response, AppError> {
    v.check_session_csrf_headers(&headers)?;
    v.require_sudo()?;
    let user = v.require_user()?;
    let (challenge, owner) = take_challenge(&state, &headers).await?;
    if owner != Some(user.id) {
        return Err(AppError::Forbidden);
    }
    let cred =
        wa::verify_registration(&rp(&state)?, &challenge, &unb64(&b.client_data_json)?, &unb64(&b.attestation_object)?)
            .map_err(|e| AppError::invalid("The passkey could not be added: {e}.", &[("e", &e)]))?;
    let name: String = b.name.trim().chars().take(60).collect();
    let name = if name.is_empty() { "Passkey".to_owned() } else { name };
    let pool = state.db()?;
    match security::add_passkey(pool, &cred.id, user.id, &cred.public_key, i64::from(cred.sign_count), &name).await {
        Ok(()) => {}
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            return Err(AppError::Validation("That passkey is already registered.".into()));
        }
        Err(e) => return Err(db_err(e)),
    }
    crate::admin::api::audit_log(pool, Some(user.id.get()), "account.passkey_add", &format!("user:{}", user.id), serde_json::json!({})).await?;
    Ok((StatusCode::CREATED, Json(json!({ "ok": true }))).into_response())
}

async fn login_options(State(state): State<AppState>) -> Result<Response, AppError> {
    let rp = rp(&state)?;
    let (challenge, cookie) = new_challenge(&state, None).await?;
    // No allowCredentials: the authenticator offers its discoverable passkeys for this site.
    let options =
        json!({ "challenge": b64(&challenge), "rpId": rp.id, "userVerification": "required", "timeout": 120_000 });
    Ok(with_cookie(Json(options).into_response(), cookie))
}

#[derive(Deserialize)]
struct LoginBody {
    id: String,
    #[serde(rename = "clientDataJSON")]
    client_data_json: String,
    #[serde(rename = "authenticatorData")]
    authenticator_data: String,
    signature: String,
}

async fn login(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Json(b): Json<LoginBody>,
) -> Result<Response, AppError> {
    let (challenge, _) = take_challenge(&state, &headers).await?;
    let pool = state.db()?;
    let fail = || AppError::Validation("That passkey was not accepted.".into());
    let Some(row) = security::passkey(pool, &unb64(&b.id)?).await.db()? else {
        // The browser is told so it can drop the stale passkey (WebAuthn Signal API).
        let body = json!({ "type": "about:blank", "title": "Not Found", "status": 404,
            "detail": en("That passkey is not registered here."), "unknown_credential": true, "rpId": rp(&state)?.id });
        let mut res = (StatusCode::NOT_FOUND, Json(body)).into_response();
        res.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/problem+json"));
        return Ok(res);
    };
    let stored = wa::Credential {
        id: row.id.clone(),
        public_key: row.public_key.clone(),
        sign_count: u32::try_from(row.sign_count).unwrap_or(u32::MAX),
        user_verified: true,
    };
    let count = wa::verify_authentication(
        &rp(&state)?,
        &challenge,
        &stored,
        &unb64(&b.client_data_json)?,
        &unb64(&b.authenticator_data)?,
        &unb64(&b.signature)?,
    )
    .map_err(|e| {
        tracing::warn!(error = %e, "passkey sign-in rejected");
        fail()
    })?;
    security::passkey_used(pool, &row.id, i64::from(count)).await.db()?;
    // Suspended members cannot sign in, whatever the method.
    let suspended: bool =
        sqlx::query_scalar("SELECT coalesce(suspended_until > now(), false) FROM users WHERE id = $1")
            .bind(row.user_id)
            .fetch_one(pool)
            .await
            .db()?;
    if suspended {
        return Err(AppError::Forbidden);
    }
    // A passkey is possession plus (usually) user verification, so it stands in for
    // password + TOTP and signs in directly.
    let res = start_session(pool, row.user_id, v.ip).await?;
    crate::platform::webhooks::fire(&state, "user.login", json!({ "user_id": row.user_id, "method": "passkey" }));
    Ok(res)
}

/// WebAuthn Signal API: what the browser's passkey provider should know about this
/// account, so it can hide deleted passkeys and show the current name.
async fn signal(State(state): State<AppState>, v: Viewer) -> Result<Response, AppError> {
    let user = v.require_user()?;
    let rp = rp(&state)?;
    let keys = security::passkeys(state.db()?, user.id).await.db()?;
    let mut res = Json(json!({
        "rpId": rp.id,
        "userId": b64(&user.id.get().to_be_bytes()),
        "allAcceptedCredentialIds": keys.iter().map(|k| b64(&k.id)).collect::<Vec<_>>(),
        "name": user.username,
        "displayName": user.username,
    }))
    .into_response();
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(res)
}

#[derive(Deserialize)]
struct DeleteForm {
    csrf: String,
}

async fn delete(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<String>,
    Form(f): Form<DeleteForm>,
) -> Result<Response, AppError> {
    v.check_session_csrf(&f.csrf)?;
    if let Some(confirm) = v.sudo_redirect("/settings") {
        return Ok(confirm);
    }
    let user = v.require_user()?;
    security::delete_passkey(state.db()?, user.id, &unb64(&id)?).await.db()?;
    Ok(Redirect::to("/settings").into_response())
}

/// Passkeys for the settings page: (id as base64url, name, created, last used).
pub(crate) async fn list(
    state: &AppState,
    user_id: bf_core::ids::UserId,
) -> Result<Vec<(String, String, bf_web::ago::Ago, bf_web::ago::Ago)>, AppError> {
    Ok(security::passkeys(state.db()?, user_id)
        .await
        .db()?
        .into_iter()
        .map(|p| {
            let used = p.last_used_at.map_or_else(|| bf_web::ago::Ago::plain("never"), crate::http::render::ago);
            (b64(&p.id), p.name, crate::http::render::ago(p.created_at), used)
        })
        .collect())
}
