//! Web push: RFC 8291 message encryption and RFC 8292 VAPID. A worker watches for
//! new notifications, so every feature that notifies gets push without knowing about it. It is
//! off unless `BF_VAPID_PRIVATE` holds a base64url P-256 private key (see `boring-forum vapid`).

use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use aes_gcm::aead::Aead;
use aes_gcm::{Aes128Gcm, KeyInit};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use bf_core::AppError;
use bf_db::platform::push;
use hkdf::Hkdf;
use p256::ecdh::diffie_hellman;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::elliptic_curve::sec1::ToSec1Point;
use p256::{PublicKey, SecretKey};
use serde::Deserialize;
use serde_json::json;
use sha2::Sha256;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/push/key", get(public_key))
        .route("/api/v1/push/subscribe", post(subscribe).delete(unsubscribe))
}

/// Each is a request the server makes per notification, to an address the member chose.
const MAX_SUBSCRIPTIONS: i64 = 10;

fn vapid_key() -> Option<SecretKey> {
    let raw = B64.decode(std::env::var("BF_VAPID_PRIVATE").ok()?.trim()).ok()?;
    SecretKey::from_slice(&raw).ok()
}

/// Whether Web Push is set up (`BF_VAPID_PRIVATE`); pages offer it only then.
pub fn configured() -> bool {
    vapid_key().is_some()
}

/// `boring-forum vapid` prints a new key pair.
pub fn generate_keys() -> String {
    let key = <SecretKey as p256::elliptic_curve::Generate>::generate();
    format!("BF_VAPID_PRIVATE={}\n", B64.encode(key.to_bytes()))
}

/// RFC 8291: the body for one subscription. `salt` and the sender's key are fixed only in tests.
fn encrypt(
    plaintext: &[u8],
    subscriber: &[u8],
    auth: &[u8],
    sender: &SecretKey,
    salt: &[u8; 16],
) -> Result<Vec<u8>, String> {
    let ua = PublicKey::from_sec1_bytes(subscriber).map_err(|e| e.to_string())?;
    let shared = diffie_hellman(sender.to_nonzero_scalar(), ua.as_affine());
    let as_public = sender.public_key().to_sec1_point(false);

    let mut info = Vec::with_capacity(144);
    info.extend_from_slice(b"WebPush: info\0");
    info.extend_from_slice(subscriber);
    info.extend_from_slice(as_public.as_bytes());
    let mut ikm = [0u8; 32];
    Hkdf::<Sha256>::new(Some(auth), shared.raw_secret_bytes().as_slice())
        .expand(&info, &mut ikm)
        .map_err(|e| e.to_string())?;

    let hk = Hkdf::<Sha256>::new(Some(salt), &ikm);
    let mut cek = [0u8; 16];
    hk.expand(b"Content-Encoding: aes128gcm\0", &mut cek).map_err(|e| e.to_string())?;
    let mut nonce = [0u8; 12];
    hk.expand(b"Content-Encoding: nonce\0", &mut nonce).map_err(|e| e.to_string())?;

    // One record, with the delimiter that ends the last record.
    let mut record = plaintext.to_vec();
    record.push(2);
    let cipher = Aes128Gcm::new_from_slice(&cek).map_err(|e| e.to_string())?;
    let sealed = cipher.encrypt((&nonce).into(), record.as_slice()).map_err(|e| e.to_string())?;

    let mut out = Vec::with_capacity(86 + sealed.len());
    out.extend_from_slice(salt);
    out.extend_from_slice(&4096u32.to_be_bytes());
    out.push(65);
    out.extend_from_slice(as_public.as_bytes());
    out.extend_from_slice(&sealed);
    Ok(out)
}

/// RFC 8292: `Authorization: vapid t=<jwt>, k=<public key>` for the push service's origin.
fn vapid_header(endpoint: &str, key: &SecretKey, subject: &str, now: i64) -> Result<String, String> {
    let u = url::Url::parse(endpoint).map_err(|e| e.to_string())?;
    let audience = u.origin().ascii_serialization();
    let header = B64.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
    let claims = B64.encode(json!({ "aud": audience, "exp": now + 12 * 3600, "sub": subject }).to_string());
    let signing = format!("{header}.{claims}");
    let signature: Signature = SigningKey::from(key).sign(signing.as_bytes());
    let public = B64.encode(key.public_key().to_sec1_point(false).as_bytes());
    Ok(format!("vapid t={signing}.{}, k={public}", B64.encode(signature.to_bytes())))
}

async fn public_key() -> Result<Response, AppError> {
    let key = vapid_key().ok_or(AppError::NotFound)?;
    let public = B64.encode(key.public_key().to_sec1_point(false).as_bytes());
    Ok(Json(json!({ "key": public })).into_response())
}

#[derive(Deserialize)]
struct Keys {
    p256dh: String,
    auth: String,
}

#[derive(Deserialize)]
struct Subscription {
    endpoint: String,
    keys: Keys,
}

async fn subscribe(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Json(s): Json<Subscription>,
) -> Result<Response, AppError> {
    v.check_csrf_headers(&headers)?;
    let user = v.require_user()?;
    vapid_key().ok_or(AppError::NotFound)?;
    // The server later posts to this address, so it gets the same checks as a webhook's.
    if !crate::platform::outbound::acceptable_target(&s.endpoint) || s.endpoint.len() > 1000 {
        return Err(AppError::Validation("That is not a push address.".into()));
    }
    let good = B64.decode(&s.keys.p256dh).is_ok_and(|k| PublicKey::from_sec1_bytes(&k).is_ok())
        && B64.decode(&s.keys.auth).is_ok_and(|a| a.len() == 16);
    if !good {
        return Err(AppError::Validation("Those are not valid push keys.".into()));
    }
    if push::count_for(state.db()?, user.id.get()).await.db()? >= MAX_SUBSCRIPTIONS {
        return Err(AppError::Validation("Too many devices have notifications on. Turn some off first.".into()));
    }
    push::add(state.db()?, user.id.get(), &s.endpoint, &s.keys.p256dh, &s.keys.auth).await.db()?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize)]
struct Unsubscribe {
    endpoint: String,
}

async fn unsubscribe(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Json(s): Json<Unsubscribe>,
) -> Result<Response, AppError> {
    v.check_csrf_headers(&headers)?;
    push::remove(state.db()?, (v.require_user()?.id).get(), &s.endpoint).await.db()?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn send_one(row: &push::Due, key: &SecretKey, subject: &str, payload: &[u8]) -> Result<u16, String> {
    let subscriber = B64.decode(&row.p256dh).map_err(|e| e.to_string())?;
    let auth = B64.decode(&row.auth).map_err(|e| e.to_string())?;
    let sender = <SecretKey as p256::elliptic_curve::Generate>::generate();
    let salt: [u8; 16] = rand::random();
    let body = encrypt(payload, &subscriber, &auth, &sender, &salt)?;
    let authorization = vapid_header(&row.endpoint, key, subject, chrono::Utc::now().timestamp())?;
    crate::platform::outbound::post_public(
        &row.endpoint,
        &[("content-encoding", "aes128gcm"), ("ttl", "86400"), ("authorization", &authorization)],
        body,
    )
    .await
}

/// What to do with a send's outcome.
#[derive(Debug, PartialEq)]
enum Outcome {
    Delivered,
    /// The browser dropped this subscription.
    Gone,
    /// The service refused for good (a bad request); asking again will not change that.
    Refused,
    /// Worth another try: a server error, a rate limit, or a network failure.
    Retry,
}

fn outcome(sent: &Result<u16, String>) -> Outcome {
    match sent {
        Ok(404 | 410) => Outcome::Gone,
        Ok(408 | 429) | Err(_) => Outcome::Retry,
        Ok(s) if *s >= 500 => Outcome::Retry,
        Ok(s) if *s >= 400 => Outcome::Refused,
        Ok(_) => Outcome::Delivered,
    }
}

/// Runs only when configured, and costs a few small queries every few seconds when it does. The
/// queue lives in the database, so any number of nodes share it and each message goes out once.
pub fn spawn_worker(state: AppState) {
    let Some(key) = vapid_key() else { return };
    let Some(pool) = state.pool.clone() else { return };
    let subject = std::env::var("BF_VAPID_SUBJECT").unwrap_or_else(|_| state.config.base_url.clone());
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(bf_core::scaled(std::time::Duration::from_secs(5)));
        loop {
            tick.tick().await;
            if let Err(e) = push::fan_out(&pool, 200).await {
                tracing::warn!(error = %e, "push fan-out failed");
            }
            let due = match push::due(&pool, 100).await {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!(error = %e, "push query failed");
                    continue;
                }
            };
            for row in due {
                let body = json!({
                    "title": state.settings().str("site_name"),
                    "body": bf_core::notifications::text(
                        &bf_core::i18n::T::new(row.locale.as_deref().unwrap_or(bf_core::i18n::site_language(state.settings().str("default_locale")))),
                        &row.notification.kind,
                        &row.notification.data,
                    ),
                    "url": format!("/notifications/{}/open", row.notification_id),
                });
                let sent = send_one(&row, &key, &subject, body.to_string().as_bytes()).await;
                let saved = match outcome(&sent) {
                    Outcome::Delivered => push::push_done(&pool, row.queue_id).await,
                    Outcome::Gone => push::remove_endpoint(&pool, &row.endpoint).await,
                    Outcome::Refused => {
                        tracing::warn!(result = ?sent, "web message refused");
                        push::push_done(&pool, row.queue_id).await
                    }
                    Outcome::Retry => {
                        tracing::warn!(result = ?sent, attempt = row.attempts + 1, "web message failed; will retry");
                        push::push_failed(&pool, row.queue_id).await
                    }
                };
                if let Err(e) = saved {
                    tracing::warn!(error = %e, "web message status update failed");
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The worked example in RFC 8291, Appendix A.
    #[test]
    fn matches_the_rfc_example() {
        let d = |s: &str| B64.decode(s).unwrap();
        let sender = SecretKey::from_slice(&d("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw")).unwrap();
        let ua = d("BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4");
        let auth = d("BTBZMqHH6r4Tts7J_aSIgg");
        let salt: [u8; 16] = d("DGv6ra1nlYgDCS1FRnbzlw").try_into().unwrap();
        let out = encrypt(b"When I grow up, I want to be a watermelon", &ua, &auth, &sender, &salt).unwrap();
        assert_eq!(
            B64.encode(out),
            "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN"
        );
    }

    #[test]
    fn failed_sends_are_retried_only_when_they_can_succeed() {
        assert_eq!(outcome(&Ok(201)), Outcome::Delivered);
        assert_eq!(outcome(&Ok(410)), Outcome::Gone);
        assert_eq!(outcome(&Ok(404)), Outcome::Gone);
        assert_eq!(outcome(&Ok(400)), Outcome::Refused);
        assert_eq!(outcome(&Ok(429)), Outcome::Retry);
        assert_eq!(outcome(&Ok(503)), Outcome::Retry);
        assert_eq!(outcome(&Err("connection reset".into())), Outcome::Retry);
    }

    #[test]
    fn vapid_token_is_for_the_push_origin() {
        let key = <SecretKey as p256::elliptic_curve::Generate>::generate();
        let h = vapid_header("https://push.example/send/abc", &key, "mailto:a@example.com", 1000).unwrap();
        let jwt = h.strip_prefix("vapid t=").unwrap().split(',').next().unwrap();
        let claims: serde_json::Value =
            serde_json::from_slice(&B64.decode(jwt.split('.').nth(1).unwrap()).unwrap()).unwrap();
        assert_eq!(claims["aud"], "https://push.example");
        assert_eq!(claims["exp"], 1000 + 12 * 3600);
    }
}
