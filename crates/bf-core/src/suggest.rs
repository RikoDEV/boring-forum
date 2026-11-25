//! Suggestions for the administrators: settings and set-up the site would be better with.
//!
//! The rules are pure: they read the settings and a few `Facts` the server gathers (counts, what
//! the traffic looked like, the environment) and return what to propose. A suggestion that no
//! longer applies simply stops being returned, so following one needs no bookkeeping; only the
//! ones an administrator dismissed are stored. The text is written here in English and
//! translated into the viewer's language.

use crate::i18n::T;
use crate::settings::Settings;
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Worth doing; nothing is wrong without it.
    Info,
    /// Something is likely working badly or is unprotected right now.
    Warn,
}

/// A setting the panel can change in one click.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Change {
    pub setting: &'static str,
    pub value: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct Suggestion {
    pub id: &'static str,
    pub severity: Severity,
    pub title: String,
    pub detail: String,
    /// What "Apply" does; none when the fix is not a setting (an environment variable, a page).
    pub change: Option<Change>,
    /// The panel page or setting to open instead of, or as well as, applying.
    pub link: Option<String>,
}

/// What the rules know about the site besides its settings.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    pub base_url: String,
    /// Mail can leave the server (`BF_SMTP_URL` or `BF_MAIL_DIR`).
    pub mail_transport: bool,
    /// `BF_TRUSTED_PROXIES` names at least one proxy.
    pub trusted_proxies: bool,
    /// Requests since the start from a private address with forwarding headers nobody trusts.
    pub unlisted_proxy_requests: u64,
    pub members: i64,
    pub topics: i64,
    /// Administrators with neither an authenticator app nor a passkey.
    pub admins_without_2fa: i64,
    /// Sessions of the last 30 days, and those from Cloudflare's own addresses.
    pub recent_sessions: i64,
    pub cloudflare_sessions: i64,
}

/// Every id the rules can return, so a dismissal can be checked against it.
pub const IDS: &[&str] = &[
    "cloudflare-real-ip",
    "trusted-proxies",
    "base-url-http",
    "admin-2fa",
    "backups-off",
    "email-verification",
    "indexnow",
    "site-description",
];

/// The address of a setting in the panel.
fn setting_link(key: &str) -> String {
    format!("/admin/settings/#setting={key}")
}

fn turn_on(setting: &'static str) -> Option<Change> {
    Some(Change { setting, value: json!(true) })
}

/// A host that only the developer's own machine can reach.
fn is_local(url: &str) -> bool {
    let host = url.split("://").nth(1).unwrap_or(url).split(['/', ':']).next().unwrap_or("");
    host == "localhost" || host.starts_with("127.") || host == "[::1]" || host.ends_with(".local")
}

/// Every suggestion that applies now, most urgent first. `dismissed` ones are left to the caller.
pub fn evaluate(s: &Settings, f: &Facts, t: &T) -> Vec<Suggestion> {
    let mut out = Vec::new();
    let https = f.base_url.starts_with("https://");

    // Behind Cloudflare without saying so: every member shares a handful of edge addresses.
    if !s.bool("cloudflare_real_ip") && f.cloudflare_sessions >= 3 && f.cloudflare_sessions * 2 >= f.recent_sessions {
        out.push(Suggestion {
            id: "cloudflare-real-ip",
            severity: Severity::Warn,
            title: t.msg("Turn on Cloudflare's real visitor address"),
            detail: t.msgf(
                "{n} of {total} recent sign-ins came from Cloudflare's own addresses, so the site sees the same few visitors: rate limits, IP bans and the staff log use the wrong address. The setting trusts the visitor address Cloudflare passes on, only from Cloudflare.",
                &[("n", &f.cloudflare_sessions.to_string()), ("total", &f.recent_sessions.to_string())],
            ),
            change: turn_on("cloudflare_real_ip"),
            link: Some(setting_link("cloudflare_real_ip")),
        });
    }

    // A reverse proxy in front, but the server was not told to believe it.
    if !f.trusted_proxies && !s.bool("cloudflare_real_ip") && f.unlisted_proxy_requests >= 20 {
        out.push(Suggestion {
            id: "trusted-proxies",
            severity: Severity::Warn,
            title: t.msg("Name your reverse proxy in BF_TRUSTED_PROXIES"),
            detail: t.msg(
                "Requests arrive through a proxy on a private address that sends forwarding headers, but BF_TRUSTED_PROXIES is empty. Every visitor then looks like the proxy: rate limits and IP bans hit everyone at once. Set it to the proxy's address and restart.",
            ),
            change: None,
            link: None,
        });
    }

    if !https && !is_local(&f.base_url) && !f.base_url.is_empty() {
        out.push(Suggestion {
            id: "base-url-http",
            severity: Severity::Warn,
            title: t.msg("The site address does not use https"),
            detail: t.msg(
                "BF_BASE_URL starts with http://, so links in email, feeds and previews are not secure and some browser features will not work. Serve the site over https and change the address.",
            ),
            change: None,
            link: None,
        });
    }

    if f.admins_without_2fa > 0 {
        out.push(Suggestion {
            id: "admin-2fa",
            severity: Severity::Warn,
            title: t.msg("Administrators without two-factor sign-in"),
            detail: t.msgf(
                "{n} administrator accounts have neither an authenticator app nor a passkey. A stolen password would give full control of the site.",
                &[("n", &f.admins_without_2fa.to_string())],
            ),
            change: None,
            link: Some("/admin/users/".to_owned()),
        });
    }

    if !s.bool("backup_enabled") && f.members >= 10 {
        out.push(Suggestion {
            id: "backups-off",
            severity: Severity::Warn,
            title: t.msg("Turn on automatic backups"),
            detail: t.msg("The site has members and content but is not backed up automatically."),
            change: turn_on("backup_enabled"),
            link: Some(setting_link("backup_enabled")),
        });
    }

    if !s.bool("require_email_verification") && s.bool("allow_registration") && f.mail_transport {
        out.push(Suggestion {
            id: "email-verification",
            severity: Severity::Info,
            title: t.msg("Ask new members to verify their email"),
            detail: t.msg(
                "Mail is delivered and registration is open, but members can post without confirming their address. Verification keeps out throwaway and mistyped addresses.",
            ),
            change: turn_on("require_email_verification"),
            link: Some(setting_link("require_email_verification")),
        });
    }

    if !s.bool("indexnow_enabled") && https && f.topics > 0 && !s.bool("login_required") && !s.bool("offline") {
        out.push(Suggestion {
            id: "indexnow",
            severity: Severity::Info,
            title: t.msg("Tell search engines about new topics at once"),
            detail: t.msg(
                "IndexNow pings Bing and others when a public topic appears, so it is found within minutes. It sends the addresses of new public topics to api.indexnow.org.",
            ),
            change: turn_on("indexnow_enabled"),
            link: Some(setting_link("indexnow_enabled")),
        });
    }

    if s.str("site_description").trim().is_empty() {
        out.push(Suggestion {
            id: "site-description",
            severity: Severity::Info,
            title: t.msg("Describe the site"),
            detail: t.msg(
                "The default meta description is empty, so search results and link previews of pages without their own text show nothing about the site.",
            ),
            change: None,
            link: Some(setting_link("site_description")),
        });
    }

    out.sort_by_key(|s| s.severity != Severity::Warn);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(s: &Settings, f: &Facts) -> Vec<&'static str> {
        evaluate(s, f, &T::default()).iter().map(|x| x.id).collect()
    }

    fn quiet() -> Facts {
        Facts { base_url: "https://f.test".into(), ..Facts::default() }
    }

    fn settings() -> Settings {
        let mut s = Settings::default();
        s.set("site_description", json!("A forum")).unwrap();
        s
    }

    #[test]
    fn cloudflare_addresses_suggest_the_real_ip_setting() {
        let s = settings();
        let mut f = Facts { recent_sessions: 100, cloudflare_sessions: 80, ..quiet() };
        let got = evaluate(&s, &f, &T::default());
        let cf = got.iter().find(|x| x.id == "cloudflare-real-ip").unwrap();
        assert_eq!(cf.change, Some(Change { setting: "cloudflare_real_ip", value: json!(true) }));
        assert!(cf.detail.contains("80 of 100"));
        // A few edge addresses among many real ones is not a proxy in front.
        f.cloudflare_sessions = 10;
        assert!(!ids(&s, &f).contains(&"cloudflare-real-ip"));
        // Too little traffic to say.
        f = Facts { recent_sessions: 2, cloudflare_sessions: 2, ..quiet() };
        assert!(!ids(&s, &f).contains(&"cloudflare-real-ip"));
        // Already on.
        let mut on = settings();
        on.set("cloudflare_real_ip", json!(true)).unwrap();
        f = Facts { recent_sessions: 100, cloudflare_sessions: 100, ..quiet() };
        assert!(!ids(&on, &f).contains(&"cloudflare-real-ip"));
    }

    #[test]
    fn an_unlisted_proxy_is_pointed_out_once_it_is_clear() {
        let s = settings();
        let mut f = Facts { unlisted_proxy_requests: 19, ..quiet() };
        assert!(!ids(&s, &f).contains(&"trusted-proxies"));
        f.unlisted_proxy_requests = 20;
        assert!(ids(&s, &f).contains(&"trusted-proxies"));
        f.trusted_proxies = true;
        assert!(!ids(&s, &f).contains(&"trusted-proxies"));
    }

    #[test]
    fn http_is_fine_only_on_the_developers_machine() {
        let s = settings();
        for (url, flagged) in [
            ("http://forum.example", true),
            ("https://forum.example", false),
            ("http://localhost:8080", false),
            ("http://127.0.0.1:8080", false),
            ("", false),
        ] {
            let f = Facts { base_url: url.into(), ..Facts::default() };
            assert_eq!(ids(&s, &f).contains(&"base-url-http"), flagged, "{url}");
        }
    }

    #[test]
    fn setting_suggestions_stop_once_followed() {
        let mut s = settings();
        let f = Facts { members: 50, topics: 10, mail_transport: true, ..quiet() };
        let first = ids(&s, &f);
        for id in ["backups-off", "email-verification", "indexnow"] {
            assert!(first.contains(&id), "{id}");
        }
        for key in ["backup_enabled", "require_email_verification", "indexnow_enabled"] {
            s.set(key, json!(true)).unwrap();
        }
        assert!(ids(&s, &f).is_empty());
    }

    #[test]
    fn what_cannot_be_applied_carries_no_change() {
        let s = Settings::default();
        let f = Facts { admins_without_2fa: 2, base_url: "http://x.example".into(), ..Facts::default() };
        for x in evaluate(&s, &f, &T::default()) {
            if matches!(x.id, "admin-2fa" | "base-url-http" | "site-description") {
                assert!(x.change.is_none(), "{}", x.id);
            }
        }
    }

    #[test]
    fn warnings_come_first_and_every_id_is_known() {
        let mut s = Settings::default();
        s.set("site_description", json!("")).unwrap();
        let f = Facts {
            base_url: "https://f.test".into(),
            mail_transport: true,
            unlisted_proxy_requests: 50,
            members: 99,
            topics: 9,
            admins_without_2fa: 1,
            recent_sessions: 10,
            cloudflare_sessions: 10,
            ..Facts::default()
        };
        let got = evaluate(&s, &f, &T::default());
        let first_info = got.iter().position(|x| x.severity == Severity::Info).unwrap();
        assert!(got[first_info..].iter().all(|x| x.severity == Severity::Info));
        assert!(got.iter().all(|x| IDS.contains(&x.id)));
        // Every id in the list can be produced (the http rule needs a different address).
        let mut all: Vec<&str> = got.iter().map(|x| x.id).collect();
        all.push("base-url-http");
        assert!(IDS.iter().all(|id| all.contains(id)), "{all:?}");
    }
}
