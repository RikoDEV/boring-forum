//! Custom code, translation, referrals and Mailchimp.

use super::super::{SettingDef, SettingType::*};
use super::{def, int};
use crate::i18n::en;
use serde_json::json;

pub(super) fn defs() -> Vec<SettingDef> {
    vec![
        // GIF picker (ACP → Settings → GIFs). The providers (KLIPY, GIPHY) require that the browser
        // asks their API and loads their media directly, so the key is public: the editor reads it
        // from the page.
        SettingDef {
            default: json!(false),
            ..def("gifs_enabled", Bool, false, en("Add a button to the editor to search and insert GIFs"))
        },
        SettingDef {
            default: json!("klipy"),
            ..def("gif_provider", String, false, en("Where GIFs come from: klipy or giphy"))
        },
        SettingDef {
            default: json!(""),
            ..def(
                "gif_api_key",
                String,
                false,
                en(
                    "The provider's API key, required to search GIFs. Members' browsers send it to the provider, as providers require",
                ),
            )
        },
        SettingDef {
            default: json!("webp"),
            ..def(
                "gif_format",
                String,
                false,
                en("Image format of an inserted GIF: webp is smaller and loads quicker, gif works in older browsers"),
            )
        },
        SettingDef {
            default: json!("high"),
            ..def(
                "gif_content_filter",
                String,
                false,
                en("How strictly results are filtered: off, low, medium or high"),
            )
        },
        SettingDef {
            default: json!("US"),
            ..def("gif_country", String, false, en("KLIPY: two-letter country code that tunes search results"))
        },
        SettingDef {
            default: json!(false),
            ..def(
                "gif_limit_scroll",
                Bool,
                false,
                en("Stop loading more GIFs as members scroll, after the maximum below, to save provider requests"),
            )
        },
        int(
            "gif_max_results",
            240,
            24,
            1000,
            en("With the limit on, stop loading after this many GIFs (24 are loaded at a time)"),
        ),
        // imgproxy (ACP → Settings → Uploads and media): pictures from other sites go through the
        // admin's own imgproxy, signed with its key and salt.
        SettingDef {
            default: json!(false),
            ..def(
                "imgproxy_enabled",
                Bool,
                false,
                en("Show pictures from other sites, and link previews' pictures, through your imgproxy"),
            )
        },
        SettingDef {
            default: json!(""),
            ..def("imgproxy_url", String, false, en("The address of your imgproxy, like https://img.example.com"))
        },
        SettingDef {
            default: json!(""),
            ..def("imgproxy_key", String, false, en("imgproxy's IMGPROXY_KEY, in hex, to sign addresses with"))
        },
        SettingDef {
            default: json!(""),
            ..def("imgproxy_salt", String, false, en("imgproxy's IMGPROXY_SALT, in hex, to sign addresses with"))
        },
        int("imgproxy_width", 1280, 100, 4000, en("The widest picture imgproxy makes, in pixels")),
        SettingDef {
            default: json!(""),
            ..def(
                "imgproxy_format",
                String,
                false,
                en(
                    "The format imgproxy makes: empty lets imgproxy choose by the browser (webp, avif), or webp, avif, jpeg, png",
                ),
            )
        },
        SettingDef {
            default: json!(false),
            ..def(
                "imgproxy_gifs",
                Bool,
                false,
                en(
                    "Also send GIFs through imgproxy (it keeps only the first frame unless its animation settings are on)",
                ),
            )
        },
        // Custom code (ACP → Settings → Custom code). Not sanitized: admins add scripts here.
        SettingDef {
            default: json!(""),
            ..def(
                "custom_css",
                String,
                false,
                en("CSS added to every page after the theme's, whatever theme a member uses"),
            )
        },
        SettingDef {
            default: json!(""),
            ..def(
                "custom_head_html",
                String,
                false,
                en("HTML added at the end of <head> on every page: meta tags, stylesheets, scripts"),
            )
        },
        SettingDef {
            default: json!(""),
            ..def(
                "custom_footer_html",
                String,
                false,
                en("HTML added after the footer on every page: widgets, scripts that need the page"),
            )
        },
        SettingDef {
            default: json!(""),
            ..def(
                "custom_code_hosts",
                String,
                false,
                en(
                    "More https hosts the custom HTML may load scripts, styles, frames and data from, one per line (script hosts in the HTML are added on their own)",
                ),
            )
        },
        SettingDef {
            default: json!(false),
            ..def(
                "content_translation_enabled",
                Bool,
                true,
                en(
                    "Translations: authors, staff and the translator group can add other-language versions of posts and forums",
                ),
            )
        },
        SettingDef {
            default: json!(""),
            ..def(
                "translator_groups",
                String,
                false,
                en("Translations: groups whose members may translate anyone's posts, besides staff"),
            )
        },
        SettingDef {
            default: json!(false),
            ..def(
                "live_topics_enabled",
                Bool,
                true,
                en("Live topics: moderators and topic authors can host time-boxed Q&A sessions (listed at /live)"),
            )
        },
        SettingDef {
            default: json!(false),
            ..def(
                "referrals_enabled",
                Bool,
                true,
                en("Referrals: members share /r/their-name links and see who joined at /referrals"),
            )
        },
        SettingDef {
            default: json!(""),
            ..def(
                "referral_reward_groups",
                String,
                false,
                en("Referrals: groups given for enough referrals that reach trust level 1"),
            )
        },
        int("referral_reward_count", 5, 1, 10_000, en("Referrals: how many qualified referrals earn the group")),
        SettingDef {
            default: json!(""),
            ..def(
                "mailchimp_audience_id",
                String,
                false,
                en("Mailchimp: audience (list) id to sync newsletter opt-ins to; needs BF_MAILCHIMP_API_KEY"),
            )
        },
        SettingDef {
            default: json!(""),
            ..def(
                "mailchimp_webhook_secret",
                String,
                false,
                en("Mailchimp: secret (16+ characters) in the webhook address /newsletter/mailchimp?secret=…"),
            )
        },
    ]
}
