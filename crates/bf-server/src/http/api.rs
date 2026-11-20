//! HTTP semantics for the JSON API: `Idempotency-Key` on writes
//! (draft-ietf-httpapi-idempotency-key-header) and validators on reads (RFC 9110 ETag /
//! If-None-Match).

use crate::state::AppState;
use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use bf_core::AppError;
use bf_db::platform::idempotency;
use bf_db::platform::idempotency::Claim;
use sha2::{Digest, Sha256};

/// Bodies above this are neither fingerprinted nor stored; such requests run without the key.
const MAX_BODY: usize = 1024 * 1024;

const MAX_STORED: usize = 256 * 1024;

const MAX_ETAG_BODY: usize = 4 * 1024 * 1024;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Who is calling: the API key or the session. A key only ever replays to the caller that set it.
fn caller(req: &Request) -> Option<String> {
    let h = req.headers();
    if let Some(auth) = h.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) {
        return Some(format!("auth:{}", bf_core::auth::token_digest(auth)));
    }
    crate::http::viewer::cookie(h, "__Host-bf_session").map(|s| format!("session:{}", bf_core::auth::token_digest(s)))
}

/// Whether the request comes from a signed-in member, a valid API key or the bootstrap admin token.
async fn authenticated(state: &AppState, parts: &mut axum::http::request::Parts) -> bool {
    use axum::extract::FromRequestParts;
    let bearer = parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "));
    if let (Some(given), Some(expected)) = (bearer, state.config.admin_token.as_deref())
        && bf_core::auth::constant_eq(given.as_bytes(), expected.as_bytes())
    {
        return true;
    }
    crate::http::viewer::Viewer::from_request_parts(parts, state).await.is_ok_and(|v| v.user.is_some())
}

pub async fn layer(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if !req.uri().path().starts_with("/api/v1/") {
        return next.run(req).await;
    }
    match *req.method() {
        Method::GET | Method::HEAD => etag(req, next).await,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE => idempotent(state, req, next).await,
        _ => next.run(req).await,
    }
}

async fn idempotent(state: AppState, req: Request, next: Next) -> Response {
    let Some(raw_key) = req.headers().get("idempotency-key").and_then(|v| v.to_str().ok()).map(str::to_owned) else {
        return next.run(req).await;
    };
    // A structured-field string in the draft; accepted quoted or bare, 1..255 visible ASCII.
    let key = raw_key.trim().trim_matches('"');
    if key.is_empty() || key.len() > 255 || !key.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
        return AppError::BadRequest("Idempotency-Key must be 1 to 255 visible ASCII characters.".into())
            .into_response();
    }
    let (Some(pool), Some(who)) = (state.pool.clone(), caller(&req)) else {
        return next.run(req).await;
    };
    let (mut parts, body) = req.into_parts();
    // A key is kept only for a caller who is signed in. Anyone can send a cookie or an
    // `Authorization` header of some value; reading a megabyte and storing a row for each such
    // request would be a way to fill the table without an account.
    if !authenticated(&state, &mut parts).await {
        return next.run(Request::from_parts(parts, body)).await;
    }
    let Ok(bytes) = to_bytes(body, MAX_BODY).await else {
        return AppError::BadRequest("Request body too large for an idempotent request.".into()).into_response();
    };
    let mut h = Sha256::new();
    h.update(parts.method.as_str());
    h.update([0]);
    h.update(parts.uri.path_and_query().map_or("", |p| p.as_str()));
    h.update([0]);
    h.update(&bytes);
    let fingerprint = hex(&h.finalize());
    let digest = hex(&Sha256::digest(format!("{who}\0{key}").as_bytes()));
    match idempotency::claim(&pool, &digest, &fingerprint).await {
        Ok(Claim::New) => {}
        Ok(Claim::InFlight) => {
            return problem(StatusCode::CONFLICT, "A request with this Idempotency-Key is still being processed.");
        }
        Ok(Claim::Mismatch) => {
            return problem(StatusCode::UNPROCESSABLE_ENTITY, "This Idempotency-Key was used for a different request.");
        }
        Ok(Claim::Done(status, ctype, body)) => {
            let mut res = Response::new(Body::from(body));
            *res.status_mut() = StatusCode::from_u16(status as u16).unwrap_or(StatusCode::OK);
            if let Some(v) = ctype.and_then(|c| HeaderValue::from_str(&c).ok()) {
                res.headers_mut().insert(header::CONTENT_TYPE, v);
            }
            res.headers_mut().insert("idempotent-replayed", HeaderValue::from_static("true"));
            return res;
        }
        Err(e) => {
            tracing::warn!(error = %e, "idempotency store unavailable; request runs unprotected");
            return next.run(Request::from_parts(parts, Body::from(bytes))).await;
        }
    }
    // If the client goes away the handler future is dropped: the guard frees the claim then.
    let mut guard = ClaimGuard { pool: pool.clone(), digest: digest.clone(), armed: true };
    let res = next.run(Request::from_parts(parts, Body::from(bytes))).await;
    let status = res.status();
    // Server errors and transient refusals (rate limit, auth, timeout) must not be replayed.
    if status.is_server_error()
        || matches!(status, StatusCode::TOO_MANY_REQUESTS | StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::REQUEST_TIMEOUT)
    {
        guard.armed = false;
        if let Err(e) = idempotency::release(&pool, &digest).await {
            tracing::warn!(error = %e, "idempotency release failed");
        }
        return res;
    }
    let (parts, body) = res.into_parts();
    let ctype = parts.headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(str::to_owned);
    let Ok(bytes) = to_bytes(body, MAX_STORED).await else {
        // Too large to keep; the key is released so a retry is not refused as in flight.
        guard.armed = false;
        let _ = idempotency::release(&pool, &digest).await;
        return (StatusCode::INTERNAL_SERVER_ERROR, "response too large").into_response();
    };
    if let Err(e) = idempotency::complete(&pool, &digest, status.as_u16() as i16, ctype.as_deref(), &bytes).await {
        tracing::warn!(error = %e, "idempotency completion failed");
    }
    guard.armed = false;
    Response::from_parts(parts, Body::from(bytes))
}

/// Frees an idempotency claim whose response was never stored (the handler future was dropped).
struct ClaimGuard {
    pool: sqlx::PgPool,
    digest: String,
    armed: bool,
}

impl Drop for ClaimGuard {
    fn drop(&mut self) {
        if self.armed {
            let (pool, digest) = (self.pool.clone(), std::mem::take(&mut self.digest));
            tokio::spawn(async move {
                let _ = idempotency::release(&pool, &digest).await;
            });
        }
    }
}

fn problem(status: StatusCode, detail: &str) -> Response {
    let body = serde_json::json!({ "type": "about:blank", "title": status.canonical_reason(), "status": status.as_u16(), "detail": detail });
    let mut res = (status, axum::Json(body)).into_response();
    res.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/problem+json"));
    res
}

/// A weak validator over the JSON body, so a polling client gets 304 and no body when nothing
/// changed. Streams (SSE) and responses that already carry a validator are left alone.
async fn etag(req: Request, next: Next) -> Response {
    let inm = req.headers().get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()).map(str::to_owned);
    let res = next.run(req).await;
    let json = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|c| c.starts_with("application/json") || c.starts_with("application/problem+json"));
    if res.status() != StatusCode::OK || !json || res.headers().contains_key(header::ETAG) {
        return res;
    }
    let (mut parts, body) = res.into_parts();
    let Ok(bytes) = to_bytes(body, MAX_ETAG_BODY).await else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "response too large").into_response();
    };
    let tag = format!("W/\"{}\"", &hex(&Sha256::digest(&bytes))[..32]);
    let matches = inm.is_some_and(|v| v.trim() == "*" || v.split(',').any(|t| weak_eq(t.trim(), &tag)));
    if let Ok(v) = HeaderValue::from_str(&tag) {
        parts.headers.insert(header::ETAG, v);
    }
    // Private API data may differ per caller; the validator is for this client's own cache.
    parts.headers.entry(header::CACHE_CONTROL).or_insert(HeaderValue::from_static("private, no-cache"));
    if matches {
        parts.status = StatusCode::NOT_MODIFIED;
        parts.headers.remove(header::CONTENT_LENGTH);
        parts.headers.remove(header::CONTENT_TYPE);
        return Response::from_parts(parts, Body::empty());
    }
    Response::from_parts(parts, Body::from(bytes))
}

/// RFC 9110 weak comparison: the opaque tags match whether or not either is marked weak.
fn weak_eq(a: &str, b: &str) -> bool {
    a.trim_start_matches("W/") == b.trim_start_matches("W/")
}

#[cfg(test)]
mod tests {
    #[test]
    fn weak_comparison_ignores_the_weak_marker() {
        assert!(super::weak_eq("W/\"abc\"", "\"abc\""));
        assert!(!super::weak_eq("\"abd\"", "W/\"abc\""));
    }
}
