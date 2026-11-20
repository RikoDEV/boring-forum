//! Fuzzes log redaction.
//!
//! `redact_uri` runs on every request before the URI reaches a log line, so it has to be total:
//! a panic here is a panic on any request. The property is that no sensitive query value
//! survives, however odd the URI -- repeated keys, missing values, encoded separators, no path.

#![no_main]

use libfuzzer_sys::fuzz_target;

/// The keys whose values must never appear in the output. Kept in step with `SENSITIVE` in
/// `bf_core::log`; a key added there and not here only weakens this check, never the redaction.
const SENSITIVE: &[&str] = &["token", "code", "secret", "password", "access_token", "state", "key", "signature"];

fuzz_target!(|data: &[u8]| {
    let Ok(uri) = std::str::from_utf8(data) else { return };
    if uri.len() > 8192 {
        return;
    }

    let redacted = bf_core::log::redact_uri(uri);

    // Every sensitive key must come out with its value replaced. Checked against the output's
    // own pairs rather than by searching for the value, because an unrelated parameter may
    // legitimately carry the same text and that is not a leak.
    if let Some((_, query)) = redacted.split_once('?') {
        for pair in query.split('&') {
            let Some((key, value)) = pair.split_once('=') else { continue };
            if SENSITIVE.iter().any(|s| key.eq_ignore_ascii_case(s)) {
                assert_eq!(
                    value, "***",
                    "redact_uri kept the value of {key:?}\n  input:  {uri:?}\n  output: {redacted:?}"
                );
            }
        }
    }

    // The path is what makes a log line useful, so it must be preserved exactly.
    let path = uri.split('?').next().unwrap_or("");
    assert!(redacted.starts_with(path), "redact_uri changed the path\n  input: {uri:?}\n  output: {redacted:?}");

    // Idempotent, so a URI that passes through two layers of logging is not mangled.
    assert_eq!(bf_core::log::redact_uri(&redacted), redacted, "redact_uri is not idempotent for {uri:?}");
});
