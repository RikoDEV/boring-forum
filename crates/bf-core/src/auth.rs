//! Authentication primitives: argon2id passwords, opaque session tokens,
//! double-submit CSRF tokens and username rules. Storage lives in bf-db.

use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use argon2::{Algorithm, Argon2, Params, Version};
use rand::{TryRng, rngs::SysRng};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// OWASP-recommended argon2id parameters (19 MiB, 2 passes): safe on a 512 MB host.
#[expect(clippy::expect_used, reason = "the Params are constants inside argon2's accepted ranges")]
fn hasher() -> Argon2<'static> {
    Argon2::new(Algorithm::Argon2id, Version::V0x13, Params::new(19 * 1024, 2, 1, None).expect("valid params"))
}

#[expect(clippy::expect_used, reason = "hashing fails only on invalid Params, and they are constants")]
pub fn hash_password(password: &str) -> String {
    hasher().hash_password(password.as_bytes()).expect("argon2 hashing cannot fail with valid params").to_string()
}

/// Imported forums keep their old hashes until the member's next sign-in, when they
/// are replaced by argon2id. Formats: bcrypt (`$2…`: phpBB 3.1+, IPS 4), phpass (`$H$`/`$P$`:
/// phpBB 3.0), and two shapes the importer writes: `discourse$<iterations>$<salt>$<hex>`
/// (PBKDF2-HMAC-SHA256) and `ips3$<salt>$<hex>` (`md5(md5(salt) . md5(password))`: Invision 3, and MyBB's
/// `password` and `salt` columns, which are the same scheme).
pub fn is_legacy_hash(hash: &str) -> bool {
    hash.starts_with("$2")
        || hash.starts_with("$H$")
        || hash.starts_with("$P$")
        || hash.starts_with("discourse$")
        || hash.starts_with("ips3$")
}

pub fn verify_password(password: &str, phc: &str) -> bool {
    if phc.starts_with("$2") {
        return bcrypt::verify(password, phc).unwrap_or(false);
    }
    if phc.starts_with("$H$") || phc.starts_with("$P$") {
        return phpass(password, phc).is_some_and(|h| h.as_bytes().ct_eq(phc.as_bytes()).into());
    }
    if let Some(rest) = phc.strip_prefix("discourse$") {
        return discourse(password, rest);
    }
    if let Some(rest) = phc.strip_prefix("ips3$") {
        return ips3(password, rest);
    }
    PasswordHash::new(phc).is_ok_and(|h| hasher().verify_password(password.as_bytes(), &h).is_ok())
}

const ITOA64: &[u8; 64] = b"./0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// The portable PHP hash (phpass) that phpBB 3.0 and WordPress use: `$H$`, one character for
/// log2 of the rounds, an 8-character salt and 22 characters of MD5 chain.
fn phpass(password: &str, setting: &str) -> Option<String> {
    use md5::{Digest as _, Md5};
    let b = setting.as_bytes();
    if b.len() < 12 {
        return None;
    }
    let log = ITOA64.iter().position(|c| *c == b[3])?;
    if !(7..=30).contains(&log) {
        return None;
    }
    let salt = &b[4..12];
    let mut hash = Md5::digest([salt, password.as_bytes()].concat()).to_vec();
    for _ in 0..(1u64 << log) {
        hash = Md5::digest([hash.as_slice(), password.as_bytes()].concat()).to_vec();
    }
    let mut out = String::from_utf8_lossy(&b[..12]).into_owned();
    let mut i = 0;
    while i < 16 {
        let mut value = u32::from(hash[i]);
        i += 1;
        out.push(ITOA64[(value & 0x3f) as usize] as char);
        if i < 16 {
            value |= u32::from(hash[i]) << 8;
        }
        out.push(ITOA64[((value >> 6) & 0x3f) as usize] as char);
        if i >= 16 {
            break;
        }
        i += 1;
        if i < 16 {
            value |= u32::from(hash[i]) << 16;
        }
        out.push(ITOA64[((value >> 12) & 0x3f) as usize] as char);
        if i >= 16 {
            break;
        }
        i += 1;
        out.push(ITOA64[((value >> 18) & 0x3f) as usize] as char);
    }
    Some(out)
}

/// `<iterations>$<salt>$<hex>`: Discourse's PBKDF2-HMAC-SHA256 with a 32-byte result.
fn discourse(password: &str, rest: &str) -> bool {
    use hmac::{Hmac, KeyInit, Mac};
    let mut parts = rest.splitn(3, '$');
    let (Some(iters), Some(salt), Some(want)) = (parts.next(), parts.next(), parts.next()) else { return false };
    let Ok(iters) = iters.parse::<u32>() else { return false };
    if iters == 0 || iters > 2_000_000 {
        return false;
    }
    let Ok(base) = Hmac::<Sha256>::new_from_slice(password.as_bytes()) else { return false };
    let mut mac = base.clone();
    mac.update(salt.as_bytes());
    mac.update(&1u32.to_be_bytes());
    let mut u = mac.finalize().into_bytes();
    let mut t = u;
    for _ in 1..iters {
        let mut mac = base.clone();
        mac.update(&u);
        u = mac.finalize().into_bytes();
        for (a, b) in t.iter_mut().zip(u.iter()) {
            *a ^= b;
        }
    }
    let got: String = t.iter().map(|b| format!("{b:02x}")).collect();
    got.as_bytes().ct_eq(want.to_ascii_lowercase().as_bytes()).into()
}

/// `<salt>$<hex>`: Invision 3's `md5(md5(salt) . md5(password))`.
fn ips3(password: &str, rest: &str) -> bool {
    use md5::{Digest as _, Md5};
    let Some((salt, want)) = rest.split_once('$') else { return false };
    let hex = |d: &[u8]| d.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let inner = format!("{}{}", hex(&Md5::digest(salt.as_bytes())), hex(&Md5::digest(password.as_bytes())));
    hex(&Md5::digest(inner.as_bytes())).as_bytes().ct_eq(want.to_ascii_lowercase().as_bytes()).into()
}

pub fn validate_password(password: &str, username: &str) -> Result<(), &'static str> {
    if password.chars().count() < 10 {
        return Err("Password must be at least 10 characters");
    }
    validate_password_rest(password, username)
}

/// The rules after the length, for a site whose minimum is its own setting: the caller checks
/// the length and how many different characters there must be.
pub fn validate_password_rest(password: &str, username: &str) -> Result<(), &'static str> {
    if password.len() > 1024 {
        return Err("Password is too long");
    }
    if password.eq_ignore_ascii_case(username) {
        return Err("Password must not be your username");
    }
    if is_common_password(password) {
        return Err("That password is one of the most common ones and is easy to guess. Choose another.");
    }
    Ok(())
}

/// (ASVS 2.1.7): about 143,000 passwords of 10 or more characters from public breach
/// corpora (SecLists' top 1M and the NCSC top 100k), lowercased and sorted bytewise.
static COMMON_PASSWORDS: &str = include_str!("../data/common-passwords.txt");

static COMMON: std::sync::LazyLock<Vec<&'static str>> =
    std::sync::LazyLock::new(|| COMMON_PASSWORDS.lines().filter(|l| !l.is_empty()).collect());

/// Case-insensitive: "Password123" is as weak as "password123".
pub fn is_common_password(password: &str) -> bool {
    let lower = password.to_lowercase();
    COMMON.binary_search(&lower.as_str()).is_ok()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 256 random bits, hex-encoded. Used for session, CSRF, reset and verification tokens.
#[expect(
    clippy::expect_used,
    reason = "the operating system's random source is always present; without it nothing secure can run"
)]
pub fn random_token() -> String {
    let mut b = [0u8; 32];
    SysRng.try_fill_bytes(&mut b).expect("the system random source is available");
    hex(&b)
}

/// Only the SHA-256 of a token is stored, so a database leak does not leak sessions.
pub fn token_digest(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}

/// Double-submit CSRF check: the form/header value must equal the cookie value.
pub fn csrf_matches(cookie: Option<&str>, submitted: Option<&str>) -> bool {
    match (cookie, submitted) {
        (Some(c), Some(s)) if c.len() == 64 => c.as_bytes().ct_eq(s.as_bytes()).into(),
        _ => false,
    }
}

/// Constant-time equality for secrets compared with what a client sent (tokens, keys).
pub fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && bool::from(a.ct_eq(b))
}

const RESERVED: &[&str] = &[
    "admin",
    "administrator",
    "moderator",
    "staff",
    "system",
    "root",
    "support",
    "api",
    "me",
    "new",
    "null",
    "anonymous",
    "imported",
];

/// Allowed characters, length, reserved names. Returns the lowercased key.
pub fn validate_username(name: &str) -> Result<String, &'static str> {
    if !(3..=20).contains(&name.len()) {
        return Err("Username must be 3–20 characters");
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')) {
        return Err("Username may only contain letters, digits, _ - and .");
    }
    if !name.starts_with(|c: char| c.is_ascii_alphanumeric()) || !name.ends_with(|c: char| c.is_ascii_alphanumeric()) {
        return Err("Username must start and end with a letter or digit");
    }
    let lower = name.to_ascii_lowercase();
    reserved_check(&lower)?;
    Ok(lower)
}

/// Is this skeleton that of a reserved name, or of a name with a reserved prefix?
pub fn is_reserved_skeleton(skeleton: &str) -> bool {
    static EXACT: std::sync::LazyLock<Vec<String>> =
        std::sync::LazyLock::new(|| RESERVED.iter().map(|n| username_skeleton(n)).collect());
    static PREFIXES: std::sync::LazyLock<Vec<String>> =
        std::sync::LazyLock::new(|| ["plugin-", "deleted-", "forum."].iter().map(|n| username_skeleton(n)).collect());
    EXACT.iter().any(|e| e == skeleton) || PREFIXES.iter().any(|p| skeleton.starts_with(p.as_str()))
}

/// `plugin-` and `deleted-` names belong to plugin accounts and removed members; `forum.` is
/// how forums are addressed over ActivityPub.
fn reserved_check(lower: &str) -> Result<(), &'static str> {
    if RESERVED.contains(&lower)
        || lower.starts_with("plugin-")
        || lower.starts_with("deleted-")
        || lower.starts_with("forum.")
    {
        return Err("That username is reserved");
    }
    // A look-alike of a reserved name ("adm1n", "ro0t") is as misleading as the name itself.
    if is_reserved_skeleton(&username_skeleton(lower)) {
        return Err("That username is reserved");
    }
    Ok(())
}

/// A strict, canonical form of an email address, or None when it is not one we will mail: one
/// `@`, an ASCII dot-atom local part (at most 64 characters), a domain of two or more
/// LDH labels, no whitespace or control characters. The domain is lowercased and a trailing dot is
/// dropped; the local part is kept as written, since mailboxes may be case or tag sensitive.
pub fn normalize_email(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > 254 || !raw.is_ascii() || raw.bytes().any(|b| b <= b' ' || b == 0x7f) {
        return None;
    }
    let (local, domain) = raw.split_once('@')?;
    if domain.contains('@') {
        return None;
    }
    let atext = |c: char| c.is_ascii_alphanumeric() || "!#$%&'*+-/=?^_`{|}~".contains(c);
    if local.is_empty()
        || local.len() > 64
        || local.starts_with('.')
        || local.ends_with('.')
        || local.contains("..")
        || !local.chars().all(|c| atext(c) || c == '.')
    {
        return None;
    }
    let domain = domain.strip_suffix('.').unwrap_or(domain).to_ascii_lowercase();
    let labels: Vec<&str> = domain.split('.').collect();
    let label_ok = |l: &&str| {
        (1..=63).contains(&l.len())
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    };
    if labels.len() < 2
        || !labels.iter().all(label_ok)
        || labels.last().is_some_and(|t| t.chars().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    Some(format!("{local}@{domain}"))
}

/// (local part, domain) with the parts that deliver to the same mailbox on the big providers
/// removed: a `+tag`, and for Gmail the dots and the `googlemail.com` spelling. Lowercased. Only for
/// spotting one person registering many accounts; never stored or mailed to.
pub fn email_alias_key(canonical: &str) -> (String, String) {
    let (local, domain) = canonical.rsplit_once('@').unwrap_or((canonical, ""));
    let mut local = local.to_ascii_lowercase();
    let mut domain = domain.to_ascii_lowercase();
    if let Some((base, _)) = local.split_once('+') {
        local = base.to_owned();
    }
    if domain == "gmail.com" || domain == "googlemail.com" {
        domain = "gmail.com".into();
        local.retain(|c| c != '.');
    }
    (local, domain)
}

/// NFKC, so full-width and compatibility forms become their plain letters. Applied
/// before validation and stored as the name.
pub fn normalize_username(name: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    name.trim().nfkc().collect()
}

/// With `unicode` off, exactly `validate_username`. With it on, letters and digits of
/// any one script (UTS #39 "highly restrictive": one script, or Latin with Han/Hiragana/
/// Katakana/Hangul) plus `_ - .`, every character allowed by the General Security Profile.
/// Expects a name already passed through `normalize_username`. Returns the lowercased key.
pub fn validate_username_for(name: &str, unicode: bool) -> Result<String, &'static str> {
    use unicode_security::{GeneralSecurityProfile, RestrictionLevel, RestrictionLevelDetection};
    if !unicode || name.is_ascii() {
        return validate_username(name);
    }
    let count = name.chars().count();
    if !(3..=20).contains(&count) || name.len() > 60 {
        return Err("Username must be 3–20 characters");
    }
    let letter = |c: char| c.is_alphanumeric() && c.identifier_allowed();
    if !name.chars().all(|c| letter(c) || matches!(c, '_' | '-' | '.')) {
        return Err("Username may only contain letters, digits, _ - and .");
    }
    if !name.starts_with(letter) || !name.ends_with(letter) {
        return Err("Username must start and end with a letter or digit");
    }
    let word: String = name.chars().filter(|c| letter(*c)).collect();
    if !word.as_str().check_restriction_level(RestrictionLevel::HighlyRestrictive) {
        return Err("Username must use letters from one writing system");
    }
    let lower = name.to_lowercase();
    reserved_check(&lower)?;
    Ok(lower)
}

/// The UTS #39 confusable skeleton of a name, lowercased first: "раураl" (Cyrillic) and
/// "paypal" share one. Two accounts with the same skeleton look the same to people.
pub fn username_skeleton(name: &str) -> String {
    let lower = normalize_username(name).to_lowercase();
    unicode_security::skeleton(&lower).collect::<String>().to_lowercase()
}

/// A short "Browser on System" from a User-Agent header, for the session list.
pub fn device_label(ua: &str) -> String {
    let (browser, system) = device_parts(ua);
    format!("{browser} on {system}")
}

/// (browser, system) from a User-Agent header.
pub fn device_parts(ua: &str) -> (&'static str, &'static str) {
    let browser =
        [("Edg/", "Edge"), ("OPR/", "Opera"), ("Firefox/", "Firefox"), ("Chrome/", "Chrome"), ("Safari/", "Safari")]
            .iter()
            .find(|(k, _)| ua.contains(k))
            .map_or("Unknown browser", |(_, n)| n);
    let system = [
        ("Windows", "Windows"),
        ("Android", "Android"),
        ("iPhone", "iPhone"),
        ("iPad", "iPad"),
        ("Mac OS", "macOS"),
        ("Linux", "Linux"),
    ]
    .iter()
    .find(|(k, _)| ua.contains(k))
    .map_or("unknown system", |(_, n)| n);
    (browser, system)
}

/// Hex HMAC-SHA256 of a message. Used where the wire format is fixed by someone else's protocol
/// (DiscourseConnect SSO,), which replays are kept out of by a single-use nonce instead of
/// a timestamp. New signatures of our own should use [`webhook_signature`].
#[expect(clippy::expect_used, reason = "HMAC accepts a key of any length")]
pub fn hmac_sha256_hex(secret: &str, message: &[u8]) -> String {
    use hmac::{Hmac, KeyInit, Mac};
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()).expect("any key length");
    mac.update(message);
    mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

/// `t=<unix seconds>,sha256=<hex>` for the `x-bf-signature` header.
///
/// The timestamp is inside the signed message, not just alongside it, so it cannot be changed
/// without breaking the MAC (webhooks signed with a timestamp to prevent replay). A
/// receiver recomputes the HMAC over `<t>.<body>` and rejects anything older than its tolerance,
/// which turns a captured delivery into a short-lived rather than permanent replay.
#[expect(clippy::expect_used, reason = "HMAC accepts a key of any length")]
pub fn webhook_signature(secret: &str, timestamp: i64, body: &[u8]) -> String {
    use hmac::{Hmac, KeyInit, Mac};
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()).expect("any key length");
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    let hex: String = mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect();
    format!("t={timestamp},sha256={hex}")
}

/// Verifies a signature produced by [`webhook_signature`], refusing one outside `tolerance`
/// seconds of `now`. Used by our own inbound endpoints and by the conformance tests; a
/// third-party receiver reimplements this in its own language.
pub fn verify_webhook_signature(secret: &str, header: &str, body: &[u8], now: i64, tolerance: i64) -> bool {
    use subtle::ConstantTimeEq;
    let mut timestamp = None;
    let mut given = None;
    for part in header.split(',') {
        match part.trim().split_once('=') {
            Some(("t", v)) => timestamp = v.parse::<i64>().ok(),
            Some(("sha256", v)) => given = Some(v),
            _ => {}
        }
    }
    let (Some(timestamp), Some(given)) = (timestamp, given) else { return false };
    if now.saturating_sub(timestamp).abs() > tolerance {
        return false;
    }
    let expected = webhook_signature(secret, timestamp, body);
    let Some(expected) = expected.split("sha256=").nth(1) else { return false };
    expected.as_bytes().ct_eq(given.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A member of a MyBB test install: MyBB's `md5(md5(salt) . md5(password))` is the Invision 3 scheme, which is
    /// how the MyBB exporter carries it.
    #[test]
    fn a_mybb_hash_is_checked_and_marked_for_replacement() {
        let hash = "ips3$6384e2b2$cab5b1a59233cbf778b4113c8f410624";
        assert!(is_legacy_hash(hash));
        assert!(verify_password("Alice-pass-1", hash));
        assert!(!verify_password("Alice-pass-2", hash));
    }

    #[test]
    fn password_round_trip() {
        let h = hash_password("correct horse battery");
        assert!(h.starts_with("$argon2id$"));
        assert!(verify_password("correct horse battery", &h));
        assert!(!verify_password("wrong", &h));
        assert!(!verify_password("x", "not a hash"));
    }

    #[test]
    fn tokens_are_unique_and_digested() {
        let (a, b) = (random_token(), random_token());
        assert_ne!(a, b);
        assert_eq!(a.len(), 64);
        assert_eq!(token_digest(&a), token_digest(&a));
        assert_ne!(token_digest(&a), a);
    }

    #[test]
    fn csrf() {
        let t = random_token();
        assert!(csrf_matches(Some(&t), Some(&t)));
        assert!(!csrf_matches(Some(&t), Some("x")));
        assert!(!csrf_matches(None, Some(&t)));
        assert!(!csrf_matches(Some(""), Some("")));
    }

    #[test]
    fn unicode_usernames() {
        assert_eq!(validate_username_for("Michał", true).unwrap(), "michał");
        assert!(validate_username_for("Michał", false).is_err(), "off by default");
        assert_eq!(validate_username_for(&normalize_username("ｊａｎｅ"), true).unwrap(), "jane", "full-width");
        assert!(validate_username_for("pаypal", true).is_err(), "Latin mixed with Cyrillic");
        assert!(validate_username_for("Дмитрий", true).is_ok());
        assert!(validate_username_for("bob@x.y", true).is_err());
        assert!(validate_username_for("forum.news", true).is_err());
        assert_eq!(username_skeleton("раураl"), username_skeleton("paypal"));
        assert_ne!(username_skeleton("alice"), username_skeleton("bob"));
    }

    #[test]
    fn usernames() {
        assert_eq!(validate_username("Jane_Doe").unwrap(), "jane_doe");
        assert!(validate_username("ab").is_err());
        assert!(validate_username("Admin").is_err());
        assert!(validate_username("bad name").is_err());
        assert!(validate_username("_lead").is_err());
    }

    #[test]
    fn passwords() {
        assert!(validate_password("short", "x").is_err());
        assert!(validate_password("JaneDoe123", "janedoe123").is_err());
        assert!(validate_password("a long passphrase", "jane").is_ok());
        assert!(validate_password("Password123", "jane").is_err(), "common password, any case");
        assert!(validate_password("qwertyuiop", "jane").is_err());
    }

    #[test]
    fn hmac_matches_rfc4231_style_vector() {
        // HMAC-SHA256("key", "The quick brown fox jumps over the lazy dog").
        assert_eq!(
            hmac_sha256_hex("key", b"The quick brown fox jumps over the lazy dog"),
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );
    }

    #[test]
    fn webhook_signature_covers_the_timestamp() {
        let a = webhook_signature("key", 1_700_000_000, b"{}");
        assert!(a.starts_with("t=1700000000,sha256="));
        // A different send time must produce a different MAC, or the timestamp could be edited.
        assert_ne!(a.split("sha256=").nth(1), webhook_signature("key", 1_700_000_001, b"{}").split("sha256=").nth(1));
    }

    #[test]
    fn webhook_signature_round_trips_and_refuses_tampering() {
        let now = 1_700_000_000;
        let sig = webhook_signature("key", now, b"{\"a\":1}");
        assert!(verify_webhook_signature("key", &sig, b"{\"a\":1}", now, 300));
        // Wrong key, edited body, and a body swapped for another all fail.
        assert!(!verify_webhook_signature("other", &sig, b"{\"a\":1}", now, 300));
        assert!(!verify_webhook_signature("key", &sig, b"{\"a\":2}", now, 300));
    }

    #[test]
    fn webhook_signature_expires() {
        let sig = webhook_signature("key", 1_700_000_000, b"{}");
        // Replayed 10 minutes later, outside a 5-minute tolerance.
        assert!(!verify_webhook_signature("key", &sig, b"{}", 1_700_000_600, 300));
        assert!(verify_webhook_signature("key", &sig, b"{}", 1_700_000_200, 300));
        // A timestamp far in the future is refused too, so a forged clock gains nothing.
        assert!(!verify_webhook_signature("key", &sig, b"{}", 1_699_999_000, 300));
    }

    #[test]
    fn webhook_signature_rejects_malformed_headers() {
        for header in ["", "sha256=abc", "t=1700000000", "t=notanumber,sha256=abc", "garbage"] {
            assert!(!verify_webhook_signature("key", header, b"{}", 1_700_000_000, 300), "accepted {header:?}");
        }
    }
}

#[cfg(test)]
mod csrf_tests;
