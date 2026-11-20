//! CSRF double-submit, constant-time comparison and webhook signing.

use super::*;

#[test]
fn csrf_needs_both_values_equal_and_a_full_length_cookie() {
    let t = random_token();
    assert!(csrf_matches(Some(&t), Some(&t)));
    assert!(!csrf_matches(Some(&t), None));
    assert!(!csrf_matches(None, None));
    assert!(!csrf_matches(Some(&t), Some("")));
    assert!(!csrf_matches(Some(&t), Some(&t.to_uppercase())), "comparison is exact");
    assert!(!csrf_matches(Some(&t), Some(&format!("{t}x"))), "a longer submission");
    assert!(!csrf_matches(Some(&t), Some(&t[..63])), "a prefix");
    // A short or empty cookie must not be matchable by echoing it.
    assert!(!csrf_matches(Some("abc"), Some("abc")));
    assert!(!csrf_matches(Some(""), Some("")));
    let other = random_token();
    assert!(!csrf_matches(Some(&t), Some(&other)));
}

#[test]
fn constant_eq_compares_content_and_length() {
    assert!(constant_eq(b"", b""));
    assert!(constant_eq(b"secret", b"secret"));
    assert!(!constant_eq(b"secret", b"secreT"));
    assert!(!constant_eq(b"secret", b"secre"));
    assert!(!constant_eq(b"", b"x"));
}

#[test]
fn webhook_signatures_are_refused_for_every_kind_of_damage() {
    let (secret, body, now) = ("whsec", &b"{\"event\":\"x\"}"[..], 1_800_000_000_i64);
    let good = webhook_signature(secret, now, body);
    assert!(good.starts_with(&format!("t={now},sha256=")));
    assert!(verify_webhook_signature(secret, &good, body, now + 299, 300));
    assert!(verify_webhook_signature(secret, &good, body, now - 299, 300));
    assert!(!verify_webhook_signature(secret, &good, body, now + 301, 300), "stale");
    assert!(!verify_webhook_signature(secret, &good, body, now - 301, 300), "from the future");
    assert!(!verify_webhook_signature(secret, &good, b"{\"event\":\"y\"}", now, 300), "body");
    assert!(!verify_webhook_signature("other", &good, body, now, 300), "key");
    // Moving the timestamp in the header without re-signing breaks the MAC.
    let moved = good.replacen(&format!("t={now}"), &format!("t={}", now + 10), 1);
    assert!(!verify_webhook_signature(secret, &moved, body, now + 10, 300));
    for bad in ["", "t=1", "sha256=ab", "t=x,sha256=ab", &format!("t={now},sha256="), &format!("t={now},sha256=zz")] {
        assert!(!verify_webhook_signature(secret, bad, body, now, 300), "{bad:?}");
    }
    assert_ne!(webhook_signature("a", now, body), webhook_signature("b", now, body));
    assert_eq!(webhook_signature(secret, now, body), good, "deterministic");
}

#[test]
fn hmac_matches_a_known_vector() {
    // RFC 4231 test case 2.
    assert_eq!(
        hmac_sha256_hex("Jefe", b"what do ya want for nothing?"),
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );
}
