//! In-process fixed-window rate limiter keyed by client IP (no Redis needed).

use axum::extract::{ConnectInfo, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use bf_core::AppError;
use dashmap::DashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct RateLimiter {
    limit: u32,
    window: Duration,
    hits: Arc<DashMap<IpAddr, (Instant, u32)>>,
}

impl RateLimiter {
    pub fn new(limit: u32, window: Duration) -> Self {
        Self { limit, window, hits: Arc::default() }
    }

    /// Records a hit; returns the seconds to wait when over the limit.
    pub fn check(&self, ip: IpAddr, now: Instant) -> Result<(), u64> {
        // An IPv6 client owns a whole /64, so its addresses share one bucket.
        let ip = bf_db::accounts::security::ip_bucket(ip);
        let mut entry = self.hits.entry(ip).or_insert((now, 0));
        let (start, count) = &mut *entry;
        if now.duration_since(*start) >= self.window {
            *start = now;
            *count = 0;
        }
        if *count >= self.limit {
            let left = self.window.saturating_sub(now.duration_since(*start));
            return Err(left.as_secs().max(1));
        }
        *count += 1;
        Ok(())
    }

    /// Removes expired windows so the map does not grow without bound.
    pub fn sweep(&self) {
        let now = Instant::now();
        self.hits.retain(|_, (start, _)| now.duration_since(*start) < self.window);
    }
}

/// A fixed-window limiter keyed by any string: an account, an address, a member id. Where
/// [`RateLimiter`] guards every request by address, these guard one action per account or route,
/// so many addresses cannot be used to flood one mailbox or one member's reports.
pub struct KeyedLimiter {
    limit: u32,
    window: Duration,
    hits: DashMap<String, (Instant, u32)>,
}

impl KeyedLimiter {
    pub fn new(limit: u32, window: Duration) -> Self {
        Self { limit, window, hits: DashMap::new() }
    }

    /// Records a hit; returns the seconds to wait when over the limit.
    pub fn check(&self, key: &str, now: Instant) -> Result<(), u64> {
        // Expired windows are dropped now and then, so the map cannot grow without bound.
        if self.hits.len() > 50_000 {
            self.hits.retain(|_, (start, _)| now.duration_since(*start) < self.window);
        }
        let mut entry = self.hits.entry(key.to_owned()).or_insert((now, 0));
        let (start, count) = &mut *entry;
        if now.duration_since(*start) >= self.window {
            *start = now;
            *count = 0;
        }
        if *count >= self.limit {
            return Err(self.window.saturating_sub(now.duration_since(*start)).as_secs().max(1));
        }
        *count += 1;
        Ok(())
    }

    /// `check` as an error a handler can return.
    pub fn enforce(&self, key: &str) -> Result<(), AppError> {
        self.check(key, Instant::now()).map_err(|retry_after_secs| {
            metrics::counter!("bf_rate_limited_total").increment(1);
            AppError::RateLimited { retry_after_secs }
        })
    }

    /// `check` as a yes/no, for places that must answer the same either way.
    pub fn allow(&self, key: &str) -> bool {
        self.enforce(key).is_ok()
    }
}

macro_rules! limiter {
    ($(#[$doc:meta])* $name:ident, $limit:expr, $secs:expr) => {
        $(#[$doc])*
        pub static $name: std::sync::LazyLock<KeyedLimiter> =
            std::sync::LazyLock::new(|| KeyedLimiter::new($limit, Duration::from_secs($secs)));
    };
}

limiter!(
    /// Mail that can be asked for on behalf of one address (reset, sign-in link, verification).
    MAIL_BY_ACCOUNT, 5, 3600
);
limiter!(
    /// The same, per visitor address and route, so one machine cannot walk through many mailboxes.
    MAIL_BY_IP, 15, 3600
);
/// Accounts created from one address (a /64 for IPv6) per hour: `BF_REGISTER_LIMIT_PER_HOUR`
/// (default 10). The end-to-end suite registers every test member from one address.
pub static REGISTER_BY_IP: std::sync::LazyLock<KeyedLimiter> = std::sync::LazyLock::new(|| {
    let limit = std::env::var("BF_REGISTER_LIMIT_PER_HOUR").ok().and_then(|v| v.parse().ok()).unwrap_or(10);
    KeyedLimiter::new(limit, Duration::from_secs(3600))
});
limiter!(
    /// Reports (posts, messages) filed by one member.
    REPORTS_BY_USER, 20, 3600
);
limiter!(
    /// Reactions given by one member.
    REACTIONS_BY_USER, 120, 60
);
limiter!(
    /// Searches run by one member or, for a guest, one address.
    SEARCH_BY_USER, 60, 60
);
limiter!(
    /// Searches by a guest: every one is a scan of the posts, so a visitor gets fewer than a member.
    SEARCH_BY_GUEST, 10, 60
);
limiter!(
    /// Data exports (the ZIP reads every file the member uploaded).
    EXPORTS_BY_USER, 3, 3600
);
limiter!(
    /// Files uploaded by one member.
    UPLOADS_BY_USER, 60, 3600
);

/// The address as a limiter key, with an IPv6 client's /64 as one.
pub fn ip_key(ip: Option<IpAddr>) -> String {
    ip.map_or_else(|| "none".to_owned(), |ip| bf_db::accounts::security::ip_bucket(ip).to_string())
}

/// A proxy address, or a network of them (`172.16.0.0/12`): what `BF_TRUSTED_PROXIES` lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Net {
    net: IpAddr,
    prefix: u8,
}

impl Net {
    /// One address.
    pub fn host(ip: IpAddr) -> Self {
        let ip = ip.to_canonical();
        Net { net: ip, prefix: if ip.is_ipv4() { 32 } else { 128 } }
    }

    /// `10.0.0.1`, `172.16.0.0/12` or `fd00::/8`. A network of every address (`/0`) is refused: it would
    /// make every visitor a proxy and let anyone claim any address.
    pub fn parse(text: &str) -> Option<Self> {
        let (addr, prefix) = match text.split_once('/') {
            Some((a, p)) => (a, Some(p)),
            None => (text, None),
        };
        let ip = addr.trim().parse::<IpAddr>().ok()?.to_canonical();
        let max = if ip.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(p) => p.trim().parse::<u8>().ok().filter(|p| (1..=max).contains(p))?,
            None => max,
        };
        Some(Net { net: Self::masked(ip, prefix), prefix })
    }

    fn masked(ip: IpAddr, prefix: u8) -> IpAddr {
        match ip {
            IpAddr::V4(v) => {
                let mask = u32::MAX.checked_shl(32 - u32::from(prefix)).unwrap_or(0);
                IpAddr::V4((u32::from(v) & mask).into())
            }
            IpAddr::V6(v) => {
                let mask = u128::MAX.checked_shl(128 - u32::from(prefix)).unwrap_or(0);
                IpAddr::V6((u128::from(v) & mask).into())
            }
        }
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        let ip = ip.to_canonical();
        ip.is_ipv4() == self.net.is_ipv4() && Self::masked(ip, self.prefix) == self.net
    }
}

fn is_listed(list: &[Net], ip: IpAddr) -> bool {
    list.iter().any(|n| n.contains(ip))
}

static TRUSTED: std::sync::LazyLock<Vec<Net>> = std::sync::LazyLock::new(|| {
    std::env::var("BF_TRUSTED_PROXIES")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .filter_map(|p| {
            // An entry that is dropped silently would leave every visitor sharing the proxy's address.
            Net::parse(p).or_else(|| {
                tracing::error!(
                    entry = p,
                    "BF_TRUSTED_PROXIES takes an address or a network such as 172.16.0.0/12 (not /0); entry ignored"
                );
                None
            })
        })
        .collect()
});

/// Cloudflare's edge networks (https://www.cloudflare.com/ips/). With the `cloudflare_real_ip`
/// setting on, a request from one of these is believed about the visitor: it is Cloudflare itself
/// talking, not someone who found the origin's address.
const CLOUDFLARE: [&str; 22] = [
    "173.245.48.0/20",
    "103.21.244.0/22",
    "103.22.200.0/22",
    "103.31.4.0/22",
    "141.101.64.0/18",
    "108.162.192.0/18",
    "190.93.240.0/20",
    "188.114.96.0/20",
    "197.234.240.0/22",
    "198.41.128.0/17",
    "162.158.0.0/15",
    "104.16.0.0/13",
    "104.24.0.0/14",
    "172.64.0.0/13",
    "131.0.72.0/22",
    "2400:cb00::/32",
    "2606:4700::/32",
    "2803:f800::/32",
    "2405:b500::/32",
    "2405:8100::/32",
    "2a06:98c0::/29",
    "2c0f:f248::/32",
];

static CLOUDFLARE_NETS: std::sync::LazyLock<Vec<Net>> =
    std::sync::LazyLock::new(|| CLOUDFLARE.iter().filter_map(|n| Net::parse(n)).collect());

/// The `cloudflare_real_ip` setting, kept here so the many callers of `client_ip` need no state.
static CLOUDFLARE_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Applies the `cloudflare_real_ip` setting (called whenever the settings are read).
pub fn set_cloudflare_real_ip(on: bool) {
    CLOUDFLARE_ON.store(on, std::sync::atomic::Ordering::Relaxed);
}

fn behind_cloudflare() -> bool {
    CLOUDFLARE_ON.load(std::sync::atomic::Ordering::Relaxed)
}

/// The visitor's address. `X-Forwarded-For` counts only from a proxy named in
/// `BF_TRUSTED_PROXIES`; otherwise anyone could claim any address. With `cloudflare_real_ip` on,
/// `CF-Connecting-IP` counts too, from Cloudflare's own addresses or such a proxy (a Cloudflare
/// Tunnel's connector, for one).
pub fn client_ip(peer: IpAddr, headers: &axum::http::HeaderMap) -> IpAddr {
    // A dual-stack listener reports IPv4 peers as ::ffff:a.b.c.d.
    let peer = peer.to_canonical();
    let cf = headers.get("cf-connecting-ip").and_then(|h| h.to_str().ok());
    let xff = headers.get("x-forwarded-for").and_then(|h| h.to_str().ok());
    resolve_client(peer, cf, xff, &TRUSTED, behind_cloudflare())
}

fn resolve_client(peer: IpAddr, cf: Option<&str>, xff: Option<&str>, trusted: &[Net], cloudflare: bool) -> IpAddr {
    if cloudflare
        && (is_listed(&CLOUDFLARE_NETS, peer) || is_listed(trusted, peer))
        && let Some(ip) = cf.and_then(|h| h.trim().parse::<IpAddr>().ok())
    {
        return ip.to_canonical();
    }
    forwarded_client(peer, xff, trusted)
}

/// Whether `peer` is a proxy whose `X-Forwarded-*` headers may be believed: one named in
/// `BF_TRUSTED_PROXIES`, or Cloudflare's edge when `cloudflare_real_ip` is on.
pub fn is_trusted_proxy(peer: IpAddr) -> bool {
    is_listed(&TRUSTED, peer) || (behind_cloudflare() && is_listed(&CLOUDFLARE_NETS, peer.to_canonical()))
}

/// Reads from the right: each proxy appends what it saw, so the rightmost address that is not
/// one of ours is the first anyone outside could not have forged.
fn forwarded_client(peer: IpAddr, header: Option<&str>, trusted: &[Net]) -> IpAddr {
    if !is_listed(trusted, peer) {
        return peer;
    }
    let mut last = peer;
    for part in header.unwrap_or_default().rsplit(',') {
        let Ok(ip) = part.trim().parse::<IpAddr>() else { return last };
        last = ip;
        if !is_listed(trusted, ip) {
            return ip;
        }
    }
    last
}

pub async fn layer(State(rl): State<RateLimiter>, req: Request, next: Next) -> Response {
    // Probes and the scraper come from a load balancer or orchestrator on one address; limiting
    // them would take the instance out of rotation under load. `/metrics` has its own token.
    if matches!(req.uri().path(), "/healthz" | "/livez" | "/readyz" | "/metrics") {
        return next.run(req).await;
    }
    // Without ConnectInfo (unit tests) every request shares one bucket.
    let peer = req.extensions().get::<ConnectInfo<SocketAddr>>().map_or(IpAddr::from([0, 0, 0, 0]), |c| c.0.ip());
    let ip = client_ip(peer, req.headers());
    match rl.check(ip, Instant::now()) {
        Ok(()) => next.run(req).await,
        Err(retry_after_secs) => {
            metrics::counter!("bf_rate_limited_total").increment(1);
            let mut res = AppError::RateLimited { retry_after_secs }.into_response();
            // The IETF RateLimit header fields (draft-ietf-httpapi-ratelimit-headers), so a
            // client learns the policy and when to come back, not only Retry-After.
            let policy = format!("\"ip\";q={};w={}", rl.limit, rl.window.as_secs());
            let left = format!("\"ip\";r=0;t={retry_after_secs}");
            let h = res.headers_mut();
            for (name, value) in [("ratelimit-policy", policy), ("ratelimit", left)] {
                if let Ok(v) = axum::http::HeaderValue::from_str(&value) {
                    h.insert(name, v);
                }
            }
            res
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwarded_addresses_count_only_from_trusted_proxies() {
        let (proxy, visitor, evil): (IpAddr, IpAddr, IpAddr) =
            ("10.0.0.1".parse().unwrap(), "203.0.113.5".parse().unwrap(), "198.51.100.9".parse().unwrap());
        let t = [Net::host(proxy)];
        assert_eq!(forwarded_client(visitor, Some("198.51.100.9"), &t), visitor, "not from our proxy");
        assert_eq!(forwarded_client(proxy, Some("203.0.113.5"), &t), visitor);
        assert_eq!(forwarded_client(proxy, Some("198.51.100.9, 203.0.113.5"), &t), visitor, "spoofed left part");
        assert_eq!(forwarded_client(proxy, Some("203.0.113.5, 10.0.0.1"), &t), visitor, "chain of proxies");
        assert_eq!(forwarded_client(proxy, Some("junk"), &t), proxy);
        assert_eq!(forwarded_client(proxy, None, &t), proxy);
        let _ = evil;
    }

    #[test]
    fn cloudflare_header_counts_only_when_on_and_from_a_trusted_peer() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        let (edge, visitor, other) = (ip("104.16.1.1"), ip("203.0.113.5"), ip("198.51.100.9"));
        let mine = [Net::host(ip("10.0.0.1"))];
        // On, from Cloudflare's edge: the header names the visitor, and wins over X-Forwarded-For.
        assert_eq!(resolve_client(edge, Some("203.0.113.5"), Some("198.51.100.9"), &[], true), visitor);
        // Off: the setting changes nothing, and the edge is just another peer.
        assert_eq!(resolve_client(edge, Some("203.0.113.5"), None, &[], false), edge);
        // On, but from an address that is neither Cloudflare nor ours: anyone can send the header.
        assert_eq!(resolve_client(other, Some("203.0.113.5"), None, &mine, true), other);
        // From a listed proxy (a tunnel's connector) it is believed.
        assert_eq!(resolve_client(ip("10.0.0.1"), Some("203.0.113.5"), None, &mine, true), visitor);
        // Junk falls back to the forwarding chain, and an IPv6 visitor works.
        assert_eq!(resolve_client(edge, Some("junk"), Some("203.0.113.5"), &mine, true), edge);
        assert_eq!(resolve_client(edge, Some("2001:db8::7"), None, &[], true), ip("2001:db8::7"));
        // The built-in list is all valid networks, and covers both families.
        assert_eq!(CLOUDFLARE_NETS.len(), CLOUDFLARE.len());
        assert!(is_listed(&CLOUDFLARE_NETS, ip("2606:4700::1")) && !is_listed(&CLOUDFLARE_NETS, ip("1.1.1.1")));
    }

    #[test]
    fn a_network_entry_covers_every_address_in_it() {
        let docker = Net::parse("172.16.0.0/12").expect("a /12");
        for inside in ["172.16.0.1", "172.17.0.2", "172.31.255.254"] {
            assert!(docker.contains(inside.parse().unwrap()), "{inside}");
        }
        for outside in ["172.15.255.255", "172.32.0.1", "10.0.0.1", "192.168.1.1", "::1"] {
            assert!(!docker.contains(outside.parse().unwrap()), "{outside}");
        }
        // The address given may be any in the network, and a plain address is one host.
        assert_eq!(Net::parse("172.20.5.9/12"), Some(docker));
        let one = Net::parse("10.0.0.1").expect("a host");
        assert!(one.contains("10.0.0.1".parse().unwrap()) && !one.contains("10.0.0.2".parse().unwrap()));
        // IPv6, and an IPv4 peer a dual-stack listener reports as ::ffff:a.b.c.d.
        let v6 = Net::parse("fd00::/8").expect("a v6 network");
        assert!(v6.contains("fd12:3456::1".parse().unwrap()) && !v6.contains("fe80::1".parse().unwrap()));
        assert!(Net::parse("192.168.0.0/16").unwrap().contains("::ffff:192.168.4.5".parse().unwrap()));
        // Not entries: junk, a prefix that is too long, and every address.
        for bad in ["junk", "10.0.0.1/33", "10.0.0.1/x", "0.0.0.0/0", "::/0", "/8", ""] {
            assert_eq!(Net::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn forwarded_addresses_trust_a_proxy_inside_a_listed_network() {
        let t = [Net::parse("172.16.0.0/12").unwrap()];
        let (proxy, visitor): (IpAddr, IpAddr) = ("172.19.0.5".parse().unwrap(), "203.0.113.5".parse().unwrap());
        assert_eq!(forwarded_client(proxy, Some("203.0.113.5"), &t), visitor);
        assert_eq!(forwarded_client(visitor, Some("198.51.100.9"), &t), visitor, "outside the network");
    }

    #[test]
    fn keyed_limiter_counts_per_key() {
        let rl = KeyedLimiter::new(2, Duration::from_secs(60));
        let t = Instant::now();
        assert!(rl.check("a@x", t).is_ok());
        assert!(rl.check("a@x", t).is_ok());
        assert!(rl.check("a@x", t).is_err());
        assert!(rl.check("b@x", t).is_ok());
        assert!(rl.check("a@x", t + Duration::from_secs(61)).is_ok());
    }

    #[test]
    fn limits_per_ip_and_resets_after_window() {
        let rl = RateLimiter::new(2, Duration::from_secs(60));
        let (a, b) = (IpAddr::from([1, 1, 1, 1]), IpAddr::from([2, 2, 2, 2]));
        let t = Instant::now();
        assert!(rl.check(a, t).is_ok());
        assert!(rl.check(a, t).is_ok());
        assert_eq!(rl.check(a, t + Duration::from_secs(10)), Err(50));
        assert!(rl.check(b, t).is_ok());
        assert!(rl.check(a, t + Duration::from_secs(61)).is_ok());
    }

    #[test]
    fn ipv6_clients_share_one_bucket_per_64() {
        let rl = RateLimiter::new(2, Duration::from_secs(60));
        let t = Instant::now();
        let (a, b, c): (IpAddr, IpAddr, IpAddr) = (
            "2001:db8:1:2::1".parse().unwrap(),
            "2001:db8:1:2:aaaa::9".parse().unwrap(),
            "2001:db8:1:3::1".parse().unwrap(),
        );
        assert!(rl.check(a, t).is_ok());
        assert!(rl.check(b, t).is_ok());
        assert!(rl.check(a, t).is_err(), "other addresses of the same /64 count against it");
        assert!(rl.check(c, t).is_ok(), "a different /64 has its own bucket");
    }
}
