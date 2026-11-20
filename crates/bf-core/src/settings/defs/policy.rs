//! Retention, formatting, trust levels, plugins, reactions, warnings, clubs and mail texts.

use super::super::{SettingDef, SettingType::*};
use super::{def, int};
use crate::i18n::en;
use serde_json::json;

pub(super) fn defs() -> Vec<SettingDef> {
    vec![
        // Backups (admin panel: Backups). Off until an administrator turns them on.
        SettingDef {
            default: json!(false),
            ..def("backup_enabled", Bool, false, en("Back up the site automatically"))
        },
        int("backup_interval_hours", 24, 1, 720, en("Hours between automatic backups")),
        int("backup_keep", 7, 1, 365, en("Backups to keep; older ones are deleted after each new backup")),
        SettingDef {
            default: json!(true),
            ..def("backup_uploads", Bool, false, en("Include uploaded files in automatic backups"))
        },
        // IP addresses are kept for a limited time, the event itself for longer.
        int(
            "ip_retention_days",
            180,
            0,
            3650,
            en(
                "Forget IP addresses after this many days: on audit events, posts, sessions and failed sign-ins; 0 keeps them",
            ),
        ),
        int(
            "operational_retention_days",
            90,
            0,
            3650,
            en(
                "Delete task logs, dismissed admin notices, link previews, profile visits and payment event records after this many days; 0 keeps them",
            ),
        ),
        int(
            "notification_retention_days",
            365,
            0,
            3650,
            en("Delete notifications older than this many days, read or not; 0 keeps them"),
        ),
        int(
            "audit_retention_days",
            0,
            0,
            36500,
            en("Delete audit log events after this many days; 0 keeps them for good"),
        ),
        SettingDef {
            default: json!(true),
            ..def("bbcode_enabled", Bool, true, en("Convert BBCode in new and edited posts"))
        },
        SettingDef {
            default: json!(false),
            ..def(
                "math_enabled",
                Bool,
                true,
                en("Typeset $…$, $$…$$ and ```math blocks with KaTeX in new and edited posts"),
            )
        },
        SettingDef {
            default: json!(""),
            ..def(
                "csp_image_hosts",
                String,
                false,
                en("Hosts posts may load images from, like https://a.example; empty allows any https host"),
            )
        },
        SettingDef {
            default: json!(""),
            ..def(
                "trust_levels",
                String,
                false,
                en(
                    "The trust levels as JSON, from level 0 up: name, whether activity earns it, and what it needs; empty is the default set",
                ),
            )
        },
        SettingDef {
            default: json!(true),
            ..def("show_trust_levels", Bool, true, en("Show each member's trust level on posts, profiles and cards"))
        },
        int("trust_recent_days", 100, 1, 3650, en("Days the recent-activity window of trust levels covers")),
        SettingDef {
            default: json!(""),
            ..def(
                "sidebar_links",
                String,
                true,
                en("Sidebar links as JSON, the same format as the navigation; empty hides the sidebar"),
            )
        },
        int("default_theme_id", 0, 0, i64::MAX, en("The site's theme; 0 uses the built-in look")),
        SettingDef {
            default: json!("standard"),
            ..def(
                "default_light_scheme",
                String,
                false,
                en("The colour scheme (its address name) whose light palette everyone who has not picked one sees"),
            )
        },
        SettingDef {
            default: json!("standard"),
            ..def(
                "default_dark_scheme",
                String,
                false,
                en("The colour scheme (its address name) whose dark palette everyone who has not picked one sees"),
            )
        },
        SettingDef {
            default: json!(true),
            ..def(
                "builtin_theme_enabled",
                Bool,
                false,
                en("Offer the built-in look as a theme; it can be turned off once another theme is the site's default"),
            )
        },
        SettingDef {
            // On by default: installing an unsigned wasm plugin is refused unless the
            // operator deliberately turns this off. Signatures are checked against the Ed25519
            // keys in BF_PLUGIN_PUBLIC_KEYS.
            default: json!(true),
            ..def(
                "require_signed_plugins",
                Bool,
                false,
                en("Refuse plugins not signed by a key in BF_PLUGIN_PUBLIC_KEYS"),
            )
        },
        int(
            "plugin_storage_mb",
            256,
            0,
            102_400,
            "Storage each plugin may use for its tables and key-value data, in MB",
        ),
        int("plugin_posts_per_hour", 60, 0, 10_000, en("Posts each plugin's account may make in an hour")),
        int("plugin_db_reads_per_minute", 600, 1, 100_000, en("Database reads each plugin may make in a minute")),
        int("plugin_notifications_per_hour", 300, 1, 100_000, en("Notifications each plugin may send in an hour")),
        int("plugin_jobs_per_minute", 60, 1, 10_000, en("Jobs each plugin may queue in a minute")),
        int("plugin_emails_per_hour", 60, 1, 10_000, en("Emails each plugin may send in an hour")),
        int("plugin_http_per_minute", 120, 1, 10_000, en("Outbound requests each plugin may make in a minute")),
        int("plugin_actions_per_minute", 30, 1, 1_000, en("Button presses each member may make on each plugin in a minute")),
        int("plugin_max_schedules", 5, 0, 100, en("Recurring jobs each plugin may schedule")),
        int("pm_max_participants", 20, 2, 500, en("Most people in one private conversation")),
        int("pm_per_hour", 20, 0, 1000, en("New private conversations a member may start in an hour")),
        int(
            "reaction_change_minutes",
            60,
            0,
            525_600,
            "Minutes a member can change or remove a reaction; 0 means no limit",
        ),
        int("reactions_per_day", 200, 1, 100_000, en("Reactions a member may give in a day")),
        SettingDef {
            default: json!(true),
            ..def("solved_enabled", Bool, true, en("Let topic authors and moderators mark a reply as the answer"))
        },
        int("warning_silence_points", 5, 0, 1000, en("Active warning points that silence a member; 0 is off")),
        int("warning_suspend_points", 10, 0, 1000, en("Active warning points that suspend a member; 0 is off")),
        int("warning_action_days", 7, 1, 3650, en("Days an automatic silence or suspension from warnings lasts")),
        int("warning_expire_days", 90, 1, 3650, en("Days before warning points stop counting")),
        int("wiki_min_trust_level", 1, 0, 20, en("Trust level needed to edit wiki posts")),
        SettingDef {
            default: json!(""),
            ..def("contact_email", String, false, en("Where the contact form sends mail; empty turns the form off"))
        },
        SettingDef {
            default: json!(true),
            ..def("signatures_enabled", Bool, true, en("Show member signatures under posts"))
        },
        int("signature_max_length", 300, 0, 2000, en("Longest signature, in characters")),
        SettingDef { default: json!(false), ..def("clubs_enabled", Bool, true, en("Let members start clubs")) },
        int("club_min_trust_level", 1, 0, 20, en("Trust level needed to start a club")),
        int("clubs_parent_node", 0, 0, 1_000_000_000, en("Category new clubs go under")),
        SettingDef {
            default: json!(false),
            ..def(
                "anonymous_posting",
                Bool,
                true,
                en("Let members reply anonymously; staff can still see who in the log"),
            )
        },
        SettingDef {
            default: json!("0:Rookie,10:Contributor,100:Respected,1000:Legend"),
            ..def("rank_names", String, true, en("Titles members earn as their reputation grows"))
        },
        int("contact_per_hour", 30, 1, 10_000, en("Contact form messages accepted per hour, site-wide")),
        SettingDef {
            default: json!(""),
            ..def("embed_hosts", String, false, en("Sites that may embed comments, as host names separated by commas"))
        },
        int(
            "embed_forum",
            0,
            0,
            1_000_000_000,
            "Public forum that holds embedded comment topics; without one, embedding is off",
        ),
        SettingDef {
            default: json!(""),
            ..def(
                "chat_webhook_url",
                String,
                false,
                en("Discord or Slack incoming webhook for new posts in public forums"),
            )
        },
        SettingDef {
            default: json!(""),
            ..def("akismet_key", String, false, en("Akismet API key; empty turns the spam check off"))
        },
        SettingDef {
            default: json!(""),
            ..def("ad_html", String, false, en("Advert shown under the header; scripts are removed"))
        },
        int("clubs_per_member", 3, 0, 1000, en("Clubs one member may start")),
        int("email_bounce_limit", 3, 1, 50, en("Mark an email address bad after this many bounces")),
        SettingDef {
            default: json!(true),
            ..def("allow_registration", Bool, true, en("Allow new accounts to register"))
        },
        SettingDef {
            default: json!(false),
            ..def("login_required", Bool, false, en("Require login to view any page"))
        },
    ]
}
