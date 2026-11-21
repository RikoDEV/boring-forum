//! Expiring links to uploaded files, so a file in a public post cannot be hotlinked for good.
//!
//! A guest who reads a page, a feed or the JSON API gets every protected file link with a short
//! lived signature added: `/uploads/<key>?x=<expires>.<signature>`. The file is then served to
//! someone who is not signed in only with a valid signature (or by following a link in the
//! browser). Someone who copies the address to another site finds it dead once the window is
//! over; a page that embeds it from a browser is refused at once (`upload_access`).
//!
//! The stored text never holds a signature: links are signed on the way out, outside the page
//! cache, so cached pages stay shared. A signed-in member needs none (their session decides), and
//! is not rewritten. A window is `upload_link_hours`; an address is stable within one, so
//! browsers can still cache the file, and it works for between one and two windows.
//!
//! The key comes from `BF_SECRET_KEY`: set it where several nodes
//! serve one site, or each process signs with a key of its own and others cannot check it.

use crate::state::AppState;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, header};
use axum::middleware::Next;
use axum::response::Response;
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::sync::LazyLock;

static KEY: LazyLock<Vec<u8>> = LazyLock::new(|| {
    if let Ok(v) = std::env::var("BF_SECRET_KEY")
        && !v.trim().is_empty()
    {
        return Sha256::digest(format!("bf-upload-links:{v}")).to_vec();
    }
    Sha256::digest(format!("bf-upload-links:{}", bf_core::auth::random_token())).to_vec()
});

const QUERY: &str = "?x=";

fn mac(sha: &str, expires: i64) -> Option<Hmac<Sha256>> {
    let mut m = Hmac::<Sha256>::new_from_slice(&KEY).ok()?;
    m.update(format!("upload:{sha}:{expires}").as_bytes());
    Some(m)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The signature for a file (by its content hash) valid until `expires`.
pub(crate) fn sign(sha: &str, expires: i64) -> String {
    mac(sha, expires).map(|m| hex(&m.finalize().into_bytes()[..16])).unwrap_or_default()
}

/// Does `value` (`<expires>.<signature>`, the `x` parameter) vouch for this file right now?
pub(crate) fn verify(sha: &str, value: &str, now: i64) -> bool {
    let Some((expires, sig)) = value.split_once('.') else { return false };
    let Ok(expires) = expires.parse::<i64>() else { return false };
    if expires <= now || sig.len() != 32 {
        return false;
    }
    let Some(m) = mac(sha, expires) else { return false };
    let want = hex(&m.finalize().into_bytes()[..16]);
    // Compared without stopping at the first difference.
    want.bytes().zip(sig.bytes()).fold(want.len() ^ sig.len(), |d, (a, b)| d | usize::from(a ^ b)) == 0
}

/// When the signatures made now run out: the start of the window after next, so an address is
/// the same all window long and lives for at least one window.
pub(crate) fn expiry(now: i64, window: i64) -> i64 {
    (now / window + 2) * window
}

/// The seconds a signature is valid for, from the site setting.
pub(crate) fn window(state: &AppState) -> i64 {
    state.settings().int("upload_link_hours").clamp(1, 168) * 3600
}

/// The content hash and extension of a file address starting right after `/uploads/`, and where
/// the address ends.
fn parse(rest: &str) -> Option<(&str, &str, usize)> {
    let sha = rest.get(..64).filter(|s| s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()))?;
    let tail = &rest[64..];
    let len =
        tail.bytes().take_while(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'-')).count();
    let name = &tail[..len];
    // `.png`, or a thumbnail `-640.webp`: the extension is the last part.
    let ext = name.rsplit('.').next().filter(|e| !e.is_empty() && name.contains('.'))?;
    Some((sha, ext, 64 + len))
}

const PICTURES: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "avif"];

/// Whether the site's hotlink rule covers a file of this kind: files always (unless `off`),
/// pictures only under `all`.
pub(crate) fn covers(mode: &str, ext: &str) -> bool {
    match mode {
        "off" => false,
        "files" => !PICTURES.contains(&ext),
        // "all", and a value nobody recognises: the strictest mode.
        _ => true,
    }
}

/// `text` with a signature on every file address the rule covers (and none yet).
pub(crate) fn rewrite<'a>(text: &'a str, mode: &str, now: i64, window: i64) -> Cow<'a, str> {
    if !text.contains("/uploads/") {
        return Cow::Borrowed(text);
    }
    let expires = expiry(now, window);
    let mut out = String::with_capacity(text.len() + 256);
    let mut rest = text;
    let mut changed = false;
    while let Some(at) = rest.find("/uploads/") {
        let after = at + "/uploads/".len();
        out.push_str(&rest[..after]);
        rest = &rest[after..];
        if let Some((sha, ext, len)) = parse(rest)
            && covers(mode, ext)
            && !rest[len..].starts_with('?')
        {
            out.push_str(&rest[..len]);
            out.push_str(QUERY);
            out.push_str(&format!("{expires}.{}", sign(sha, expires)));
            rest = &rest[len..];
            changed = true;
        }
    }
    out.push_str(rest);
    if changed { Cow::Owned(out) } else { Cow::Borrowed(text) }
}

/// Is this a guest asking for a page, feed or API answer? Only those get signed links.
fn is_guest_get(req: &Request) -> bool {
    req.method() == Method::GET
        && !req.headers().contains_key(header::AUTHORIZATION)
        && !req
            .headers()
            .get_all(header::COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .any(|c| c.split(';').any(|kv| kv.trim_start().starts_with(bf_web::microcache::SESSION_COOKIE)))
}

fn signable(res: &Response) -> bool {
    res.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|t| {
        t.starts_with("text/html")
            || t.starts_with("application/json")
            || t.starts_with("application/rss+xml")
            || t.starts_with("application/atom+xml")
            || t.starts_with("text/xml")
            || t.starts_with("application/xml")
    })
}

/// A page's validator, tied to the signing window: a page revisited in a later window is sent
/// again with fresh signatures instead of "not modified" (which would keep dead links).
fn window_tag(tag: &str, bucket: i64) -> String {
    tag_with_suffix(tag, &format!("-w{bucket}"))
}

/// A validator with `suffix` added inside its quotes: the same page under other settings is
/// another representation.
pub(crate) fn tag_with_suffix(tag: &str, suffix: &str) -> String {
    let (weak, rest) = tag.strip_prefix("W/").map_or(("", tag), |r| ("W/", r));
    format!("{weak}\"{}{suffix}\"", rest.trim_matches('"'))
}

fn request_tags(value: &str, bucket: i64) -> Option<String> {
    strip_tag_suffix(value, &format!("-w{bucket}"))
}

/// The validators of a request that carry `suffix`, without it; None when none does.
pub(crate) fn strip_tag_suffix(value: &str, suffix: &str) -> Option<String> {
    let kept: Vec<String> = value
        .split(',')
        .map(str::trim)
        .filter_map(|t| {
            let (weak, rest) = t.strip_prefix("W/").map_or(("", t), |r| ("W/", r));
            let inner = rest.trim_matches('"').strip_suffix(suffix)?;
            Some(format!("{weak}\"{inner}\""))
        })
        .collect();
    (!kept.is_empty()).then(|| kept.join(", "))
}

/// Outside the page cache: signs the file links of what a guest is sent.
pub(crate) async fn sign_links(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let mode = state.settings().str("hotlink_protection").to_owned();
    if mode == "off" || !is_guest_get(&req) {
        return next.run(req).await;
    }
    let now = chrono::Utc::now().timestamp();
    let window = window(&state);
    let bucket = now / window;
    if let Some(v) = req.headers().get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()).map(str::to_owned) {
        req.headers_mut().remove(header::IF_NONE_MATCH);
        if let Some(own) = request_tags(&v, bucket).and_then(|t| HeaderValue::from_str(&t).ok()) {
            req.headers_mut().insert(header::IF_NONE_MATCH, own);
        }
    }
    let res = next.run(req).await;
    if res.status() != axum::http::StatusCode::OK || !signable(&res) {
        return res;
    }
    let (mut parts, body) = res.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, 32 * 1024 * 1024).await else {
        return Response::builder().status(500).body(axum::body::Body::empty()).unwrap_or_default();
    };
    let body = match std::str::from_utf8(&bytes) {
        Ok(text) => match rewrite(text, &mode, now, window) {
            Cow::Owned(signed) => axum::body::Bytes::from(signed),
            Cow::Borrowed(_) => bytes,
        },
        Err(_) => bytes,
    };
    if let Some(tag) = parts.headers.get(header::ETAG).and_then(|v| v.to_str().ok()).map(|t| window_tag(t, bucket))
        && let Ok(v) = HeaderValue::from_str(&tag)
    {
        parts.headers.insert(header::ETAG, v);
    }
    parts.headers.insert(header::CONTENT_LENGTH, HeaderValue::from(body.len()));
    Response::from_parts(parts, axum::body::Body::from(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn a_signature_vouches_for_its_file_until_it_runs_out() {
        let exp = 2_000;
        let sig = sign(SHA, exp);
        assert!(verify(SHA, &format!("{exp}.{sig}"), 1_999));
        assert!(!verify(SHA, &format!("{exp}.{sig}"), 2_000), "expired");
        assert!(!verify(SHA, &format!("{}.{sig}", exp + 1), 1_000), "another time");
        let other = "f".repeat(64);
        assert!(!verify(&other, &format!("{exp}.{sig}"), 1_000), "another file");
        assert!(!verify(SHA, &format!("{exp}.{}", "0".repeat(32)), 1_000));
        assert!(!verify(SHA, "nonsense", 1_000));
    }

    #[test]
    fn an_address_is_stable_within_a_window_and_lives_at_least_one() {
        let w = 3600;
        assert_eq!(expiry(10_000, w), expiry(10_001, w));
        assert_eq!(expiry(0, w), 2 * w);
        assert!(expiry(w - 1, w) - (w - 1) >= w);
        assert_ne!(expiry(10_000, w), expiry(10_000 + w, w));
    }

    #[test]
    fn files_get_signed_in_pages_and_pictures_only_when_asked() {
        let page = format!(
            "<a href=\"/uploads/{SHA}.zip\">z</a> <img src=\"/uploads/{SHA}.png#10x10\"> <a href=\"/uploads/{SHA}.pdf?x=1.2\">p</a> and /uploads/short.zip"
        );
        let files = rewrite(&page, "files", 5_000, 3600);
        assert!(files.contains(&format!("/uploads/{SHA}.zip?x=")));
        assert!(files.contains(&format!("/uploads/{SHA}.png#10x10")), "a picture stays alone");
        assert!(files.contains(&format!("{SHA}.pdf?x=1.2\"")), "already signed stays");
        assert!(files.contains("/uploads/short.zip"));
        let all = rewrite(&page, "all", 5_000, 3600);
        assert!(all.contains(&format!("/uploads/{SHA}.png?x=")));
        assert!(all.contains("#10x10"), "the size fragment survives");
        assert!(matches!(rewrite("no files here", "files", 5_000, 3600), Cow::Borrowed(_)));
        assert!(matches!(rewrite(&page, "off", 5_000, 3600), Cow::Borrowed(_)) || !covers("off", "zip"));
    }

    #[test]
    fn a_thumbnail_is_signed_for_its_picture() {
        let t = format!("<img srcset=\"/uploads/{SHA}-640.webp 640w\">");
        let signed = rewrite(&t, "all", 5_000, 3600);
        let value = signed.split("?x=").nth(1).and_then(|r| r.split(' ').next()).unwrap_or("");
        assert!(verify(SHA, value, 5_000), "{value}");
    }

    #[test]
    fn validators_follow_the_window() {
        assert_eq!(window_tag("\"abc\"", 7), "\"abc-w7\"");
        assert_eq!(window_tag("W/\"abc\"", 7), "W/\"abc-w7\"");
        assert_eq!(request_tags("\"abc-w7\"", 7).as_deref(), Some("\"abc\""));
        assert_eq!(request_tags("\"abc-w6\", \"x-w7\"", 7).as_deref(), Some("\"x\""));
        assert_eq!(request_tags("\"abc-w6\"", 7), None);
    }
}
