//! Site traffic: records one row per page view for the dashboard (`/api/v1/admin/traffic`).
//!
//! Nothing here identifies a person. No cookie is set and no address is stored: a visitor is a
//! hash of address, browser and the UTC day under a server secret, so it separates one day's
//! visitors (bounce rate, visit length) and cannot be followed to the next day or reversed.
//! Rows go to the database in batches, off the request path; a full queue drops views rather than
//! slowing a page.

use crate::state::AppState;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::Response;
use bf_db::admin::traffic::{self, View};
use sha2::{Digest, Sha256};
use std::net::{IpAddr, SocketAddr};
use std::sync::{LazyLock, OnceLock};
use std::time::Duration;
use tokio::sync::mpsc;

static KEY: LazyLock<Vec<u8>> = LazyLock::new(|| {
    for var in ["BF_SECRET_KEY", "BF_CAPTCHA_SECRET"] {
        if let Ok(v) = std::env::var(var)
            && !v.trim().is_empty()
        {
            return Sha256::digest(format!("bf-traffic:{v}")).to_vec();
        }
    }
    // Without a configured secret it lasts until restart: visitors still count, only a visit
    // split by a restart would count twice.
    Sha256::digest(format!("bf-traffic:{}", bf_core::auth::random_token())).to_vec()
});

static QUEUE: OnceLock<mpsc::Sender<View>> = OnceLock::new();

const BATCH: usize = 200;
const EVERY: Duration = Duration::from_secs(3);

/// The visitor of a day: stable within the UTC day, unrelated across days.
fn visitor(ip: IpAddr, agent: &str, day: &str) -> i64 {
    let mut h = Sha256::new();
    h.update(&*KEY);
    h.update(ip.to_string().as_bytes());
    h.update([0]);
    h.update(agent.as_bytes());
    h.update([0]);
    h.update(day.as_bytes());
    let d = h.finalize();
    i64::from_be_bytes([d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]])
}

/// The site a visitor came from, without `www.`; None for a direct visit or a link inside this site.
fn referrer(value: Option<&str>, own_host: &str) -> Option<String> {
    let url = url::Url::parse(value?).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?.trim_start_matches("www.").to_ascii_lowercase();
    let own = own_host.split(':').next().unwrap_or("").trim_start_matches("www.").to_ascii_lowercase();
    (!host.is_empty() && host != own).then(|| host.chars().take(120).collect())
}

fn counted_path(path: &str) -> bool {
    !(path.starts_with("/admin") || path.starts_with("/api/") || path.starts_with("/uploads/"))
}

fn sender(state: &AppState) -> Option<&'static mpsc::Sender<View>> {
    if let Some(tx) = QUEUE.get() {
        return Some(tx);
    }
    let pool = state.pool.clone()?;
    let (tx, mut rx) = mpsc::channel::<View>(10_000);
    if QUEUE.set(tx).is_err() {
        return QUEUE.get();
    }
    tokio::spawn(async move {
        let mut batch = Vec::with_capacity(BATCH);
        loop {
            let open = tokio::time::timeout(EVERY, async {
                while batch.len() < BATCH {
                    match rx.recv().await {
                        Some(v) => batch.push(v),
                        None => return false,
                    }
                }
                true
            })
            .await
            .unwrap_or(true);
            if !batch.is_empty() {
                if let Err(e) = traffic::insert(&pool, &batch).await {
                    tracing::warn!(error = %e, "page views not stored");
                }
                batch.clear();
            }
            if !open {
                break;
            }
        }
    });
    QUEUE.get()
}

/// Outside the microcache, so a page served from it is counted too.
pub(crate) async fn record(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let wanted = req.method() == Method::GET
        && counted_path(req.uri().path())
        && !req.headers().contains_key("purpose")
        && req.headers().get("sec-purpose").is_none()
        && state.settings().bool("record_pageviews");
    if !wanted {
        return next.run(req).await;
    }
    let headers = req.headers();
    let agent = headers.get(header::USER_AGENT).and_then(|h| h.to_str().ok()).unwrap_or("").to_owned();
    let own_host = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("");
    let came_from = referrer(headers.get(header::REFERER).and_then(|h| h.to_str().ok()), own_host);
    let signed_in = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .any(|c| c.split(';').any(|kv| kv.trim_start().starts_with("__Host-bf_session")));
    // E.g. BF_GEO_HEADER=CF-IPCountry; no address database is shipped.
    let country = std::env::var("BF_GEO_HEADER").ok().and_then(|name| {
        let v = headers.get(name.as_str())?.to_str().ok()?;
        (v.len() == 2 && v.chars().all(|c| c.is_ascii_alphabetic()) && !v.eq_ignore_ascii_case("xx"))
            .then(|| v.to_ascii_uppercase())
    });
    let ip = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| crate::http::ratelimit::client_ip(c.0.ip(), headers))
        .unwrap_or(IpAddr::from([0, 0, 0, 0]));
    let path: String = req.uri().path().chars().take(200).collect();
    let res = next.run(req).await;
    let html = res.status() == StatusCode::OK
        && res.headers().get(header::CONTENT_TYPE).and_then(|h| h.to_str().ok()).is_some_and(|t| t.starts_with("text/html"));
    if html && let Some(tx) = sender(&state) {
        let now = chrono::Utc::now();
        let kind = if crate::community::browsing::bot_name(&agent).is_some() {
            "crawler"
        } else if signed_in {
            "member"
        } else {
            "anonymous"
        };
        let _ = tx.try_send(View {
            at: now,
            path,
            referrer: came_from,
            country,
            kind,
            visitor: visitor(ip, &agent, &now.format("%Y-%m-%d").to_string()),
        });
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_inside_the_site_is_not_a_referrer() {
        assert_eq!(referrer(Some("https://www.google.com/search?q=x"), "forum.example"), Some("google.com".into()));
        assert_eq!(referrer(Some("https://forum.example/t/1"), "forum.example:8080"), None);
        assert_eq!(referrer(Some("android-app://com.x"), "forum.example"), None);
        assert_eq!(referrer(None, "forum.example"), None);
    }

    #[test]
    fn a_visitor_is_the_same_all_day_and_not_the_next() {
        let ip: IpAddr = "203.0.113.7".parse().unwrap();
        assert_eq!(visitor(ip, "Firefox", "2026-11-14"), visitor(ip, "Firefox", "2026-11-14"));
        assert_ne!(visitor(ip, "Firefox", "2026-11-14"), visitor(ip, "Firefox", "2026-11-15"));
        assert_ne!(visitor(ip, "Firefox", "2026-11-14"), visitor(ip, "Chrome", "2026-11-14"));
    }

    #[test]
    fn staff_and_api_paths_are_left_out() {
        assert!(counted_path("/t/hello/1"));
        assert!(!counted_path("/admin/forums/"));
        assert!(!counted_path("/api/v1/x"));
    }
}
