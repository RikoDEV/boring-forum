//! Inbound HTTP-signature checks: what `inbox::verify` relies on, without a database.

use super::*;
use crate::federation::inbox::{date_fresh, digest_matches, host_matches};

const KEY_ID: &str = "https://remote.example/u/a#main-key";

struct Fixture {
    key: RsaPrivateKey,
    pem: String,
    body: Vec<u8>,
    headers: HeaderMap,
}

fn fixture() -> Fixture {
    let key = RsaPrivateKey::new(&mut rand::rngs::SysRng, 1024).unwrap();
    let pem = RsaPublicKey::from(&key).to_public_key_pem(LineEnding::LF).unwrap();
    let body = br#"{"type":"Follow","actor":"https://remote.example/u/a"}"#.to_vec();
    let url = url::Url::parse("https://forum.example/ap/inbox").unwrap();
    let mut headers = HeaderMap::new();
    headers.insert("host", HeaderValue::from_static("forum.example"));
    for (k, v) in sign(&key, KEY_ID, "POST", &url, Some(&body)) {
        headers.insert(axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(), HeaderValue::from_str(&v).unwrap());
    }
    Fixture { key, pem, body, headers }
}

fn uri() -> Uri {
    "/ap/inbox".parse().unwrap()
}

/// The check `inbox::verify` makes after the header checks: signing string against the key.
fn signature_ok(f: &Fixture, pem: &str, headers: &HeaderMap, uri: &Uri) -> bool {
    let Some(p) = parse_signature(headers.get("signature").unwrap().to_str().unwrap()) else { return false };
    let Some(msg) = signing_string(&p, &Method::POST, uri, headers) else { return false };
    let _ = f;
    verify_with(pem, &msg, &p.signature)
}

#[test]
fn a_valid_request_verifies_and_its_digest_and_date_pass() {
    let f = fixture();
    assert!(signature_ok(&f, &f.pem, &f.headers, &uri()));
    assert!(digest_matches(f.headers.get("digest").unwrap().to_str().unwrap(), &f.body));
    assert!(date_fresh(f.headers.get("date").unwrap().to_str().unwrap(), chrono::Utc::now()).is_ok());
    assert!(host_matches(f.headers.get("host").unwrap().to_str().ok(), "forum.example"));
}

#[test]
fn a_tampered_body_fails_the_digest() {
    let f = fixture();
    let digest = f.headers.get("digest").unwrap().to_str().unwrap();
    assert!(!digest_matches(digest, br#"{"type":"Delete","actor":"https://remote.example/u/a"}"#));
    assert!(!digest_matches(digest, b""));
}

#[test]
fn a_swapped_digest_header_breaks_the_signature() {
    // An attacker who changes the body must also change `Digest`; that header is signed.
    let mut f = fixture();
    let evil = b"{\"type\":\"Delete\"}";
    f.headers.insert("digest", HeaderValue::from_str(&digest_header(evil)).unwrap());
    assert!(digest_matches(f.headers.get("digest").unwrap().to_str().unwrap(), evil));
    assert!(!signature_ok(&f, &f.pem, &f.headers, &uri()));
}

#[test]
fn a_changed_date_host_or_content_type_breaks_the_signature() {
    for (name, value) in
        [("date", "Mon, 09 Nov 2026 12:00:00 GMT"), ("host", "other.example"), ("content-type", "text/plain")]
    {
        let mut f = fixture();
        f.headers.insert(name, HeaderValue::from_static(value));
        assert!(!signature_ok(&f, &f.pem, &f.headers, &uri()), "{name}");
    }
}

#[test]
fn a_stale_or_future_date_is_refused() {
    let now = chrono::Utc::now();
    let fmt = |d: chrono::Duration| (now + d).format("%a, %d %b %Y %H:%M:%S GMT").to_string();
    assert!(date_fresh(&fmt(chrono::Duration::seconds(-290)), now).is_ok());
    assert!(date_fresh(&fmt(chrono::Duration::seconds(-310)), now).is_err());
    assert!(date_fresh(&fmt(chrono::Duration::seconds(310)), now).is_err());
    assert!(date_fresh(&fmt(chrono::Duration::days(-1)), now).is_err());
    assert!(date_fresh("", now).is_err());
    assert!(date_fresh("2026-11-09T12:00:00Z", now).is_err(), "not an HTTP date");
}

#[test]
fn another_key_or_a_garbage_key_does_not_verify() {
    let f = fixture();
    let other = RsaPrivateKey::new(&mut rand::rngs::SysRng, 1024).unwrap();
    let other_pem = RsaPublicKey::from(&other).to_public_key_pem(LineEnding::LF).unwrap();
    assert!(!signature_ok(&f, &other_pem, &f.headers, &uri()));
    assert!(!signature_ok(&f, "", &f.headers, &uri()));
    assert!(!signature_ok(&f, "-----BEGIN PUBLIC KEY-----\nAAAA\n-----END PUBLIC KEY-----", &f.headers, &uri()));
    assert!(!verify_with(&f.pem, "msg", &[]));
    let _ = &f.key;
}

#[test]
fn the_request_target_is_bound_to_path_query_and_method() {
    let f = fixture();
    let other: Uri = "/ap/users/1/inbox".parse().unwrap();
    assert!(!signature_ok(&f, &f.pem, &f.headers, &other));
    let query: Uri = "/ap/inbox?x=1".parse().unwrap();
    assert!(!signature_ok(&f, &f.pem, &f.headers, &query));
    let p = parse_signature(f.headers.get("signature").unwrap().to_str().unwrap()).unwrap();
    let get = signing_string(&p, &Method::GET, &uri(), &f.headers).unwrap();
    assert!(!verify_with(&f.pem, &get, &p.signature), "a signed POST cannot be replayed as GET");
}

#[test]
fn a_signed_header_that_is_missing_yields_no_signing_string() {
    let mut f = fixture();
    f.headers.remove("digest");
    let p = parse_signature(f.headers.get("signature").unwrap().to_str().unwrap()).unwrap();
    assert!(p.headers.iter().any(|h| h == "digest"));
    assert!(signing_string(&p, &Method::POST, &uri(), &f.headers).is_none());
    assert!(!signature_ok(&f, &f.pem, &f.headers, &uri()));
}

#[test]
fn the_signature_header_parser_rejects_incomplete_or_broken_headers() {
    assert!(parse_signature("").is_none());
    assert!(parse_signature(r#"headers="date",signature="AAAA""#).is_none(), "no keyId");
    assert!(parse_signature(r#"keyId="k",headers="date""#).is_none(), "no signature");
    assert!(parse_signature(r#"keyId="k",signature="not base64!!""#).is_none());
    assert!(parse_signature("garbage").is_none());
    let p = parse_signature(r#"keyId="k",headers="(request-target) Host DATE",signature="AAAA""#).unwrap();
    assert_eq!(p.headers, ["(request-target)", "host", "date"], "names are lowercased");
    let d = parse_signature(r#"keyId="k",signature="AAAA""#).unwrap();
    assert_eq!(d.headers, ["date"], "default is only the date");
}

#[test]
fn what_inbox_requires_signed_is_checkable_from_the_parsed_headers() {
    // `inbox::verify` demands these four be in the signed list; a signature that leaves one
    // out (say, no digest) must be visibly short of them.
    let f = fixture();
    let p = parse_signature(f.headers.get("signature").unwrap().to_str().unwrap()).unwrap();
    for required in ["(request-target)", "host", "date", "digest"] {
        assert!(p.headers.iter().any(|h| h == required), "{required}");
    }
    let url = url::Url::parse("https://forum.example/ap/inbox").unwrap();
    let bodiless = sign(&f.key, KEY_ID, "POST", &url, None);
    let sig = &bodiless.iter().find(|(k, _)| k == "signature").unwrap().1;
    let p = parse_signature(sig).unwrap();
    assert!(!p.headers.iter().any(|h| h == "digest"));
}
