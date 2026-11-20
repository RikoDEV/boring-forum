//! Mailchimp newsletter sync. Members opt in at sign-up or under Account settings →
//! Notifications; each tick sends changed choices to the audience in `mailchimp_audience_id`
//! with the key in `BF_MAILCHIMP_API_KEY` (its `-us21` suffix names the data centre).
//! Mailchimp calls `/newsletter/mailchimp?secret=…` when someone unsubscribes there.

use crate::http::render::DbResult;
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Form, Router};
use base64::Engine;
use bf_core::AppError;
use bf_db::platform::newsletter as db;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;

pub fn router() -> Router<AppState> {
    // Mailchimp checks the address with a GET before it starts posting.
    Router::new().route("/newsletter/mailchimp", get(ping).post(hook))
}

fn api_key() -> Option<String> {
    std::env::var("BF_MAILCHIMP_API_KEY").ok().filter(|k| k.contains('-'))
}

/// Whether sign-up and settings offer the newsletter.
pub(crate) fn enabled(state: &AppState) -> bool {
    api_key().is_some() && !state.settings().str("mailchimp_audience_id").trim().is_empty()
}

/// `https://us21.api.mailchimp.com/3.0`, from the key's suffix.
fn base(key: &str) -> Option<String> {
    let dc = key.rsplit_once('-')?.1;
    (!dc.is_empty() && dc.chars().all(|c| c.is_ascii_alphanumeric()))
        .then(|| format!("https://{dc}.api.mailchimp.com/3.0"))
}

fn md5_hex(s: &str) -> String {
    // Mailchimp names members by the MD5 of the lowercased address. MD5 is only an identifier
    // here, so a small implementation keeps a dependency out.
    md5::digest(s.to_lowercase().as_bytes())
}

/// From the maintenance tick: up to 200 changed choices.
pub(crate) async fn run_tick(state: &AppState) {
    if !enabled(state) {
        return;
    }
    let (Some(pool), Some(key)) = (&state.pool, api_key()) else { return };
    let Some(base) = base(&key) else { return };
    let audience = state.settings().str("mailchimp_audience_id").trim().to_owned();
    if !audience.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return;
    }
    let rows = match db::pending(pool, 200).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "newsletter queue failed");
            return;
        }
    };
    let auth = format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("bf:{key}")));
    for (id, email, username, on, changed) in rows {
        let body = json!({
            "email_address": email,
            "status_if_new": if on { "subscribed" } else { "unsubscribed" },
            "status": if on { "subscribed" } else { "unsubscribed" },
            "merge_fields": { "FNAME": username },
        });
        let url = format!("{base}/lists/{audience}/members/{}", md5_hex(&email));
        let headers = vec![
            ("Authorization".to_owned(), auth.clone()),
            ("Content-Type".to_owned(), "application/json".to_owned()),
        ];
        match crate::platform::outbound::request_public(
            reqwest::Method::PUT,
            &url,
            &headers,
            Some(body.to_string().into_bytes()),
            1 << 16,
        )
        .await
        {
            Ok((200..=299, _)) => {
                if let Err(e) = db::synced(pool, id, changed).await {
                    tracing::warn!(error = %e, "newsletter sync mark failed");
                }
            }
            // 400 is Mailchimp refusing this address for good (a fake or compliance-blocked one):
            // trying again every tick would never succeed.
            Ok((400, _)) => {
                tracing::info!(user = id, "Mailchimp refused this address");
                let _ = db::synced(pool, id, changed).await;
            }
            Ok((status, _)) => {
                tracing::warn!(status, "Mailchimp sync failed; trying again next tick");
                return;
            }
            Err(e) => {
                tracing::warn!(error = %e, "Mailchimp unreachable; trying again next tick");
                return;
            }
        }
    }
}

#[derive(Deserialize)]
struct Secret {
    #[serde(default)]
    secret: String,
}

fn secret_ok(state: &AppState, given: &str) -> bool {
    let want = state.settings().str("mailchimp_webhook_secret").to_owned();
    want.len() >= 16 && crate::admin::api::constant_time_eq(want.as_bytes(), given.as_bytes())
}

async fn ping(State(state): State<AppState>, Query(q): Query<Secret>) -> Response {
    if secret_ok(&state, &q.secret) { StatusCode::OK } else { StatusCode::NOT_FOUND }.into_response()
}

/// Mailchimp's webhook is a form post: `type=unsubscribe&data[email]=…`.
async fn hook(
    State(state): State<AppState>,
    Query(q): Query<Secret>,
    Form(f): Form<HashMap<String, String>>,
) -> Result<Response, AppError> {
    if !secret_ok(&state, &q.secret) {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }
    if matches!(f.get("type").map(String::as_str), Some("unsubscribe" | "cleaned"))
        && let Some(email) = f.get("data[email]")
    {
        db::unsubscribe_email(state.db()?, email).await.db()?;
    }
    Ok(StatusCode::OK.into_response())
}

/// A small MD5 (RFC 1321), for Mailchimp's member ids only; never for security.
mod md5 {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14,
        20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6,
        10, 15, 21,
    ];

    pub(super) fn digest(input: &[u8]) -> String {
        let k: Vec<u32> = (0..64).map(|i| ((i as f64 + 1.0).sin().abs() * 4_294_967_296.0) as u32).collect();
        let (mut a0, mut b0, mut c0, mut d0) = (0x6745_2301u32, 0xefcd_ab89u32, 0x98ba_dcfeu32, 0x1032_5476u32);
        let mut msg = input.to_vec();
        let bits = (input.len() as u64).wrapping_mul(8);
        msg.push(0x80);
        while msg.len() % 64 != 56 {
            msg.push(0);
        }
        msg.extend_from_slice(&bits.to_le_bytes());
        for chunk in msg.chunks(64) {
            let m: Vec<u32> = chunk.chunks(4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]])).collect();
            let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
            for i in 0..64 {
                let (f, g) = match i {
                    0..=15 => ((b & c) | (!b & d), i),
                    16..=31 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                    32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                    _ => (c ^ (b | !d), (7 * i) % 16),
                };
                let f = f.wrapping_add(a).wrapping_add(k[i]).wrapping_add(m[g]);
                a = d;
                d = c;
                c = b;
                b = b.wrapping_add(f.rotate_left(S[i]));
            }
            a0 = a0.wrapping_add(a);
            b0 = b0.wrapping_add(b);
            c0 = c0.wrapping_add(c);
            d0 = d0.wrapping_add(d);
        }
        [a0, b0, c0, d0].iter().flat_map(|w| w.to_le_bytes()).map(|b| format!("{b:02x}")).collect()
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn known_digests() {
            assert_eq!(super::digest(b""), "d41d8cd98f00b204e9800998ecf8427e");
            assert_eq!(super::digest(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
            assert_eq!(super::digest(b"message digest"), "f96b697d7cb7938d525a2f31aaf161d0");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_centre_from_the_key() {
        assert_eq!(base("abc123-us21").as_deref(), Some("https://us21.api.mailchimp.com/3.0"));
        assert_eq!(base("abc123-us21.evil.com/"), None);
        assert_eq!(md5_hex("Urist@Example.com"), md5_hex("urist@example.com"));
    }
}
