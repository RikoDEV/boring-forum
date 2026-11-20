//! Key rotation and malformed stored values.

use super::*;

const A: [u8; 32] = [1u8; 32];
const B: [u8; 32] = [2u8; 32];
const C: [u8; 32] = [3u8; 32];

#[test]
fn rotation_walks_a_value_from_the_old_key_to_the_new_one() {
    let under_old = Keyring::from_keys(&[B]).seal("totp-secret");
    // Step 1: new key in BF_SECRET_KEY, the old one in BF_SECRET_KEY_OLD.
    let during = Keyring::from_keys(&[A, B]);
    assert!(during.stale(&under_old));
    let plain = during.open(&under_old).unwrap();
    let resealed = during.seal(&plain);
    assert!(!during.stale(&resealed));
    // Step 2: the old key is dropped; the rewritten value still opens, the old one does not.
    let after = Keyring::from_keys(&[A]);
    assert_eq!(after.open(&resealed).as_deref(), Some("totp-secret"));
    assert_eq!(after.open(&under_old), None);
}

#[test]
fn several_retired_keys_all_stay_readable() {
    let v_b = Keyring::from_keys(&[B]).seal("b");
    let v_c = Keyring::from_keys(&[C]).seal("c");
    let ring = Keyring::from_keys(&[A, B, C]);
    assert_eq!(ring.open(&v_b).as_deref(), Some("b"));
    assert_eq!(ring.open(&v_c).as_deref(), Some("c"));
    assert_eq!(ring.current_id(), Keyring::from_keys(&[A]).current_id());
}

#[test]
fn a_value_swapped_to_another_key_id_does_not_open() {
    // Pointing the id at another key in the ring must fail authentication, not decrypt.
    let sealed = Keyring::from_keys(&[B]).seal("x");
    let parts: Vec<&str> = sealed.split('.').collect();
    let id_c = Keyring::from_keys(&[C]).current_id().to_owned();
    let forged = format!("bf1.{id_c}.{}.{}", parts[2], parts[3]);
    assert_eq!(Keyring::from_keys(&[B, C]).open(&forged), None);
}

#[test]
fn malformed_sealed_values_are_refused_not_returned_as_plain_text() {
    let ring = Keyring::from_keys(&[A]);
    let id = ring.current_id().to_owned();
    for bad in [
        format!("bf1.{id}.AAAA.AAAA"),
        format!("bf1.{id}.!!!.!!!"),
        format!("bf1.{id}.{}.AAAA", B64.encode([0u8; 11])),
        format!("bf1.{id}.{}.AAAA", B64.encode([0u8; 12])),
    ] {
        assert_eq!(ring.open(&bad), None, "{bad}");
    }
    // Not in the sealed shape at all: legacy plain text passes through unchanged.
    for plain in ["bf1", "bf1.", "bf1.x", "bf1.x.y", "bf2.a.b.c", "", "BF1.a.b.c"] {
        assert_eq!(ring.open(plain).as_deref(), Some(plain), "{plain}");
    }
}

#[test]
fn empty_and_unicode_values_round_trip() {
    let ring = Keyring::from_keys(&[A]);
    for v in ["", "zażółć gęślą jaźń", "line\nbreak", &"x".repeat(10_000)] {
        assert_eq!(ring.open(&ring.seal(v)).as_deref(), Some(v));
    }
}

#[test]
fn keys_parse_in_url_safe_and_standard_base64_and_reject_the_rest() {
    let std_b64 = base64::engine::general_purpose::STANDARD.encode([0xfbu8; 32]);
    assert_eq!(parse_key(&std_b64).unwrap(), [0xfb; 32]);
    assert_eq!(parse_key(&format!("  {}\n", B64.encode(A))).unwrap(), A, "whitespace is trimmed");
    assert!(parse_key("").is_err());
    assert!(parse_key(&B64.encode([1u8; 33])).is_err());
}

#[test]
fn a_generated_key_is_usable_as_the_secret_key() {
    let line = generate_key();
    let value = line.trim().strip_prefix("BF_SECRET_KEY=").unwrap();
    let key = parse_key(value).unwrap();
    let ring = Keyring::from_keys(&[key]);
    assert_eq!(ring.open(&ring.seal("ok")).as_deref(), Some("ok"));
    assert_ne!(generate_key(), generate_key());
}
