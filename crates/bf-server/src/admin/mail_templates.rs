//! Emails. Each mail is two catalog messages, `mail.<kind>.subject` and `mail.<kind>.body`
//! (see `bf_core::mail::TEMPLATE_KINDS`), so an administrator rewrites one in Site text like any
//! other text, one language at a time. There is no separate template store.

use bf_core::AppError;
use bf_core::i18n::T;

/// The subject and body of a mail in the reader's language, filled in. The subject is one line.
pub(crate) fn render(t: &T, kind: &str, vars: &[(&str, &str)]) -> (String, String) {
    let subject = t.f(&format!("mail.{kind}.subject"), vars).replace(['\r', '\n'], " ");
    (subject, t.f(&format!("mail.{kind}.body"), vars))
}

/// A rewritten mail body must keep the one placeholder the mail is useless without (the link).
pub(crate) fn check_override(key: &str, value: &str) -> Result<(), AppError> {
    let Some(rest) = key.strip_prefix("mail.") else { return Ok(()) };
    let Some((kind, "body")) = rest.rsplit_once('.') else { return Ok(()) };
    let Some((_, _, required)) = bf_core::mail::TEMPLATE_KINDS.iter().find(|(k, _, _)| *k == kind) else {
        return Ok(());
    };
    if !required.is_empty() && !value.contains(&format!("${required}")) {
        return Err(AppError::invalid("The text must contain {name}.", &[("name", &format!("{{ ${required} }}"))]));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mail_renders_in_every_language() {
        let vars = [
            ("site", "Forum"),
            ("username", "jane"),
            ("by", "ann"),
            ("title", "A topic"),
            ("what", "Your password was changed"),
            ("topics", "- A topic"),
            ("link", "https://f.test/x"),
            ("unsubscribe", "https://f.test/u"),
        ];
        for (code, _) in bf_core::i18n::locales() {
            let t = T::new(code);
            for (kind, _, required) in bf_core::mail::TEMPLATE_KINDS {
                let (subject, body) = render(&t, kind, &vars);
                assert!(!subject.is_empty() && !subject.contains('\n'), "{code} {kind} subject: {subject:?}");
                assert!(!body.contains("{ $") && !subject.contains("{ $"), "{code} {kind} left a placeholder");
                assert!(body.contains("Forum"), "{code} {kind} body has no site name");
                let wanted = match *required {
                    "link" => "https://f.test/x",
                    "topics" => "- A topic",
                    _ => "",
                };
                assert!(body.contains(wanted), "{code} {kind} body lost {required}");
            }
        }
        assert_eq!(render(&T::new("en"), "verify", &vars).0, "Confirm your email - Forum");
    }

    #[test]
    fn a_body_must_keep_its_link() {
        assert!(check_override("mail.verify.body", "Go to { $link }").is_ok());
        assert!(check_override("mail.verify.body", "Go somewhere").is_err());
        assert!(check_override("mail.verify.subject", "Hi").is_ok());
        assert!(check_override("mail.security.body", "no link needed").is_ok());
        assert!(check_override("nav.forums", "x").is_ok());
    }
}
