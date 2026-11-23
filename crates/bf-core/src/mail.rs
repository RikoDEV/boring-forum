//! Account emails: the kinds an administrator can rewrite and the built-in verification and reset
//! texts. Delivery (SMTP, retries) is the mail job in bf-server.
//!
//! What a member receives is read from the catalog in their language: each mail's subject and body
//! are messages `mail.<kind>.subject` / `.body` (`locales/*.ftl`), rewritten per language in the
//! admin panel's Site text page. `TEMPLATE_KINDS` lists those messages; a kind needs its catalog
//! entries before it is added. `VERIFY_*` / `RESET_*` and `account_email` build the link and the
//! English fallback. Every plain-text mail follows one shape: "Hi {name}," a body, then "— {site}";
//! its subject reads "{what it is} - {site}".

use crate::settings::Settings;

#[derive(Debug, Clone, PartialEq)]
pub struct Email {
    pub to: String,
    pub subject: String,
    pub text: String,
}

#[derive(Debug, Clone, Copy)]
pub enum Kind {
    Verify,
    Reset,
}

/// The mails an administrator can rewrite: (kind, placeholders, the one placeholder a text must
/// keep or `""`). Adding a kind here makes it appear in the admin panel and pass validation.
pub const TEMPLATE_KINDS: &[(&str, &[&str], &str)] = &[
    ("verify", &["site", "username", "link"], "link"),
    ("reset", &["site", "username", "link"], "link"),
    ("notification", &["site", "username", "by", "title", "link"], "link"),
    ("digest", &["site", "username", "topics", "link", "unsubscribe"], "topics"),
    ("security", &["site", "username", "what", "link"], ""),
    ("backup", &["site", "username", "name", "link"], "link"),
];

/// Built-in English texts of the two account mails (the catalog's texts are what is sent).
pub const VERIFY_SUBJECT: &str = "Confirm your email for {site}";
pub const VERIFY_BODY: &str = "Hi {username},\n\nConfirm your email address by opening this link within 24 hours:\n\n{link}\n\nIf you did not ask for this, ignore this email.\n\n— {site}\n";
pub const RESET_SUBJECT: &str = "Reset your {site} password";
pub const RESET_BODY: &str = "Hi {username},\n\nSomeone asked to reset your password. If it was you, open this link within 1 hour:\n\n{link}\n\nIf you did not ask for this, ignore this email.\n\n— {site}\n";

/// Fills a template in one pass, so a value that itself contains `{link}` (a username,
/// say) is never expanded again.
pub fn fill(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len() + 64);
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        if let Some((k, v)) = vars.iter().find(|(k, _)| tail.starts_with(&format!("{{{k}}}"))) {
            out.push_str(v);
            rest = &tail[k.len() + 2..];
        } else {
            out.push('{');
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

/// Links carry the raw token; only its digest is stored (auth::token_digest).
pub fn account_email(kind: Kind, settings: &Settings, base_url: &str, to: &str, username: &str, token: &str) -> Email {
    let base = base_url.trim_end_matches('/');
    let (subject_tpl, body_tpl, path) = match kind {
        Kind::Verify => (VERIFY_SUBJECT, VERIFY_BODY, "verify"),
        Kind::Reset => (RESET_SUBJECT, RESET_BODY, "reset"),
    };
    let link = format!("{base}/{path}/{token}");
    let vars = [("site", settings.str("site_name")), ("username", username), ("link", link.as_str())];
    // Header injection guard: a subject or address can never contain a line break.
    let clean = |s: &str| s.replace(['\r', '\n'], " ");
    let mut text = fill(body_tpl, &vars);
    // An edited template that dropped {link} would make the email useless; keep it working.
    if !text.contains(&link) {
        text.push_str(&format!("\n{link}\n"));
    }
    Email { to: clean(to), subject: clean(&fill(subject_tpl, &vars)), text }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn settings() -> Settings {
        let mut s = Settings::default();
        s.set("site_name", json!("Forum")).unwrap();
        s
    }

    #[test]
    fn reset_email_links_token() {
        let e = account_email(Kind::Reset, &settings(), "https://f.test/", "a@b.c", "jane", "tok");
        assert!(e.text.contains("https://f.test/reset/tok"));
        assert!(e.text.starts_with("Hi jane,"));
        assert_eq!(e.subject, "Reset your Forum password");
    }

    #[test]
    fn no_header_injection() {
        let mut s = settings();
        s.set("site_name", json!("F\r\nBcc: x@y.z")).unwrap();
        let e = account_email(Kind::Verify, &s, "https://f", "a@b.c\nBcc: evil@x", "j", "t");
        assert!(!e.subject.contains('\n') && !e.to.contains('\n'));
    }

    #[test]
    fn fill_is_single_pass() {
        assert_eq!(fill("{username} {link}", &[("username", "{link}"), ("link", "L")]), "{link} L");
        assert_eq!(fill("{unknown} {", &[("site", "x")]), "{unknown} {");
    }
}
