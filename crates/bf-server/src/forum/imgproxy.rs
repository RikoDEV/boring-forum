//! imgproxy (<https://imgproxy.net>) for pictures from other sites: posts and link previews show
//! them through the admin's own imgproxy instead of from the third party's server, so a reader's
//! address and browser never reach it, the picture is resized and recompressed, and a site that
//! takes its picture away does not leave a hole in an old post.
//!
//! The address is imgproxy's signed form, `{base}/{signature}/{options}/{base64url(source)}[.ext]`,
//! with the signature an HMAC-SHA256 of the salt and the path under the key (both hex in the
//! settings, as imgproxy's own configuration has them). Posts keep the original address in their
//! stored text; it is swapped in the page on the way out, outside the page cache, so changing the
//! settings (or switching imgproxy off) applies at once to every post.
//!
//! Left alone: this site's own pictures, GIF providers' media (the providers' terms ask that it
//! loads straight from them, and imgproxy flattens animations unless told otherwise), and GIFs in
//! general unless `imgproxy_gifs` is on.

use crate::state::AppState;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::Response;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bf_core::settings::Settings;
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};
use std::borrow::Cow;

/// Hosts of GIF providers, never proxied (see the module notes).
const UNTOUCHED: [&str; 2] = ["klipy.com", "giphy.com"];

pub(crate) struct Config {
    base: String,
    /// The host imgproxy answers on, so its own addresses are not proxied again.
    host: String,
    own_host: String,
    key: Vec<u8>,
    salt: Vec<u8>,
    width: u32,
    format: &'static str,
    gifs: bool,
    /// Tells apart two configurations, for validators.
    fingerprint: String,
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.is_empty() || !s.len().is_multiple_of(2) || s.len() > 512 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

/// A host that is not on the public internet: an address literal in a private, loopback,
/// link-local, unique-local, shared or reserved range, `localhost`, or a name with no dot or in an
/// internal-only zone. (A public name that resolves to a private address is imgproxy's to refuse:
/// keep its private/loopback/link-local source options off.)
fn internal_host(u: &url::Url) -> bool {
    use std::net::{Ipv4Addr, Ipv6Addr};
    fn v4(a: Ipv4Addr) -> bool {
        let o = a.octets();
        a.is_loopback()
            || a.is_private()
            || a.is_link_local()
            || a.is_unspecified()
            || a.is_broadcast()
            || a.is_multicast()
            || o[0] == 0
            || (o[0] == 100 && (64..128).contains(&o[1]))
            || (o[0] == 192 && o[1] == 0 && o[2] == 0)
            || (o[0] == 198 && (18..20).contains(&o[1]))
            || o[0] >= 240
    }
    fn v6(a: Ipv6Addr) -> bool {
        let seg = a.segments();
        a.is_loopback()
            || a.is_unspecified()
            || a.is_multicast()
            || (seg[0] & 0xfe00) == 0xfc00
            || (seg[0] & 0xffc0) == 0xfe80
            || a.to_ipv4_mapped().is_some_and(v4)
            || (seg[0] == 0x64 && seg[1] == 0xff9b)
            || seg[0] == 0x2002
    }
    match u.host() {
        Some(url::Host::Ipv4(a)) => v4(a),
        Some(url::Host::Ipv6(a)) => v6(a),
        Some(url::Host::Domain(d)) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            !d.contains('.')
                || ["localhost", "local", "internal", "intranet", "lan", "home", "corp", "localdomain", "home.arpa"]
                    .iter()
                    .any(|z| d == *z || d.ends_with(&format!(".{z}")))
        }
        None => true,
    }
}

fn host_of(url: &str) -> Option<String> {
    url::Url::parse(url).ok()?.host_str().map(str::to_ascii_lowercase)
}

impl Config {
    /// None while imgproxy is off or its settings do not make a working address.
    pub(crate) fn from_settings(s: &Settings, site_url: &str) -> Option<Config> {
        if !s.bool("imgproxy_enabled") {
            return None;
        }
        let url = url::Url::parse(s.str("imgproxy_url").trim()).ok()?;
        if !matches!(url.scheme(), "http" | "https") || url.query().is_some() || url.fragment().is_some() {
            return None;
        }
        let host = url.host_str()?.to_ascii_lowercase();
        let base = url.as_str().trim_end_matches('/').to_owned();
        let (key, salt) = (unhex(s.str("imgproxy_key"))?, unhex(s.str("imgproxy_salt"))?);
        let format = match s.str("imgproxy_format") {
            "webp" => "webp",
            "avif" => "avif",
            "jpeg" => "jpg",
            "png" => "png",
            _ => "",
        };
        let width = u32::try_from(s.int("imgproxy_width")).unwrap_or(1280).clamp(100, 4000);
        let gifs = s.bool("imgproxy_gifs");
        let fingerprint: String = Sha256::digest(format!(
            "{base}|{}|{}|{width}|{format}|{gifs}",
            s.str("imgproxy_key"),
            s.str("imgproxy_salt")
        ))
        .iter()
        .take(4)
        .map(|b| format!("{b:02x}"))
        .collect();
        Some(Config {
            base,
            host,
            own_host: host_of(site_url).unwrap_or_default(),
            key,
            salt,
            width,
            format,
            gifs,
            fingerprint,
        })
    }

    /// The origin pictures are sent from, for the page's policy.
    pub(crate) fn origin(&self) -> String {
        url::Url::parse(&self.base).map(|u| u.origin().ascii_serialization()).unwrap_or_default()
    }

    /// Whether a picture's address should go through imgproxy.
    fn wanted(&self, src: &str) -> bool {
        let Ok(u) = url::Url::parse(src) else { return false };
        if !matches!(u.scheme(), "http" | "https") {
            return false;
        }
        let Some(host) = u.host_str().map(str::to_ascii_lowercase) else { return false };
        // The site signs this address, so imgproxy would fetch whatever it names: never an
        // internal one (loopback, private, link-local such as a cloud's metadata service).
        if internal_host(&u) {
            return false;
        }
        if host == self.host
            || host == self.own_host
            || UNTOUCHED.iter().any(|h| host == *h || host.ends_with(&format!(".{h}")))
        {
            return false;
        }
        self.gifs || !u.path().to_ascii_lowercase().ends_with(".gif")
    }

    /// The signed imgproxy address of `src`, at most `width` wide.
    pub(crate) fn url(&self, src: &str, width: u32) -> Option<String> {
        if !self.wanted(src) {
            return None;
        }
        let encoded = URL_SAFE_NO_PAD.encode(src);
        let ext = if self.format.is_empty() { String::new() } else { format!(".{}", self.format) };
        let path = format!("/rs:fit:{}:0:0/{encoded}{ext}", width.min(self.width));
        Some(format!("{}/{}{path}", self.base, self.sign(&path)?))
    }

    /// imgproxy's signature of a path: HMAC-SHA256 of the salt and the path under the key.
    fn sign(&self, path: &str) -> Option<String> {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.key).ok()?;
        mac.update(&self.salt);
        mac.update(path.as_bytes());
        Some(URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
    }
}

/// `text` with every third-party `<img src>` pointing at imgproxy. It reads pages and JSON alike:
/// in JSON the quotes around an attribute are escaped, and the text is left as it was written.
pub(crate) fn rewrite<'a>(text: &'a str, cfg: &Config) -> Cow<'a, str> {
    if !text.contains("<img") {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + 256);
    let mut rest = text;
    let mut changed = false;
    while let Some(at) = rest.find("<img") {
        let tag_end = rest[at..].find('>').map_or(rest.len(), |e| at + e);
        let tag = &rest[at..tag_end];
        out.push_str(&rest[..at]);
        match swap(tag, cfg) {
            Some(new) => {
                out.push_str(&new);
                changed = true;
            }
            None => out.push_str(tag),
        }
        rest = &rest[tag_end..];
    }
    out.push_str(rest);
    if changed { Cow::Owned(out) } else { Cow::Borrowed(text) }
}

/// The tag with its `src` swapped, or None when it has none worth swapping.
fn swap(tag: &str, cfg: &Config) -> Option<String> {
    let at = tag.find(" src=")? + " src=".len();
    let (quote_len, quote) = if tag[at..].starts_with("\\\"") {
        (2, "\\\"")
    } else if tag[at..].starts_with('"') {
        (1, "\"")
    } else {
        return None;
    };
    let start = at + quote_len;
    let end = start + tag[start..].find(quote)?;
    let src = tag[start..end].replace("&amp;", "&");
    let new = cfg.url(&src, cfg.width)?;
    Some(format!("{}{new}{}", &tag[..start], &tag[end..]))
}

/// Answers that carry what a member typed, as they typed it (an edit form's source, an export):
/// swapping an address in them would be saved back in place of the original.
fn carries_source(path: &str, json: &str) -> bool {
    path.starts_with("/api/v1/admin")
        || path.contains("/export")
        || ["\"raw\":", "\"markdown\":", "\"source\":", "\"bio\":", "\"signature\":"].iter().any(|k| json.contains(k))
}

fn signable(res: &Response) -> bool {
    res.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|t| {
        t.starts_with("text/html")
            || t.starts_with("application/json")
            || t.starts_with("application/rss+xml")
            || t.starts_with("application/atom+xml")
    })
}

/// Outside the page cache: what is sent has its third-party pictures swapped for imgproxy's.
pub(crate) async fn layer(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let Some(cfg) = Config::from_settings(&state.settings(), &state.config.base_url) else {
        return next.run(req).await;
    };
    let path = req.uri().path().to_owned();
    let suffix = format!("-p{}", cfg.fingerprint);
    if let Some(v) = req.headers().get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()).map(str::to_owned) {
        req.headers_mut().remove(header::IF_NONE_MATCH);
        if let Some(own) =
            super::upload_links::strip_tag_suffix(&v, &suffix).and_then(|t| HeaderValue::from_str(&t).ok())
        {
            req.headers_mut().insert(header::IF_NONE_MATCH, own);
        }
    }
    let res = next.run(req).await;
    if res.status() != StatusCode::OK || !signable(&res) {
        return res;
    }
    let (mut parts, body) = res.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, 32 * 1024 * 1024).await else {
        return Response::builder().status(500).body(axum::body::Body::empty()).unwrap_or_default();
    };
    let is_json = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|t| t.starts_with("application/json"));
    let body = match std::str::from_utf8(&bytes) {
        Ok(text) if is_json && carries_source(&path, text) => bytes,
        Ok(text) => match rewrite(text, &cfg) {
            Cow::Owned(swapped) => axum::body::Bytes::from(swapped),
            Cow::Borrowed(_) => bytes,
        },
        Err(_) => bytes,
    };
    // A page keeps its validator only for as long as the settings that shaped it.
    if let Some(tag) = parts
        .headers
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(|t| super::upload_links::tag_with_suffix(t, &suffix))
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

    fn cfg(format: &'static str, gifs: bool) -> Config {
        Config {
            base: "https://img.example.com".into(),
            host: "img.example.com".into(),
            own_host: "forum.example".into(),
            key: unhex("736563726574").unwrap(),
            salt: unhex("68656c6c6f").unwrap(),
            width: 1280,
            format,
            gifs,
            fingerprint: "abcd1234".into(),
        }
    }

    #[test]
    fn internal_addresses_are_never_signed() {
        let c = cfg("", false);
        for bad in [
            "http://169.254.169.254/latest/meta-data/",
            "http://127.0.0.1/x.png",
            "http://10.0.0.5/x.png",
            "http://192.168.1.1/x.png",
            "http://[::1]/x.png",
            "http://[::ffff:127.0.0.1]/x.png",
            "http://2130706433/x.png",
            "http://localhost/x.png",
            "http://db/x.png",
            "http://printer.local/x.png",
        ] {
            assert!(c.url(bad, 800).is_none(), "{bad}");
        }
        assert!(c.url("https://example.com/x.png", 800).is_some());
    }

    #[test]
    fn an_address_is_imgproxys_signed_form() {
        let c = cfg("", false);
        let u = c.url("http://example.com/lorem/ipsum.jpg", 800).unwrap();
        let (sig, path) = u.strip_prefix("https://img.example.com/").unwrap().split_once('/').unwrap();
        assert_eq!(path, "rs:fit:800:0:0/aHR0cDovL2V4YW1wbGUuY29tL2xvcmVtL2lwc3VtLmpwZw");
        let mut mac = Hmac::<Sha256>::new_from_slice(b"secret").unwrap();
        mac.update(b"hello");
        mac.update(format!("/{path}").as_bytes());
        assert_eq!(sig, URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()));
        // A width above the site's limit is brought down to it; a format adds an extension.
        assert!(cfg("webp", false).url("https://a.example/x.png", 99_999).unwrap().contains("/rs:fit:1280:0:0/"));
        assert!(cfg("webp", false).url("https://a.example/x.png", 600).unwrap().ends_with(".webp"));
    }

    #[test]
    fn the_signature_is_the_one_imgproxys_documentation_gives() {
        // https://docs.imgproxy.net/usage/signing_url: key 736563726574, salt 68656c6c6f.
        let path = "/rs:fill:300:400:0/g:sm/aHR0cDovL2V4YW1w/bGUuY29tL2ltYWdl/cy9jdXJpb3NpdHku/anBn.png";
        assert_eq!(cfg("", false).sign(path).unwrap(), "oKfUtW34Dvo2BGQehJFR4Nr0_rIjOtdtzJ3QFsUcXH8");
    }

    #[test]
    fn only_other_sites_pictures_go_through_it() {
        let c = cfg("", false);
        assert!(c.url("https://forum.example/uploads/a.png", 600).is_none(), "ours");
        assert!(c.url("https://img.example.com/x.png", 600).is_none(), "already imgproxy's");
        assert!(c.url("/uploads/a.png", 600).is_none());
        assert!(c.url("data:image/png;base64,AAAA", 600).is_none());
        assert!(c.url("javascript:alert(1)", 600).is_none());
        // GIF providers' media loads straight from them; other GIFs only when asked.
        assert!(c.url("https://static.klipy.com/a/b.webp", 600).is_none());
        assert!(c.url("https://media3.giphy.com/media/x/giphy.webp", 600).is_none());
        assert!(c.url("https://a.example/funny.GIF", 600).is_none());
        assert!(cfg("", true).url("https://a.example/funny.gif", 600).is_some());
    }

    #[test]
    fn pages_and_json_get_their_pictures_swapped() {
        let c = cfg("", false);
        let page = r#"<p>hi</p><img loading="lazy" src="https://a.example/p.png?x=1&amp;y=2" alt="a"><img src="/uploads/x.png"><img src="https://static.klipy.com/g.webp">"#;
        let out = rewrite(page, &c);
        assert!(out.contains(r#"src="https://img.example.com/"#) && !out.contains("a.example/p.png"));
        assert!(out.contains(r#"src="/uploads/x.png""#) && out.contains("static.klipy.com/g.webp"));
        let mut expected = String::new();
        // `&amp;` is read as `&` before it is encoded.
        expected.push_str(&URL_SAFE_NO_PAD.encode("https://a.example/p.png?x=1&y=2"));
        assert!(out.contains(&expected));
        let json = r#"{"cooked":"<img loading=\"lazy\" src=\"https://a.example/p.png\" alt=\"\">"}"#;
        let swapped = rewrite(json, &c);
        assert!(swapped.contains(r#"src=\"https://img.example.com/"#), "{swapped}");
        assert!(serde_json::from_str::<serde_json::Value>(&swapped).is_ok(), "still JSON");
        assert!(matches!(rewrite("<p>no pictures</p>", &c), Cow::Borrowed(_)));
    }

    #[test]
    fn settings_make_a_config_only_when_they_are_whole() {
        let mut s = bf_core::settings::Settings::default();
        assert!(Config::from_settings(&s, "https://forum.example").is_none(), "off");
        for (k, v) in [
            ("imgproxy_enabled", serde_json::json!(true)),
            ("imgproxy_url", "https://img.example.com/".into()),
            ("imgproxy_key", "00ff".into()),
        ] {
            s.set(k, v).unwrap();
        }
        assert!(Config::from_settings(&s, "https://forum.example").is_none(), "no salt");
        s.set("imgproxy_salt", "not hex".into()).unwrap();
        assert!(Config::from_settings(&s, "https://forum.example").is_none(), "salt is not hex");
        s.set("imgproxy_salt", "ab12".into()).unwrap();
        let c = Config::from_settings(&s, "https://forum.example").unwrap();
        assert_eq!(c.origin(), "https://img.example.com");
        s.set("imgproxy_url", "javascript:alert(1)".into()).unwrap();
        assert!(Config::from_settings(&s, "https://forum.example").is_none());
    }
}
