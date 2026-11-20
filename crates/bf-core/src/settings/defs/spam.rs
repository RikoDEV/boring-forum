//! Spam defences beyond the basics: reports that silence, link domains, sign-up caps.

use super::super::{SettingDef, SettingType::*};
use super::{def, int};
use crate::i18n::en;
use serde_json::json;

fn text(key: &'static str, description: &'static str) -> SettingDef {
    SettingDef { default: json!(""), ..def(key, String, false, description) }
}

fn number(key: &'static str, default: i64, min: i64, max: i64, description: &'static str) -> SettingDef {
    SettingDef { public: false, ..int(key, default, min, max, description) }
}

pub(super) fn defs() -> Vec<SettingDef> {
    vec![
        number(
            "flag_auto_silence_hours",
            24,
            0,
            720,
            en(
                "Hours to silence a new member whose post is hidden by spam reports or marked spam by AI; 0 never silences",
            ),
        ),
        number(
            "flag_silence_max_trust_level",
            1,
            0,
            20,
            en("Only members up to this trust level are silenced automatically for spam"),
        ),
        number(
            "new_user_spam_host_threshold",
            3,
            0,
            50,
            en(
                "When this many new members post links to the same site within a week, the posts are reported as spam; 0 turns the check off",
            ),
        ),
        number(
            "max_new_accounts_per_ip",
            5,
            0,
            1000,
            en("Most accounts that may be created from one address within the window below; 0 for no limit"),
        ),
        number("new_account_ip_window_hours", 24, 1, 720, en("The window for accounts per address, in hours")),
        text("blocked_link_domains", en("Sites members may not link to, one domain per line (subdomains included)")),
        text(
            "review_link_domains",
            en("Sites whose links hold a post for review before it appears, one domain per line (subdomains included)"),
        ),
        number(
            "duplicate_post_minutes",
            5,
            0,
            1440,
            en("Refuse a post identical to the member's own from this many minutes ago; 0 turns the check off"),
        ),
        text(
            "blocked_user_agents",
            en("Refuse visitors whose browser identifier contains any of these words, one per line (case ignored)"),
        ),
    ]
}
