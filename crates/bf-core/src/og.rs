//! Share pictures: the picture a link to a page shows on social networks and in chats, drawn by
//! the server (`bf-server`, `/og.png`) from the page's title and the site's look.
//!
//! This module holds what the page and the renderer must agree on: which pages (`kind_of`), the
//! look (`Style`, from the `og_*` settings) and the address of one picture. An address carries
//! the text to draw and a signature over it, so only text a page of this site really had can be
//! drawn: the endpoint cannot be used to put words of someone else's choosing on our domain, nor
//! as a free renderer.

use crate::settings::Settings;
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::sync::LazyLock;

/// The kinds of page a picture can be switched on for, as the `og_kinds` setting lists them.
pub const KINDS: [&str; 7] = ["home", "topic", "forum", "tag", "profile", "page", "other"];

/// The looks the renderer knows (`og_template`).
pub const TEMPLATES: [&str; 3] = ["classic", "centered", "poster"];

/// Settings that hold a colour written `#rrggbb`.
pub const COLOUR_SETTINGS: [&str; 4] = ["og_background_colour", "og_background_colour_end", "og_text_colour", "og_accent_colour"];

/// The longest text drawn, in characters; longer text is cut when the address is made.
pub const TITLE_MAX: usize = 110;
pub const DESCRIPTION_MAX: usize = 160;

/// Which kind of page a path (or an absolute address) is.
pub fn kind_of(address: &str) -> &'static str {
    let path = address.split_once("://").map_or(address, |(_, rest)| rest.find('/').map_or("/", |i| &rest[i..]));
    let path = path.split(['?', '#']).next().unwrap_or("/");
    let first = path.trim_start_matches('/').split('/').next().unwrap_or("");
    match first {
        "" => "home",
        "t" => "topic",
        "c" => "forum",
        "tag" => "tag",
        "u" => "profile",
        "page" => "page",
        _ => "other",
    }
}

/// Whether a list setting names this kind. An empty list names none.
pub fn kind_listed(list: &str, kind: &str) -> bool {
    list.split(',').any(|k| k.trim() == kind)
}

/// Whether every entry is a known kind; an empty list is fine.
pub fn kinds_ok(list: &str) -> bool {
    list.split(',').map(str::trim).filter(|k| !k.is_empty()).all(|k| KINDS.contains(&k))
}

/// `#rrggbb`, in either case.
pub fn colour_ok(c: &str) -> bool {
    c.len() == 7 && c.starts_with('#') && c[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// The look of the pictures, read from the settings. A colour that is not `#rrggbb` (the settings
/// refuse one, but a stored value may predate a rule) falls back to the default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Style {
    pub enabled: bool,
    pub template: String,
    pub kinds: String,
    pub background: String,
    pub background_end: String,
    pub text: String,
    pub accent: String,
    pub background_image: String,
    pub overlay: u8,
    pub logo: bool,
    pub site_name: bool,
    pub kind_label: bool,
    pub description: bool,
    pub font_scale: u16,
    pub footer: String,
}

impl Style {
    pub fn from_settings(s: &Settings) -> Self {
        let colour = |key: &str, fallback: &str| {
            let v = s.str(key).trim();
            if colour_ok(v) { v.to_ascii_lowercase() } else { fallback.to_owned() }
        };
        let template = s.str("og_template");
        Self {
            enabled: s.bool("og_generate"),
            template: if TEMPLATES.contains(&template) { template.to_owned() } else { "classic".to_owned() },
            kinds: s.str("og_kinds").to_owned(),
            background: colour("og_background_colour", "#111827"),
            background_end: colour("og_background_colour_end", ""),
            text: colour("og_text_colour", "#ffffff"),
            accent: colour("og_accent_colour", "#4f8cff"),
            background_image: s.str("og_background_image_url").to_owned(),
            overlay: u8::try_from(s.int("og_overlay").clamp(0, 90)).unwrap_or(55),
            logo: s.bool("og_show_logo"),
            site_name: s.bool("og_show_site_name"),
            kind_label: s.bool("og_show_kind"),
            description: s.bool("og_show_description"),
            font_scale: u16::try_from(s.int("og_font_scale").clamp(70, 130)).unwrap_or(100),
            footer: s.str("og_footer_text").trim().chars().take(60).collect(),
        }
    }

    /// Whether pages of this kind get a generated picture.
    pub fn covers(&self, kind: &str) -> bool {
        self.enabled && kind_listed(&self.kinds, kind)
    }

    /// The file type of the pictures: a photograph behind the text is far smaller as JPEG (chat
    /// apps refuse or slow down on large pictures), flat colour is crisper and smaller as PNG.
    pub fn extension(&self) -> &'static str {
        if self.template == "poster" && !self.background_image.is_empty() { "jpg" } else { "png" }
    }

    /// Eight hex digits that change whenever the look does, so a changed look is a new address and
    /// the networks that keep the old picture fetch the new one.
    pub fn version(&self) -> String {
        let mut h = Sha256::new();
        h.update(format!("{self:?}").as_bytes());
        h.finalize().iter().take(4).fold(String::new(), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
    }
}

/// The key addresses are signed with, derived from `BF_SECRET_KEY`. An https site cannot start
/// without that key (`Config::secret_key_error`), so on a production site addresses always stay
/// valid across restarts. Only plain-http development, or a site that opted out with
/// `BF_ALLOW_NO_SECRET_KEY=1`, gets a key made per run.
static KEY: LazyLock<Vec<u8>> = LazyLock::new(|| {
    match std::env::var("BF_SECRET_KEY") {
        Ok(v) if !v.trim().is_empty() => Sha256::digest(format!("bf-og-image:{v}")).to_vec(),
        _ => Sha256::digest(format!("bf-og-image:{}", crate::auth::random_token())).to_vec(),
    }
});

#[expect(clippy::expect_used, reason = "HMAC accepts a key of any length")]
fn mac(kind: &str, title: &str, description: &str) -> String {
    use hmac::{Hmac, KeyInit, Mac};
    let mut m = Hmac::<Sha256>::new_from_slice(&KEY).expect("any key length");
    for part in [kind, title, description] {
        // Lengths, not separators: no two texts can run into the same message.
        m.update(&(part.len() as u64).to_be_bytes());
        m.update(part.as_bytes());
    }
    m.finalize().into_bytes().iter().take(12).fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn clip(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    flat.chars().take(max).collect()
}

fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// The text of a picture as it is drawn and signed: whitespace folded, cut to length.
pub fn text_of(title: &str, description: &str) -> (String, String) {
    (clip(title, TITLE_MAX), clip(description, DESCRIPTION_MAX))
}

/// The address (path and query) of the picture for a page of this kind with this title and
/// description, in the look `version` names; `ext` is the style's `extension`.
pub fn picture_path(kind: &str, title: &str, description: &str, version: &str, ext: &str) -> String {
    let (title, description) = text_of(title, description);
    let sig = mac(kind, &title, &description);
    let mut out = format!("/og.{ext}?k={kind}&t={}", encode(&title));
    if !description.is_empty() {
        let _ = write!(out, "&d={}", encode(&description));
    }
    let _ = write!(out, "&v={version}&s={sig}");
    out
}

/// Whether `sig` is the signature of exactly this text, and the kind is a known one.
pub fn verify(kind: &str, title: &str, description: &str, sig: &str) -> bool {
    if !KINDS.contains(&kind) || title.chars().count() > TITLE_MAX || description.chars().count() > DESCRIPTION_MAX {
        return false;
    }
    let want = mac(kind, title, description);
    // Compared without stopping at the first difference.
    want.len() == sig.len() && want.bytes().zip(sig.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_are_told_apart_by_their_path() {
        assert_eq!(kind_of("/"), "home");
        assert_eq!(kind_of("https://forum.example"), "home");
        assert_eq!(kind_of("https://forum.example/t/hello/12?page=2"), "topic");
        assert_eq!(kind_of("/c/news/3"), "forum");
        assert_eq!(kind_of("/tag/rust"), "tag");
        assert_eq!(kind_of("/u/ann"), "profile");
        assert_eq!(kind_of("/page/about"), "page");
        assert_eq!(kind_of("/latest"), "other");
    }

    #[test]
    fn lists_and_colours_are_checked() {
        assert!(kinds_ok("") && kinds_ok("home, topic") && !kinds_ok("home,admin"));
        assert!(kind_listed("home,topic", "topic") && !kind_listed("home,topic", "tag") && !kind_listed("", "home"));
        assert!(colour_ok("#0aF09c") && !colour_ok("0af09c") && !colour_ok("#fff") && !colour_ok("#12345g"));
    }

    #[test]
    fn only_signed_text_verifies() {
        let path = picture_path("topic", "Hello  world", "A\nB", "abcd1234", "png");
        let query: std::collections::HashMap<&str, &str> =
            path.split_once('?').unwrap().1.split('&').filter_map(|p| p.split_once('=')).collect();
        assert_eq!(query["t"], "Hello%20world");
        let sig = query["s"];
        assert!(verify("topic", "Hello world", "A B", sig));
        assert!(!verify("topic", "Hello world!", "A B", sig), "changed title");
        assert!(!verify("topic", "Hello world", "A B C", sig), "changed description");
        assert!(!verify("forum", "Hello world", "A B", sig), "changed kind");
        assert!(!verify("topic", "Hello world", "A B", "0000"), "wrong signature");
        // Text that moves a word from one field to the other is not the same message.
        assert_ne!(mac("topic", "ab", "c"), mac("topic", "a", "bc"));
    }

    #[test]
    fn text_is_folded_and_cut() {
        let (t, d) = text_of(&format!("  a\n\tb {}", "x".repeat(500)), "");
        assert!(t.starts_with("a b x") && t.chars().count() == TITLE_MAX && d.is_empty());
    }

    #[test]
    fn the_settings_refuse_what_the_renderer_cannot_use() {
        use crate::settings::{SettingError, Settings};
        use serde_json::json;
        let mut s = Settings::default();
        assert!(s.set("og_kinds", json!("home,topic")).is_ok() && s.set("og_kinds", json!("")).is_ok());
        assert!(matches!(s.set("og_kinds", json!("home,admin")), Err(SettingError::NotPageKinds(_))));
        assert!(s.set("og_background_colour", json!("#0a0b0c")).is_ok());
        assert!(matches!(s.set("og_background_colour", json!("red")), Err(SettingError::NotColour(_))));
        assert!(matches!(s.set("og_text_colour", json!("")), Err(SettingError::NotColour(_))), "text needs a colour");
        assert!(s.set("og_background_colour_end", json!("")).is_ok(), "no gradient is allowed");
        assert!(matches!(s.set("og_template", json!("fancy")), Err(SettingError::NotOneOf { .. })));
        assert!(matches!(s.set("og_background_image_url", json!("https://evil.example/x.png")), Err(SettingError::NotAnUpload(_))));
        assert!(matches!(s.set("og_overlay", json!(95)), Err(SettingError::OutOfRange { .. })));
        // A look read from the settings covers only the kinds listed, and only when switched on.
        let mut look = Style::from_settings(&s);
        assert!(!look.covers("topic"), "off by default");
        look.enabled = true;
        assert!(look.covers("home") && !look.covers("tag") || look.kinds.is_empty());
        let before = look.version();
        look.accent = "#ff0000".into();
        assert_ne!(before, look.version(), "a changed look is a new address");
    }
}
