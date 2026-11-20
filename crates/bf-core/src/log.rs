//! Keeping secrets out of the logs ("logs never contain passwords, tokens, cookies, full
//! emails or post content").
//!
//! Request paths are safe to log; query strings are not. Password resets, email verification,
//! magic links, unsubscribe links, OAuth callbacks and the inbound-mail webhook all carry a
//! credential in the query string, and a log line is copied to places a credential should not
//! reach -- an aggregator, a support ticket, a screenshot. [`redact_uri`] keeps the part that
//! makes a log useful and drops the part that makes it dangerous.

/// Query-string keys whose values are credentials or personal data. Matched case-insensitively
/// against the whole key, so `token` covers `?token=` but not `tokenizer`.
const SENSITIVE: &[&str] = &[
    "access_token",
    "api_key",
    "code",
    "email",
    "id_token",
    "key",
    "nonce",
    "password",
    "refresh_token",
    "secret",
    "session",
    "signature",
    "sig",
    "sso",
    "state",
    "token",
];

fn is_sensitive(key: &str) -> bool {
    SENSITIVE.iter().any(|s| key.eq_ignore_ascii_case(s))
}

/// Routes that carry a credential as the last path segment.
const TOKEN_PATHS: &[&str] = &["/reset/", "/verify/", "/confirm-email/", "/login/magic/", "/unsubscribe/"];

/// The path with any token segment replaced by `***`.
pub fn redact_path(path: &str) -> String {
    for prefix in TOKEN_PATHS {
        if let Some(rest) = path.strip_prefix(prefix) {
            // `/unsubscribe/{token}/topic` keeps the suffix.
            let suffix = rest.find('/').map_or("", |i| &rest[i..]);
            return format!("{prefix}***{suffix}");
        }
    }
    path.to_owned()
}

/// The path, plus a query string with every credential replaced by `***`. Keys are kept so a log
/// line still shows which parameters were present, which is what makes one worth reading.
pub fn redact_uri(uri: &str) -> String {
    let (path, query) = match uri.split_once('?') {
        Some((path, query)) => (redact_path(path), query),
        None => return redact_path(uri),
    };
    let path = path.as_str();
    let mut out = String::with_capacity(uri.len());
    out.push_str(path);
    out.push('?');
    for (i, pair) in query.split('&').enumerate() {
        if i > 0 {
            out.push('&');
        }
        match pair.split_once('=') {
            Some((key, _)) if is_sensitive(key) => {
                out.push_str(key);
                out.push_str("=***");
            }
            // A bare flag has no value to leak, and an unlisted key keeps its value.
            _ => out.push_str(pair),
        }
    }
    out
}

/// An email address reduced to something that can be matched against a support request without
/// being a readable address: `jane.doe@example.com` becomes `j***@example.com`. For the cases
/// where a log line is useless without *some* handle on which account it was.
pub fn redact_email(email: &str) -> String {
    match email.split_once('@') {
        Some((local, domain)) => {
            let first = local.chars().next().map(String::from).unwrap_or_default();
            format!("{first}***@{domain}")
        }
        None => "***".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_a_plain_path() {
        assert_eq!(redact_uri("/topic/12-hello"), "/topic/12-hello");
        assert_eq!(redact_uri("/"), "/");
    }

    #[test]
    fn hides_credentials_but_keeps_the_shape() {
        assert_eq!(redact_uri("/reset?token=abc123"), "/reset?token=***");
        assert_eq!(redact_uri("/api/v1/webhooks/bounce?token=secret"), "/api/v1/webhooks/bounce?token=***");
        assert_eq!(
            redact_uri("/auth/callback?code=xyz&state=abc&provider=google"),
            "/auth/callback?code=***&state=***&provider=google"
        );
    }

    #[test]
    fn keeps_harmless_parameters() {
        assert_eq!(redact_uri("/forum/3?page=2&sort=latest"), "/forum/3?page=2&sort=latest");
    }

    #[test]
    fn matches_keys_whole_and_case_insensitively() {
        assert_eq!(redact_uri("/x?TOKEN=a"), "/x?TOKEN=***");
        // Not a credential, and losing it would make a log harder to read for no gain.
        assert_eq!(redact_uri("/x?tokenizer=a"), "/x?tokenizer=a");
    }

    #[test]
    fn survives_odd_query_strings() {
        assert_eq!(redact_uri("/x?"), "/x?");
        assert_eq!(redact_uri("/x?flag"), "/x?flag");
        assert_eq!(redact_uri("/x?token="), "/x?token=***");
        assert_eq!(redact_uri("/x?a=1&token=2&b=3"), "/x?a=1&token=***&b=3");
    }

    #[test]
    fn emails_keep_only_a_hint() {
        assert_eq!(redact_email("jane.doe@example.com"), "j***@example.com");
        assert_eq!(redact_email("not-an-email"), "***");
        assert_eq!(redact_email("@example.com"), "***@example.com");
    }
}
