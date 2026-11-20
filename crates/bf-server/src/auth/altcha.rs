//! Self-hosted proof-of-work challenge, compatible with the ALTCHA v1 payload format.
//! The browser searches for the number behind a signed hash; nothing leaves this site.

use crate::state::AppState;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use base64::Engine;
use bf_core::AppError;
use hmac::{Hmac, KeyInit, Mac};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/captcha/altcha", get(altcha_challenge))
}

/// Upper bound of the search: a few seconds of work in a browser, and about three times what a
/// script has to do for each account it makes. Proof of work only slows a bot down; the limits per
/// address are what stop one, and Turnstile or hCaptcha are stronger than this.
const MAX_NUMBER: u64 = 150_000;

const LIFETIME_SECONDS: i64 = 20 * 60;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn mac(secret: &str) -> Option<Hmac<Sha256>> {
    Hmac::<Sha256>::new_from_slice(secret.as_bytes()).ok()
}

fn digest(salt: &str, number: u64) -> String {
    hex(&Sha256::digest(format!("{salt}{number}").as_bytes()))
}

/// A new challenge. `number` is picked here and thrown away; only the signed hash goes out.
pub fn challenge(secret: &str, now: i64) -> Option<Value> {
    let salt = format!("{}?expires={}", bf_core::auth::random_token(), now + LIFETIME_SECONDS);
    let number = rand::random_range(0..=MAX_NUMBER);
    let challenge = digest(&salt, number);
    let mut signer = mac(secret)?;
    signer.update(challenge.as_bytes());
    let signature = hex(&signer.finalize().into_bytes());
    Some(
        json!({ "algorithm": "SHA-256", "challenge": challenge, "maxnumber": MAX_NUMBER, "salt": salt, "signature": signature }),
    )
}

#[derive(Deserialize)]
struct Payload {
    algorithm: String,
    challenge: String,
    number: u64,
    salt: String,
    signature: String,
}

/// A solved, signed and unexpired challenge: its signature, to be spent, and when it expires.
pub fn verify(secret: &str, payload: &str, now: i64) -> Option<(String, i64)> {
    let raw = base64::engine::general_purpose::STANDARD.decode(payload.trim()).ok()?;
    let p: Payload = serde_json::from_slice(&raw).ok()?;
    if p.algorithm != "SHA-256" || p.number > MAX_NUMBER {
        return None;
    }
    let expires = p.salt.rsplit_once("?expires=")?.1.parse::<i64>().ok()?;
    if expires <= now || digest(&p.salt, p.number) != p.challenge {
        return None;
    }
    let mut signer = mac(secret)?;
    signer.update(p.challenge.as_bytes());
    let wanted: Vec<u8> = (0..p.signature.len())
        .step_by(2)
        .map(|i| p.signature.get(i..i + 2).and_then(|b| u8::from_str_radix(b, 16).ok()))
        .collect::<Option<_>>()?;
    signer.verify_slice(&wanted).ok()?;
    // The decoded value, because hex can be written in several ways for the same bytes.
    Some((wanted.iter().map(|b| format!("{b:02x}")).collect(), expires))
}

/// A fresh challenge for the registration form; never cached, each one works once.
async fn altcha_challenge(State(state): State<AppState>) -> Result<Response, AppError> {
    if state.settings().str("captcha_provider") != "altcha" {
        return Err(AppError::NotFound);
    }
    let secret = std::env::var("BF_CAPTCHA_SECRET").map_err(|_| AppError::NotFound)?;
    let body = crate::auth::altcha::challenge(&secret, chrono::Utc::now().timestamp()).ok_or(AppError::NotFound)?;
    let mut res = Json(body).into_response();
    res.headers_mut().insert(axum::http::header::CACHE_CONTROL, axum::http::HeaderValue::from_static("no-store"));
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solved(c: &Value, number: Option<u64>) -> String {
        let salt = c["salt"].as_str().unwrap();
        let found = number.unwrap_or_else(|| {
            (0..=MAX_NUMBER).find(|n| digest(salt, *n) == c["challenge"].as_str().unwrap()).unwrap()
        });
        let body = json!({ "algorithm": "SHA-256", "challenge": c["challenge"], "number": found, "salt": salt, "signature": c["signature"] });
        base64::engine::general_purpose::STANDARD.encode(body.to_string())
    }

    #[test]
    fn accepts_a_solution_and_nothing_else() {
        let c = challenge("key", 1000).unwrap();
        assert!(verify("key", &solved(&c, None), 1001).is_some());
        assert!(verify("other key", &solved(&c, None), 1001).is_none(), "signed with another key");
        assert!(verify("key", &solved(&c, None), 1000 + LIFETIME_SECONDS).is_none(), "expired");
        let real =
            (0..=MAX_NUMBER).find(|n| digest(c["salt"].as_str().unwrap(), *n) == c["challenge"].as_str().unwrap());
        let wrong = if real == Some(1) { 2 } else { 1 };
        assert!(verify("key", &solved(&c, Some(wrong)), 1001).is_none(), "wrong number");
        assert!(verify("key", "not base64!", 1001).is_none());
    }
}
