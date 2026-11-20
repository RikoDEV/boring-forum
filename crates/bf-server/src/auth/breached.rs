//! Have I Been Pwned range check (ASVS 2.1.7), on top of the embedded common-password
//! list in `bf_core::auth`. Only the first five hex characters of the password's SHA-1 leave
//! the server (k-anonymity), and `Add-Padding` hides the size of the answer. A network failure
//! lets the password through: an outage at a third party must not stop people signing up.

use crate::state::AppState;
use sha1::{Digest, Sha1};
use std::time::Duration;

const API: &str = "https://api.pwnedpasswords.com/range/";

static HTTP: std::sync::LazyLock<Option<reqwest::Client>> = std::sync::LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("boring-forum password check")
        .build()
        .ok()
});

/// The message to show when the password appears in a breach; `None` to accept it.
pub async fn refuse(state: &AppState, password: &str) -> Option<&'static str> {
    use bf_core::i18n::en;
    if !state.settings().bool("hibp_password_check") {
        return None;
    }
    match seen_in_breach(password).await {
        Some(true) => {
            metrics::counter!("bf_breached_password_refusals_total").increment(1);
            // Marked English: the caller translates it into the viewer's language.
            Some(en("That password has appeared in a data breach, so attackers try it first. Choose another."))
        }
        Some(false) => None,
        None => {
            tracing::warn!("breached-password service unreachable; password accepted");
            None
        }
    }
}

/// `None` when the service could not be asked.
async fn seen_in_breach(password: &str) -> Option<bool> {
    let digest: String = Sha1::digest(password.as_bytes()).iter().map(|b| format!("{b:02X}")).collect();
    let (prefix, suffix) = digest.split_at(5);
    let client = HTTP.as_ref()?;
    let res = client.get(format!("{API}{prefix}")).header("Add-Padding", "true").send().await.ok()?;
    if !res.status().is_success() {
        return None;
    }
    let body = res.text().await.ok()?;
    Some(listed(&body, suffix))
}

/// Padding rows have a count of 0 and do not count as a breach.
fn listed(body: &str, suffix: &str) -> bool {
    body.lines()
        .filter_map(|l| l.trim().split_once(':'))
        .any(|(s, n)| s.eq_ignore_ascii_case(suffix) && n.trim().parse::<u64>().is_ok_and(|n| n > 0))
}

#[cfg(test)]
mod tests {
    #[test]
    fn padding_rows_do_not_count() {
        let body = "0018A45C4D1DEF81644B54AB7F969B88D65:1\r\n00D4F6E8FA6EECAD2A3AA415EEC418D38EC:0\r\n";
        assert!(super::listed(body, "0018a45c4d1def81644b54ab7f969b88d65"));
        assert!(!super::listed(body, "00D4F6E8FA6EECAD2A3AA415EEC418D38EC"));
        assert!(!super::listed(body, "FFFF"));
    }
}
