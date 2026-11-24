//! Share pictures: the look of the pictures drawn for links to pages (`bf_core::og`); and the link
//! previews that links in posts become.

use super::super::{SettingDef, SettingType::*};
use super::{def, int};
use crate::i18n::en;
use serde_json::json;

pub(super) fn defs() -> Vec<SettingDef> {
    let text = |key: &'static str, default: &str, description: &'static str| SettingDef {
        default: json!(default),
        ..def(key, String, false, description)
    };
    let flag = |key: &'static str, default: bool, description: &'static str| SettingDef {
        default: json!(default),
        ..def(key, Bool, false, description)
    };
    vec![
        // Link previews: the cards a bare link in a post becomes (`bf-server` `forum/onebox.rs`).
        flag(
            "onebox_enabled",
            true,
            en("A link alone on its line in a post becomes a card with the page's title, description and picture"),
        ),
        flag(
            "link_click_counts",
            true,
            en(
                "Show how many times a link in a post was clicked, as a count beside it (each visitor counts once a day; the post's author does not)",
            ),
        ),
        flag("onebox_images", true, en("Show the page's picture on a link preview card")),
        text(
            "onebox_blocked_hosts",
            "",
            en(
                "Sites that never get a link preview card, one per line or separated by spaces, like example.com (their subdomains too)",
            ),
        ),
        int("onebox_cache_days", 7, 1, 90, en("Days a link preview card is kept before the page is fetched again")),
        flag(
            "og_generate",
            false,
            en("Draw a share picture for pages that have none, from the page's title and the site's look"),
        ),
        text(
            "og_template",
            "classic",
            en(
                "Layout of the share picture: classic (text on the left), centered, or poster (a picture behind the text)",
            ),
        ),
        text(
            "og_kinds",
            "home,topic,forum,tag,profile,page,other",
            en(
                "Pages that get a drawn share picture: home, topic, forum, tag, profile, page and other, separated by commas",
            ),
        ),
        text("og_background_colour", "#111827", en("Share picture: background colour")),
        text(
            "og_background_colour_end",
            "",
            en("Share picture: second background colour for a gradient; empty keeps the first colour"),
        ),
        text("og_text_colour", "#ffffff", en("Share picture: text colour")),
        text("og_accent_colour", "#4f8cff", en("Share picture: accent colour of the stripe and the page label")),
        text(
            "og_background_image_url",
            "",
            en("Share picture: a picture behind the text, from the site's uploads (1200×630px shows best)"),
        ),
        int("og_overlay", 55, 0, 90, en("Share picture: how much the background picture is darkened, in percent")),
        flag("og_show_logo", true, en("Share picture: show the site's logo")),
        flag("og_show_site_name", true, en("Share picture: show the site's name")),
        flag("og_show_kind", true, en("Share picture: label the kind of page (Topic, Forum, Profile…)")),
        flag("og_show_description", true, en("Share picture: show the page's description under the title")),
        int("og_font_scale", 100, 70, 130, en("Share picture: text size in percent")),
        text("og_footer_text", "", en("Share picture: line at the bottom; empty shows the site's address")),
    ]
}
