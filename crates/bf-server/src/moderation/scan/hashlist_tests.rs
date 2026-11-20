//! Known-image hash list: entry parsing and the distance rule.

use super::*;

#[test]
fn hash_entries_must_be_exactly_64_hex_characters() {
    let good = "ab".repeat(32);
    assert_eq!(parse_hex32(&good), Some([0xab; 32]));
    assert_eq!(parse_hex32(&"AB".repeat(32)), Some([0xab; 32]), "case does not matter");
    assert!(parse_hex32(&good[..62]).is_none());
    assert!(parse_hex32(&format!("{good}00")).is_none());
    assert!(parse_hex32(&"zz".repeat(32)).is_none());
    assert!(parse_hex32("").is_none());
    assert!(parse_hex32(&"é".repeat(32)).is_none(), "64 bytes of multibyte text is not hex");
    assert!(parse_hex32(&format!("+{}", "a".repeat(63))).is_none(), "a sign is not a hex digit");
}

#[test]
fn hamming_distance_counts_differing_bits() {
    let a = [0u8; 32];
    let mut b = [0u8; 32];
    assert_eq!(hamming(&a, &b), 0);
    b[0] = 0b1011;
    b[31] = 0xff;
    assert_eq!(hamming(&a, &b), 3 + 8);
    assert_eq!(hamming(&[0xff; 32], &a), 256);
    assert_eq!(hamming(&a, &b), hamming(&b, &a));
}

#[test]
fn the_threshold_is_the_published_one_and_inclusive() {
    assert_eq!(PDQ_THRESHOLD, 31);
    let a = [0u8; 32];
    let mut at = [0u8; 32];
    at[..3].fill(0xff); // 24 bits
    at[3] = 0x7f; // +7 = 31
    assert_eq!(hamming(&a, &at), 31);
    assert!(hamming(&a, &at) <= PDQ_THRESHOLD);
    at[3] = 0xff; // 32
    assert!(hamming(&a, &at) > PDQ_THRESHOLD);
}
