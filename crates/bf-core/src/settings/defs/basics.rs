//! Identity and branding, size and rate limits, posting rules and tags.

use super::super::{SettingDef, SettingType::*};
use super::{def, int};
use crate::i18n::en;
use serde_json::json;

pub(super) fn defs() -> Vec<SettingDef> {
    vec![
        SettingDef {
            default: json!("Boring Forum"),
            ..def("site_name", String, true, en("Name shown in the header and page titles"))
        },
        SettingDef {
            default: json!("A calm place to talk."),
            ..def("site_description", String, true, en("Default meta description"))
        },
        // Branding (§ branding): pictures from the site's own uploads; empty uses the site name.
        SettingDef {
            default: json!(""),
            ..def(
                "logo_url",
                String,
                true,
                en("Logo in the header, in place of the site name; about 40px tall shows best"),
            )
        },
        SettingDef {
            default: json!(""),
            ..def("logo_dark_url", String, true, en("Logo for the dark colour scheme; empty uses the main logo"))
        },
        SettingDef {
            default: json!(""),
            ..def("logo_small_url", String, true, en("Square logo for narrow screens; empty uses the main logo"))
        },
        SettingDef {
            default: json!(""),
            ..def("favicon_url", String, true, en("Browser tab icon: a square PNG or SVG, at least 32px"))
        },
        SettingDef {
            default: json!(""),
            ..def(
                "large_icon_url",
                String,
                true,
                en("App icon for home screens and the installed app: a square PNG, 512px"),
            )
        },
        SettingDef {
            default: json!(""),
            ..def("og_image_url", String, true, en("Picture shown when a page is shared on social media, 1200×630px"))
        },
        int("posts_per_page", 20, 5, 100, en("Posts shown on one topic page")),
        int("topics_per_page", 30, 5, 100, en("Topics shown on one forum page")),
        int("min_post_length", 10, 1, 1000, en("Minimum characters in a post")),
        int("max_post_length", 32000, 100, 200_000, en("Maximum characters in a post")),
        int("min_title_length", 8, 1, 100, en("Minimum characters in a topic title")),
        int("max_title_length", 120, 10, 300, en("Maximum characters in a topic title")),
        SettingDef {
            default: json!(false),
            ..def("require_email_verification", Bool, true, en("Members must verify their email before posting"))
        },
        int("max_upload_kb", 4096, 64, 102_400, en("Largest file a member can upload, in KB")),
        SettingDef {
            default: json!("png,jpg,gif,webp,pdf,zip,txt"),
            ..def(
                "upload_allowed_types",
                String,
                false,
                en("Kinds of file members may upload to posts, messages and chat (png, jpg, gif, webp, pdf, zip, txt)"),
            )
        },
        int("upload_min_trust_level", 0, 0, 20, en("Trust level needed to upload files; staff always may")),
        SettingDef {
            default: json!(false),
            ..def(
                "uploads_members_only",
                Bool,
                false,
                en("Files in posts, messages and chat can only be opened by signed-in members"),
            )
        },
        SettingDef {
            default: json!("files"),
            ..def(
                "hotlink_protection",
                String,
                false,
                en(
                    "Other sites using your files: off, files (not pictures) or all (pictures too). Guests get links that expire",
                ),
            )
        },
        int(
            "upload_link_hours",
            6,
            1,
            168,
            en("How long a guest's link to a file stays good, in hours (one to two times this)"),
        ),
        SettingDef {
            default: json!(false),
            ..def(
                "gravatar_enabled",
                Bool,
                true,
                en("Members without an uploaded picture use their Gravatar, fetched through this server"),
            )
        },
        SettingDef {
            default: json!(false),
            ..def(
                "upload_strip_metadata",
                Bool,
                false,
                en(
                    "Remove all camera details from uploaded JPEGs, keeping only the orientation (location is always removed)",
                ),
            )
        },
        int("upload_daily_mb", 50, 1, 10_000, en("Upload allowance per member per day, in MB")),
        int("minimum_age", 0, 0, 30, en("Minimum age to register; 0 turns the age check off")),
        SettingDef {
            default: json!(false),
            ..def("require_account_approval", Bool, true, en("New accounts wait for staff approval before posting"))
        },
        int("edit_grace_seconds", 300, 0, 3600, en("Edits this soon after posting keep no revision")),
        int("min_register_seconds", 0, 0, 120, en("Reject sign-ups submitted faster than this (bots); 0 is off")),
        int("username_change_days", 30, 0, 3650, en("Days between username changes; 0 forbids changes")),
        int("username_change_min_trust_level", 0, 0, 20, en("Trust level needed to change your username")),
        int("flag_hide_score", 3, 1, 100, en("Hide a post from members once this many members report it")),
        SettingDef {
            default: json!(false),
            ..def("offline", Bool, true, en("Offline mode: only administrators can use the site"))
        },
        SettingDef {
            default: json!("We are doing some maintenance and will be back soon."),
            ..def("offline_message", String, true, en("Message shown while the site is offline"))
        },
        int(
            "edit_time_limit_minutes",
            0,
            0,
            525_600,
            en("Authors can edit for this long after posting; 0 is unlimited"),
        ),
        SettingDef {
            default: json!(false),
            ..def("require_edit_reason", Bool, true, en("Edits after the grace period need a reason"))
        },
        int("flood_wait_seconds", 0, 0, 3600, en("Minimum seconds between two posts by a member; 0 is off")),
        int("new_user_posts", 0, 0, 1000, en("Members with fewer posts than this count as new; 0 is off")),
        int("new_user_max_links", 2, 0, 100, en("Most links a new member may put in one post")),
        SettingDef {
            default: json!(false),
            ..def("block_disposable_emails", Bool, false, en("Refuse addresses from known temporary email services"))
        },
        SettingDef {
            default: json!(""),
            ..def(
                "blocked_email_domains",
                String,
                false,
                en("Comma-separated email domains refused at sign-up and on email change"),
            )
        },
        int("session_idle_minutes", 0, 0, 525_600, en("Sign members out after this long without activity; 0 is off")),
        SettingDef {
            default: json!("browser"),
            ..def(
                "default_locale",
                String,
                true,
                en(
                    "Language for visitors who have not picked one: browser (from their browser, English if we have none of theirs) or one language for all (en, pl, ar)",
                ),
            )
        },
        SettingDef {
            default: json!("classic"),
            ..def(
                "post_style",
                String,
                true,
                en("How posts look: classic (author column) or discourse (avatar beside the text)"),
            )
        },
        SettingDef {
            default: json!("list"),
            ..def(
                "forum_layout",
                String,
                true,
                en(
                    "How the forum index lists forums: list (rows with the latest post), boxes (cards), compact (one line each) or latest (forums beside the latest topics)",
                ),
            )
        },
        SettingDef {
            default: json!(true),
            ..def("recently_browsing", Bool, true, en("Show which members were on a page in the last 15 minutes"))
        },
        int("max_mentions_per_post", 10, 0, 100, en("Only this many mentions in a post send notifications")),
        SettingDef {
            default: json!(false),
            ..def("allow_magic_link", Bool, true, en("Let members sign in with a link sent by email"))
        },
        int("approve_first_posts", 0, 0, 50, en("Hold this many first posts of each new member for approval")),
        SettingDef {
            default: json!(false),
            ..def(
                "mailing_list_mode",
                Bool,
                true,
                en("Members may choose to get every new post by email (mailing-list mode)"),
            )
        },
        int(
            "topic_votes_per_member",
            10,
            0,
            1000,
            "Votes a member at trust level 0 or 1 can give to open topics in ideas forums; closed topics give theirs back; 0 is no limit",
        ),
        int("topic_votes_tl2", 15, 0, 1000, en("Votes for ideas at trust level 2; 0 is no limit")),
        int("topic_votes_tl3", 20, 0, 1000, en("Votes for ideas at trust level 3; 0 is no limit")),
        int("topic_votes_tl4", 25, 0, 1000, en("Votes for ideas at trust level 4; 0 is no limit")),
        int(
            "pm_min_trust_level",
            0,
            0,
            20,
            en("Trust level needed to start private messages; messages to staff are always allowed"),
        ),
        SettingDef {
            default: json!(""),
            ..def("nav_links", String, true, en("Header links as JSON; edit them on the Navigation page"))
        },
        SettingDef {
            default: json!(true),
            ..def("members_create_tags", Bool, true, en("Members may create new tags; otherwise only staff"))
        },
        int("max_tags_per_topic", 5, 0, 20, en("Most tags on one topic")),
        int("min_tags_per_topic", 0, 0, 10, en("Fewest tags a topic needs; 0 makes tags optional")),
        int("max_tag_length", 40, 2, 40, en("Longest tag name, in characters")),
        SettingDef {
            default: json!(false),
            ..def("force_lowercase_tags", Bool, true, en("Store every tag name in lowercase"))
        },
        SettingDef {
            default: json!(true),
            ..def(
                "allow_duplicate_topic_titles",
                Bool,
                true,
                en("Allow a new topic to reuse the title of an existing topic"),
            )
        },
        SettingDef {
            default: json!(true),
            ..def("allow_all_caps_titles", Bool, true, en("Allow topic titles written entirely in capital letters"))
        },
        int("max_links_per_post", 0, 0, 200, en("Most links in one post for members; 0 means no limit")),
        SettingDef {
            default: json!("all"),
            ..def(
                "nofollow_links",
                String,
                false,
                en("Which links in posts get rel=\"nofollow ugc\": all, external (only links to other sites) or none"),
            )
        },
        int(
            "nofollow_exempt_trust_level",
            5,
            0,
            5,
            en("Links in posts by members at or above this trust level are followed; 5 means no one"),
        ),
        SettingDef {
            default: json!(""),
            ..def(
                "nofollow_exempt_domains",
                String,
                false,
                en(
                    "Links to these sites (and their subdomains) are followed, whoever posts them: domains separated by spaces or commas",
                ),
            )
        },
        int(
            "min_first_post_length",
            0,
            0,
            1000,
            en("Minimum characters in a member's very first post; 0 uses the normal minimum"),
        ),
        int("max_quotes_per_post", 0, 0, 100, en("Most quotes in one post; 0 means no limit")),
        int(
            "new_user_max_media",
            0,
            0,
            50,
            en("Most images, videos and embeds in a new member's post; 0 means no limit"),
        ),
        int("max_emojis_in_title", 0, 0, 50, en("Most emoji in a topic title; 0 means no limit")),
        int("title_max_word_length", 0, 0, 200, en("Longest single word in a topic title; 0 means no limit")),
        int(
            "max_consecutive_replies",
            0,
            0,
            50,
            en("Replies one member may post in a row in a topic before someone else answers; 0 means no limit"),
        ),
        int("max_topics_per_day", 0, 0, 1000, en("Topics a member may start per day; 0 means no limit")),
        int("max_edits_per_day", 0, 0, 1000, en("Post edits a member may make per day; 0 means no limit")),
        int(
            "max_topics_in_first_day",
            0,
            0,
            100,
            en("Topics a member may start in their first 24 hours; 0 means no limit"),
        ),
        int(
            "max_replies_in_first_day",
            0,
            0,
            1000,
            en("Replies a member may post in their first 24 hours; 0 means no limit"),
        ),
        int(
            "auto_close_topics_post_count",
            0,
            0,
            100_000,
            en("Close a topic once it has this many replies; 0 means never"),
        ),
        int("min_password_length", 10, 8, 128, en("Shortest password allowed")),
        int(
            "password_unique_characters",
            0,
            0,
            32,
            en("Different characters a password must contain; 0 means no requirement"),
        ),
        int("min_username_length", 3, 3, 20, en("Shortest username allowed, from 3")),
        int("max_username_length", 20, 3, 20, en("Longest username allowed, up to 20")),
        SettingDef {
            default: json!(""),
            ..def(
                "reserved_usernames",
                String,
                false,
                en(
                    "Comma-separated usernames nobody may register; end a name with * to reserve every name that starts with it",
                ),
            )
        },
        SettingDef {
            default: json!(""),
            ..def(
                "allowed_email_domains",
                String,
                false,
                en("Comma-separated email domains; when set, only these may sign up or be used on an account"),
            )
        },
        SettingDef {
            default: json!("everyone"),
            ..def(
                "edit_history_visible_to",
                String,
                true,
                en("Who sees edit history: everyone, members, author or staff"),
            )
        },
        int(
            "edit_history_retention_days",
            0,
            0,
            36500,
            "Purge older revisions, keeping the first and last; 0 keeps all",
        ),
        SettingDef {
            default: json!("https://raw.githubusercontent.com/RikoDEV/boring-forum/main/VERSION"),
            ..def(
                "version_check_url",
                String,
                false,
                en("URL returning the latest version as text; empty turns the check off"),
            )
        },
        SettingDef {
            default: json!(false),
            ..def(
                "cloudflare_real_ip",
                Bool,
                false,
                en(
                    "Behind Cloudflare: take each visitor's address from the CF-Connecting-IP header. Believed only from Cloudflare's own addresses and from proxies in BF_TRUSTED_PROXIES",
                ),
            )
        },
        SettingDef { default: json!(false), ..def("invite_only", Bool, true, en("New accounts need an invite code")) },
        SettingDef {
            default: json!(""),
            ..def(
                "captcha_provider",
                String,
                true,
                en("Checks new sign-ups for bots. Stays off until the challenge secret is set"),
            )
        },
        SettingDef {
            default: json!(""),
            ..def("captcha_site_key", String, true, en("Public site key from Cloudflare, hCaptcha or Google"))
        },
        SettingDef {
            default: json!(""),
            ..def(
                "captcha_secret",
                String,
                false,
                en("Secret of the challenge: from the provider, or any long random text for ALTCHA"),
            )
        },
        SettingDef {
            default: json!(""),
            ..def(
                "cookie_consent_text",
                String,
                true,
                en("Consent banner text; leave empty unless you add non-essential cookies"),
            )
        },
        int("max_images_per_post", 50, 0, 1000, en("Most images in one post; a group limit overrides it")),
        int(
            "email_delay_minutes",
            0,
            0,
            1440,
            "Hold notification emails this long, and skip them if read in the app meanwhile",
        ),
        int(
            "deleted_retention_days",
            0,
            0,
            36500,
            en("Remove deleted content for good after this many days; 0 keeps it"),
        ),
    ]
}
