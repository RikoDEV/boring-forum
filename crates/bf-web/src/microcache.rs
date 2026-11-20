//! Guest microcache: rendered guest pages are kept in memory for a few seconds.
//! Concurrent misses for one key are coalesced, so 1,000 simultaneous guests cause
//! a single render.

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use moka::future::Cache;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// Bodies above this size are not cached, which keeps memory bounded.
const MAX_BODY: usize = 512 * 1024;
pub const SESSION_COOKIE: &str = "__Host-bf_session";

#[derive(Clone)]
pub struct Microcache {
    cache: Cache<String, Arc<Cached>>,
    /// Tells the other nodes to drop their copies too; set only when several nodes run.
    on_clear: Arc<OnceLock<Box<dyn Fn() + Send + Sync>>>,
}

struct Cached {
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
    storable: bool,
    /// Strong validator of the body, so a returning guest can be answered with 304.
    etag: Option<HeaderValue>,
}

/// A strong ETag from the body: the first 16 bytes of its SHA-256.
fn etag_of(body: &[u8]) -> Option<HeaderValue> {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(body);
    let tag = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&digest[..16]);
    HeaderValue::from_str(&format!("\"{tag}\"")).ok()
}

/// Whether an `If-None-Match` header lists the tag (or is `*`).
fn matches(header_value: &str, etag: &str) -> bool {
    header_value.split(',').map(str::trim).any(|t| t == "*" || t.trim_start_matches("W/") == etag)
}

impl Microcache {
    pub fn new(ttl: Duration) -> Self {
        Self { cache: Cache::builder().max_capacity(10_000).time_to_live(ttl).build(), on_clear: Arc::default() }
    }

    /// Drops every cached page, here and on the other nodes; called after writes until
    /// per-key purging lands with content.
    pub fn clear(&self) {
        self.cache.invalidate_all();
        if let Some(f) = self.on_clear.get() {
            f();
        }
    }

    /// Drops this node's pages only: another node asked for it.
    pub fn clear_local(&self) {
        self.cache.invalidate_all();
    }

    /// Runs `f` on every `clear` (once; a second call is ignored).
    pub fn set_on_clear(&self, f: impl Fn() + Send + Sync + 'static) {
        let _ = self.on_clear.set(Box::new(f));
    }
}

fn is_guest_get(req: &Request) -> bool {
    if req.method() != Method::GET || req.headers().contains_key(header::AUTHORIZATION) {
        return false;
    }
    // Files are streamed from the store, and each request is checked (who may have it, whether
    // another site embeds it): a copy kept here would skip both.
    if req.uri().path().starts_with("/uploads/") {
        return false;
    }
    let has_session = req
        .headers()
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .any(|c| c.split(';').any(|kv| kv.trim_start().starts_with(SESSION_COOKIE)));
    !has_session
}

fn storable(res: &Response) -> bool {
    res.status() == StatusCode::OK
        && !res.headers().contains_key(header::SET_COOKIE)
        && !res
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains("private") || v.contains("no-store"))
}

pub async fn layer(State(mc): State<Microcache>, req: Request, next: Next) -> Response {
    if !is_guest_get(&req) {
        return next.run(req).await;
    }
    // Guest pages are translated, so the language is part of the key.
    let lang_cookie = req
        .headers()
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|kv| kv.trim().strip_prefix("bf_lang="))
        .unwrap_or("")
        .to_owned();
    let accept = req.headers().get(header::ACCEPT_LANGUAGE).and_then(|v| v.to_str().ok()).unwrap_or("");
    let lang = if lang_cookie.is_empty() {
        bf_core::i18n::from_accept_language(accept).unwrap_or("").to_owned()
    } else {
        lang_cookie
    };
    let key = format!("{}|{lang}", req.uri());
    let if_none_match = req.headers().get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()).map(str::to_owned);
    let entry = mc
        .cache
        .get_with(key.clone(), async move {
            let res = next.run(req).await;
            let storable = storable(&res);
            let (parts, body) = res.into_parts();
            // The response is complete in memory either way; only what stays cached is bounded.
            let body = axum::body::to_bytes(body, usize::MAX).await;
            let storable = storable && body.as_ref().is_ok_and(|b| b.len() <= MAX_BODY);
            let body = body.unwrap_or_default();
            let etag = if storable && parts.status == StatusCode::OK { etag_of(&body) } else { None };
            Arc::new(Cached { status: parts.status, headers: parts.headers, body, storable, etag })
        })
        .await;
    if !entry.storable {
        mc.cache.invalidate(&key).await;
    }
    if let (Some(tag), Some(inm)) = (&entry.etag, &if_none_match)
        && matches(inm, tag.to_str().unwrap_or_default())
    {
        let mut res = Response::new(Body::empty());
        *res.status_mut() = StatusCode::NOT_MODIFIED;
        let h = res.headers_mut();
        h.insert(header::ETAG, tag.clone());
        // What a 304 repeats so the browser keeps treating the stored page the same way.
        for name in [header::CACHE_CONTROL, header::CONTENT_SECURITY_POLICY, header::EXPIRES] {
            if let Some(v) = entry.headers.get(&name) {
                h.insert(name, v.clone());
            }
        }
        h.insert(header::VARY, HeaderValue::from_static("Accept-Language, Cookie"));
        return res;
    }
    let mut res = Response::new(Body::from(entry.body.clone()));
    *res.status_mut() = entry.status;
    *res.headers_mut() = entry.headers.clone();
    if let Some(tag) = &entry.etag {
        res.headers_mut().insert(header::ETAG, tag.clone());
        // Guest pages change when the forum does: the browser asks again, and a 304 costs no body.
        if !res.headers().contains_key(header::CACHE_CONTROL) {
            res.headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=0, must-revalidate"));
        }
    }
    res.headers_mut()
        .insert("x-microcache", HeaderValue::from_static(if entry.storable { "hit-or-fill" } else { "bypass" }));
    res.headers_mut().insert(header::VARY, HeaderValue::from_static("Accept-Language, Cookie"));
    res.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, middleware, routing::get};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt;

    fn app(counter: Arc<AtomicUsize>) -> Router {
        let mc = Microcache::new(Duration::from_secs(5));
        Router::new()
            .route(
                "/",
                get(move || {
                    let c = counter.clone();
                    async move {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        format!("render {}", c.fetch_add(1, Ordering::SeqCst))
                    }
                }),
            )
            .layer(middleware::from_fn_with_state(mc, layer))
    }

    #[tokio::test]
    async fn coalesces_concurrent_guest_requests() {
        let counter = Arc::new(AtomicUsize::new(0));
        let app = app(counter.clone());
        let reqs = (0..50).map(|_| app.clone().oneshot(Request::get("/").body(Body::empty()).unwrap()));
        for res in futures_join(reqs).await {
            assert_eq!(res.unwrap().status(), StatusCode::OK);
        }
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn members_bypass_cache() {
        let counter = Arc::new(AtomicUsize::new(0));
        let app = app(counter.clone());
        for _ in 0..3 {
            let req = Request::get("/").header(header::COOKIE, "__Host-bf_session=abc").body(Body::empty()).unwrap();
            app.clone().oneshot(req).await.unwrap();
        }
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    async fn futures_join<F>(fs: impl Iterator<Item = F>) -> Vec<F::Output>
    where
        F: std::future::Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let handles: Vec<_> = fs.map(tokio::spawn).collect();
        let mut out = Vec::new();
        for h in handles {
            out.push(h.await.unwrap());
        }
        out
    }
}
