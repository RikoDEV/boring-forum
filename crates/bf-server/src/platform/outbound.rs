//! Outbound HTTP to member-supplied addresses, with SSRF protection: link previews, webhook
//! and federation deliveries.

use bf_core::i18n::en;
use std::net::IpAddr;
use std::time::Duration;

pub(crate) fn loopback_allowed() -> bool {
    std::env::var("BF_WEBHOOK_ALLOW_LOOPBACK").is_ok_and(|v| v == "1")
}

/// Addresses a webhook must never reach: this server's own network.
pub(crate) fn forbidden_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => {
            a.is_private()
                || a.is_loopback()
                || a.is_link_local()
                || a.is_broadcast()
                || a.is_documentation()
                || a.is_unspecified()
                || a.is_multicast()
                || a.octets()[0] == 0
                || a.octets()[0] >= 240
                || a.octets()[0] == 100 && (64..128).contains(&a.octets()[1]) // shared address space
                || a.octets()[0] == 198 && (18..20).contains(&a.octets()[1]) // benchmarking
                || a.octets()[..3] == [192, 0, 0] // protocol assignments
        }
        IpAddr::V6(a) => {
            let s = a.segments();
            a.is_loopback()
                || a.is_unspecified()
                || (s[0] & 0xfe00) == 0xfc00
                || (s[0] & 0xffc0) == 0xfe80
                || (s[0] & 0xff00) == 0xff00 // multicast
                || (s[0] == 0x2001 && s[1] == 0x0db8) // documentation
                || (s[0] == 0x2001 && s[1] == 0) // Teredo: tunnels to an IPv4 address it hides
                || (s[0] == 0x0064 && s[1] == 0xff9b && s[2] == 1) // local-use NAT64 (RFC 8215)
                || (s[0] == 0x0100 && s[1..4].iter().all(|&x| x == 0)) // discard-only
                // Addresses that carry an IPv4 one: NAT64 and 6to4.
                || (s[0] == 0x0064 && s[1] == 0xff9b && forbidden_ip(IpAddr::V4(std::net::Ipv4Addr::new(
                    (s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8))))
                || (s[0] == 0x2002 && forbidden_ip(IpAddr::V4(std::net::Ipv4Addr::new(
                    (s[1] >> 8) as u8, s[1] as u8, (s[2] >> 8) as u8, s[2] as u8))))
                || a.to_ipv4_mapped().is_some_and(|m| forbidden_ip(IpAddr::V4(m)))
                // Deprecated IPv4-compatible form `::a.b.c.d` (first 96 bits zero): judge it by the
                // embedded IPv4, so `::127.0.0.1` and the like cannot slip past.
                || (s[..6].iter().all(|&x| x == 0)
                    && !a.is_loopback()
                    && !a.is_unspecified()
                    && forbidden_ip(IpAddr::V4(std::net::Ipv4Addr::new(
                        (s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8))))
        }
    }
}

pub(crate) fn valid_url(raw: &str) -> Option<url::Url> {
    let u = url::Url::parse(raw).ok()?;
    let secure = u.scheme() == "https" || (u.scheme() == "http" && loopback_allowed());
    (secure && u.host_str().is_some() && u.username().is_empty() && u.password().is_none()).then_some(u)
}

/// One step of fetching a web page for a preview.
pub(crate) enum Page {
    Redirect(String),
    Html(Vec<u8>),
}

/// GETs a public page with the webhook protections; a redirect comes back to be checked again.
pub(crate) async fn fetch_page(raw: &str, max: usize) -> Result<Page, String> {
    let u = valid_url(raw).ok_or("Use an https address.")?;
    let host = u.host_str().ok_or(en("That address has no host."))?.to_owned();
    let port = u.port_or_known_default().unwrap_or(443);
    let addr = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|e| e.to_string())?
        .find(|a| loopback_allowed() || !forbidden_ip(a.ip()))
        .ok_or(en("That address is not allowed."))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .resolve(&host, addr)
        .user_agent("boring-forum link preview")
        .build()
        .map_err(|e| e.to_string())?;
    let mut res = client.get(u).send().await.map_err(|e| e.without_url().to_string())?;
    if res.status().is_redirection() {
        let to = res.headers().get("location").and_then(|l| l.to_str().ok()).ok_or(en("No redirect target."))?;
        return Ok(Page::Redirect(to.to_owned()));
    }
    let html = res.headers().get("content-type").and_then(|c| c.to_str().ok()).is_some_and(|c| c.contains("html"));
    if !res.status().is_success() || !html {
        return Err(en("Not a web page.").into());
    }
    let mut out = Vec::new();
    while let Some(chunk) = res.chunk().await.map_err(|e| e.without_url().to_string())? {
        out.extend_from_slice(&chunk);
        // The head is all a preview needs, so a long page is simply cut off.
        if out.len() > max {
            break;
        }
    }
    Ok(Page::Html(out))
}

/// Downloads from a public https address, with the webhook protections and a size cap.
pub(crate) async fn fetch_public(raw: &str, max: usize) -> Result<Vec<u8>, String> {
    let mut url = raw.to_owned();
    for _ in 0..5 {
        match fetch_hop(&url, max).await? {
            Ok(bytes) => return Ok(bytes),
            Err(next) => url = next,
        }
    }
    Err(en("That address redirects too many times.").into())
}

/// One request: the body, or where it redirects to. Every hop goes through the address checks.
async fn fetch_hop(raw: &str, max: usize) -> Result<Result<Vec<u8>, String>, String> {
    let u = valid_url(raw).ok_or("Use an https address.")?;
    let base = u.clone();
    let host = u.host_str().ok_or(en("That address has no host."))?.to_owned();
    let port = u.port_or_known_default().unwrap_or(443);
    let addr = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|e| e.to_string())?
        .find(|a| loopback_allowed() || !forbidden_ip(a.ip()))
        .ok_or(en("That address is not allowed."))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .resolve(&host, addr)
        .user_agent("boring-forum")
        .build()
        .map_err(|e| e.to_string())?;
    let mut res = client.get(u).send().await.map_err(|e| e.without_url().to_string())?;
    if res.status().is_redirection() {
        let next = res
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|l| l.to_str().ok())
            .and_then(|l| base.join(l).ok())
            .ok_or(en("That address redirects nowhere."))?;
        return Ok(Err(next.to_string()));
    }
    if !res.status().is_success() {
        return Err(format!("The server answered {}.", res.status().as_u16()));
    }
    let mut out = Vec::new();
    while let Some(chunk) = res.chunk().await.map_err(|e| e.without_url().to_string())? {
        out.extend_from_slice(&chunk);
        if out.len() > max {
            return Err(en("That file is too large.").into());
        }
    }
    Ok(Ok(out))
}

/// Whether a member-supplied address could be posted to later: https (or loopback in tests), no
/// credentials. The address is resolved and checked again when used.
pub(crate) fn acceptable_target(raw: &str) -> bool {
    valid_url(raw).is_some()
}

/// POSTs bytes with the same guards as webhooks: resolved once, internal addresses refused, the
/// checked address pinned, and no redirects. Returns the status.
pub(crate) async fn post_public(raw: &str, headers: &[(&str, &str)], body: Vec<u8>) -> Result<u16, String> {
    let u = valid_url(raw).ok_or("Use an https address.")?;
    let host = u.host_str().ok_or(en("That address has no host."))?.to_owned();
    let port = u.port_or_known_default().unwrap_or(443);
    let addr = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|e| e.to_string())?
        .find(|a| loopback_allowed() || !forbidden_ip(a.ip()))
        .ok_or(en("That address is not allowed."))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .resolve(&host, addr)
        .user_agent("boring-forum")
        .build()
        .map_err(|e| e.to_string())?;
    let mut req = client.post(u).body(body);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let res = req.send().await.map_err(|e| e.without_url().to_string())?;
    Ok(res.status().as_u16())
}

/// A GET or POST with the webhook protections (federation): https only, the address
/// resolved once and checked, then pinned; no redirects; the body capped at `max` bytes.
/// Returns (status, body).
pub(crate) async fn request_public(
    method: reqwest::Method,
    raw: &str,
    headers: &[(String, String)],
    body: Option<Vec<u8>>,
    max: usize,
) -> Result<(u16, Vec<u8>), String> {
    send_public(method, raw, headers, body, max, false).await
}

/// Like [`request_public`], but an answer longer than `max` bytes is cut short instead of
/// failing, for callers that only need the status (a delivery that was accepted).
pub(crate) async fn request_public_truncated(
    method: reqwest::Method,
    raw: &str,
    headers: &[(String, String)],
    body: Option<Vec<u8>>,
    max: usize,
) -> Result<(u16, Vec<u8>), String> {
    send_public(method, raw, headers, body, max, true).await
}

async fn send_public(
    method: reqwest::Method,
    raw: &str,
    headers: &[(String, String)],
    body: Option<Vec<u8>>,
    max: usize,
    truncate: bool,
) -> Result<(u16, Vec<u8>), String> {
    let u = valid_url(raw).ok_or("Use an https address.")?;
    let host = u.host_str().ok_or(en("That address has no host."))?.to_owned();
    let port = u.port_or_known_default().unwrap_or(443);
    let addr = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|e| e.to_string())?
        .find(|a| loopback_allowed() || !forbidden_ip(a.ip()))
        .ok_or(en("That address is not allowed."))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .resolve(&host, addr)
        .user_agent(concat!("boring-forum/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;
    let mut req = client.request(method, u);
    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_str());
    }
    for (k, v) in crate::http::observability::trace_headers() {
        req = req.header(k, v);
    }
    if let Some(b) = body {
        req = req.body(b);
    }
    let mut res = req.send().await.map_err(|e| e.without_url().to_string())?;
    let status = res.status().as_u16();
    let mut out = Vec::new();
    while let Some(chunk) = res.chunk().await.map_err(|e| e.without_url().to_string())? {
        out.extend_from_slice(&chunk);
        if out.len() > max {
            if truncate {
                out.truncate(max);
                break;
            }
            return Err(en("The answer is too large.").into());
        }
    }
    Ok((status, out))
}

/// Whether federation may talk to this address at all (https, or loopback in tests).
pub(crate) fn federation_url_ok(raw: &str) -> bool {
    valid_url(raw).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_addresses_are_refused_however_they_are_written() {
        for bad in [
            "198.18.0.1",
            "192.0.0.1",
            "240.0.0.1",
            "224.0.0.1",
            "0.1.2.3",
            "64:ff9b::7f00:1",
            "2002:7f00:1::",
            "ff02::1",
            "2001:db8::1",
            "::ffff:10.0.0.1",
        ] {
            assert!(forbidden_ip(bad.parse().unwrap()), "{bad}");
        }
        assert!(!forbidden_ip("8.8.8.8".parse().unwrap()));
        assert!(!forbidden_ip("2606:4700:4700::1111".parse().unwrap()));
        assert!(!forbidden_ip("64:ff9b::808:808".parse().unwrap()));
    }

    #[test]
    fn internal_addresses_are_forbidden() {
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:10.0.0.1",
        ] {
            assert!(forbidden_ip(ip.parse().unwrap()), "{ip}");
        }
        assert!(!forbidden_ip("93.184.216.34".parse().unwrap()));
        assert!(!forbidden_ip("2606:2800:220:1::1".parse().unwrap()));
    }

    #[test]
    fn webhook_urls_are_checked() {
        assert!(valid_url("https://hooks.example/x").is_some());
        assert!(valid_url("https://user:pw@hooks.example/").is_none());
        assert!(valid_url("file:///etc/passwd").is_none());
    }
}

#[cfg(test)]
mod ssrf_tests;
