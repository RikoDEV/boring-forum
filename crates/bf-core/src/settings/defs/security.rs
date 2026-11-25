//! Password and account security, federation, DSA, analytics.

use super::super::{SettingDef, SettingType::*};
use super::{def, int};
use crate::i18n::en;
use serde_json::json;

pub(super) fn defs() -> Vec<SettingDef> {
    vec![
        // Modern standards.
        SettingDef {
            default: json!(false),
            ..def(
                "hibp_password_check",
                Bool,
                false,
                en(
                    "Also check new passwords against Have I Been Pwned (k-anonymity: only 5 hex characters of a hash leave the server)",
                ),
            )
        },
        SettingDef {
            default: json!(""),
            ..def(
                "security_contact",
                String,
                false,
                en("security.txt Contact: a mailto: or https: address for vulnerability reports"),
            )
        },
        SettingDef {
            default: json!(""),
            ..def(
                "security_policy_url",
                String,
                false,
                en("security.txt Policy: the URL of the vulnerability disclosure policy"),
            )
        },
        SettingDef {
            default: json!(false),
            ..def(
                "unicode_usernames",
                Bool,
                true,
                en("Allow letters from any script in usernames (normalized, single-script, confusables refused)"),
            )
        },
        SettingDef {
            default: json!("allow"),
            ..def("ai_crawlers", String, false, en("AI training crawlers in robots.txt: allow or block"))
        },
        SettingDef {
            default: json!(
                "User-agent: *\nDisallow: /admin\nDisallow: /api/\nDisallow: /search\nDisallow: /login\nDisallow: /register"
            ),
            ..def(
                "robots_txt",
                String,
                false,
                en(
                    "robots.txt rules, one per line. The AI-crawler block (when on) and the sitemap line are added after them",
                ),
            )
        },
        SettingDef {
            default: json!(false),
            ..def(
                "indexnow_enabled",
                Bool,
                false,
                en(
                    "IndexNow: tell search engines about new public topics right away (sends their addresses to api.indexnow.org; needs an https address)",
                ),
            )
        },
        SettingDef {
            default: json!(""),
            ..def(
                "indexnow_key",
                String,
                false,
                en("IndexNow key (8 to 128 letters, digits and dashes); empty uses a key made from the site address"),
            )
        },
        SettingDef {
            default: json!(false),
            ..def(
                "federation_enabled",
                Bool,
                true,
                en("ActivityPub: members and public forums can be followed from the fediverse"),
            )
        },
        SettingDef {
            default: json!(""),
            ..def(
                "federation_blocked_domains",
                String,
                false,
                en("ActivityPub: instances to refuse, one domain per line (subdomains included)"),
            )
        },
        SettingDef {
            default: json!(false),
            ..def(
                "federation_dms",
                Bool,
                false,
                en("ActivityPub: direct messages to and from people on other servers become private conversations"),
            )
        },
        int(
            "federation_posts_per_hour",
            60,
            1,
            10_000,
            en("ActivityPub: most remote replies accepted from one instance per hour"),
        ),
        SettingDef {
            default: json!(""),
            ..def(
                "dsa_contact_email",
                String,
                true,
                en("EU DSA single point of contact (Art. 11-12), shown on the notice form and transparency page"),
            )
        },
        SettingDef {
            default: json!(true),
            ..def("dsa_notices_enabled", Bool, true, en("EU DSA: anyone can report illegal content at /legal/notice"))
        },
        SettingDef {
            default: json!(true),
            ..def(
                "record_pageviews",
                Bool,
                false,
                en(
                    "Count page views for the dashboard's site traffic: no cookie, no address kept, visitors told apart by a hash that changes daily",
                ),
            )
        },
        SettingDef {
            default: json!(""),
            ..def(
                "analytics_provider",
                String,
                false,
                en(
                    "Analytics: plausible, umami, ga4 or matomo; empty for none. GA4 and Matomo wait for cookie consent when a cookie notice is set",
                ),
            )
        },
        SettingDef {
            default: json!(""),
            ..def(
                "analytics_id",
                String,
                false,
                en(
                    "Analytics: the site's domain (Plausible), website id (Umami), measurement id G-… (GA4) or site id (Matomo)",
                ),
            )
        },
        SettingDef {
            default: json!(""),
            ..def(
                "analytics_host",
                String,
                false,
                en("Analytics: https address of a self-hosted Plausible, Umami or Matomo"),
            )
        },
    ]
}
