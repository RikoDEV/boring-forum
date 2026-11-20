//! TOTP two-factor codes (RFC 6238: HMAC-SHA1, 30 s steps, 6 digits), with
//! base32 secrets for authenticator apps and replay protection via the last
//! accepted step.

use hmac::{Hmac, KeyInit, Mac};
use rand::{TryRng, rngs::SysRng};
use sha1::Sha1;

const STEP: u64 = 30;
const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

pub fn base32_encode(bytes: &[u8]) -> String {
    let mut out = String::new();
    let (mut buf, mut bits) = (0u32, 0);
    for &b in bytes {
        buf = (buf << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buf >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buf << (5 - bits)) & 31) as usize] as char);
    }
    out
}

pub fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut buf, mut bits) = (0u32, 0);
    for c in s.chars().filter(|c| !matches!(c, ' ' | '-' | '=')) {
        let v = ALPHABET.iter().position(|&a| a == c.to_ascii_uppercase() as u8)? as u32;
        buf = (buf << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Some(out)
}

/// A new 160-bit secret, base32-encoded.
pub fn new_secret() -> String {
    let mut b = [0u8; 20];
    #[expect(
        clippy::expect_used,
        reason = "the operating system's random source is always present; without it nothing secure can run"
    )]
    SysRng.try_fill_bytes(&mut b).expect("the system random source is available");
    base32_encode(&b)
}

/// `otpauth://` URI for QR codes.
pub fn otpauth_uri(issuer: &str, account: &str, secret: &str) -> String {
    let enc = crate::html::url_encode;
    format!("otpauth://totp/{}:{}?secret={secret}&issuer={}&digits=6&period=30", enc(issuer), enc(account), enc(issuer))
}

/// A QR code for an `otpauth_uri`, as inline SVG with no width/height (so CSS sizes it) and
/// `fill="currentColor"` (so it matches the page's text colour in both themes). Dark runs in
/// each row are merged into one `<rect>`, which keeps the markup well under what a raw
/// one-`<rect>`-per-module grid would need.
pub fn qr_svg(data: &str) -> Option<String> {
    use std::fmt::Write;
    const BORDER: usize = 4;
    let code = qrcode::QrCode::new(data).ok()?;
    let width = code.width();
    let colors = code.to_colors();
    let n = width + BORDER * 2;
    let mut rects = String::new();
    for y in 0..width {
        let row = &colors[y * width..(y + 1) * width];
        let mut x = 0;
        while x < width {
            if row[x] != qrcode::Color::Dark {
                x += 1;
                continue;
            }
            let start = x;
            while x < width && row[x] == qrcode::Color::Dark {
                x += 1;
            }
            let _ = write!(
                rects,
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"1\"/>",
                start + BORDER,
                y + BORDER,
                x - start
            );
        }
    }
    Some(format!(
        "<svg viewBox=\"0 0 {n} {n}\" xmlns=\"http://www.w3.org/2000/svg\" fill=\"currentColor\" role=\"img\" aria-label=\"Two-factor setup QR code\" shape-rendering=\"crispEdges\">{rects}</svg>"
    ))
}

fn hotp(key: &[u8], counter: u64) -> u32 {
    #[expect(clippy::expect_used, reason = "HMAC accepts a key of any length")]
    let mut mac = Hmac::<Sha1>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(&counter.to_be_bytes());
    let h = mac.finalize().into_bytes();
    let o = (h[19] & 0xf) as usize;
    let n = u32::from_be_bytes([h[o] & 0x7f, h[o + 1], h[o + 2], h[o + 3]]);
    n % 1_000_000
}

pub fn code_at(secret: &str, unix: u64) -> Option<String> {
    Some(format!("{:06}", hotp(&base32_decode(secret)?, unix / STEP)))
}

/// Checks a code with ±1 step of clock skew. Returns the matched step, which the
/// caller stores; codes at or before `last_step` are rejected as replays.
pub fn verify(secret: &str, code: &str, unix: u64, last_step: Option<u64>) -> Option<u64> {
    let key = base32_decode(secret)?;
    let code: u32 = code.trim().parse().ok().filter(|_| code.trim().len() == 6)?;
    let now = unix / STEP;
    [now.saturating_sub(1), now, now + 1]
        .into_iter()
        .filter(|s| last_step.is_none_or(|l| *s > l))
        .find(|s| hotp(&key, *s) == code)
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 6238 appendix B, SHA-1 secret "12345678901234567890".
    const RFC_SECRET: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";

    #[test]
    fn rfc6238_vectors() {
        for (t, want) in
            [(59, "287082"), (1_111_111_109, "081804"), (1_234_567_890, "005924"), (2_000_000_000, "279037")]
        {
            assert_eq!(code_at(RFC_SECRET, t).unwrap(), want, "t={t}");
        }
    }

    #[test]
    fn base32_round_trip() {
        assert_eq!(base32_encode(b"12345678901234567890"), RFC_SECRET);
        let s = new_secret();
        assert_eq!(base32_encode(&base32_decode(&s).unwrap()), s);
    }

    #[test]
    fn verify_allows_skew_and_blocks_replay() {
        let t = 1_234_567_890;
        let code = code_at(RFC_SECRET, t - 30).unwrap();
        let step = verify(RFC_SECRET, &code, t, None).expect("previous step accepted");
        assert_eq!(verify(RFC_SECRET, &code, t, Some(step)), None);
        assert_eq!(verify(RFC_SECRET, "000000", t, None), None);
        assert_eq!(verify(RFC_SECRET, "12345", t, None), None);
    }

    #[test]
    fn uri_is_encoded() {
        let u = otpauth_uri("Boring Forum", "jane@x.y", "ABC");
        assert!(u.starts_with("otpauth://totp/Boring%20Forum:jane%40x.y?secret=ABC"));
    }

    #[test]
    fn qr_renders_the_uri_as_inline_svg() {
        let u = otpauth_uri("Boring Forum", "jane@x.y", &new_secret());
        let svg = qr_svg(&u).expect("an otpauth URI this short fits a QR code");
        assert!(svg.starts_with("<svg"));
        assert!(svg.ends_with("</svg>"));
        assert!(svg.contains("<rect"));
        // No width/height on the <svg> itself: CSS sizes it, scaling cleanly via currentColor.
        let root = &svg[..svg.find('>').unwrap()];
        assert!(!root.contains("width="));
        assert!(root.contains("fill=\"currentColor\""));
    }
}
