//! Colour schemes (ACP → Colour schemes): a name and two palettes of base colours, built into one
//! stylesheet. Hover shades, soft backgrounds and borders are derived from the base colours with
//! `color-mix()`, so a scheme is eleven colours per mode and every shade follows them.
//!
//! The stylesheet is kept in memory under a hash-named address (cached forever), is linked from
//! every page after the built-in CSS, and is rebuilt whenever an admin changes a scheme.

use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::sync::{Arc, LazyLock, RwLock};

/// The base colours of a palette, as (key, design token, label), in the order the editor shows them.
pub const BASE: [(&str, &str, &str); 11] = [
    ("background", "bg", "Page background"),
    ("surface", "surface", "Cards and panels"),
    ("text", "text", "Text"),
    ("muted", "muted", "Secondary text"),
    ("border", "border", "Borders"),
    ("accent", "accent", "Links and buttons"),
    ("header_background", "header-bg", "Header background"),
    ("header_text", "header-text", "Header text"),
    ("success", "success", "Success"),
    ("warning", "warning", "Warning"),
    ("danger", "danger", "Danger"),
];

/// Tokens worked out from the base colours: (token, formula). The editor's preview uses the same
/// formulas, so what it shows is what the stylesheet says.
pub const DERIVED: [(&str, &str); 7] = [
    ("surface-2", "color-mix(in srgb, var(--bf-color-surface) 94%, var(--bf-color-text))"),
    ("border-strong", "color-mix(in srgb, var(--bf-color-border) 82%, var(--bf-color-text))"),
    ("accent-hover", "color-mix(in srgb, var(--bf-color-accent) 82%, var(--bf-color-text))"),
    ("accent-soft", "color-mix(in srgb, var(--bf-color-accent) 12%, var(--bf-color-surface))"),
    ("success-soft", "color-mix(in srgb, var(--bf-color-success) 13%, var(--bf-color-surface))"),
    ("warning-soft", "color-mix(in srgb, var(--bf-color-warning) 13%, var(--bf-color-surface))"),
    ("danger-soft", "color-mix(in srgb, var(--bf-color-danger) 13%, var(--bf-color-surface))"),
];

/// One scheme as the page needs it.
#[derive(Debug, Clone, Serialize)]
pub struct Choice {
    pub slug: String,
    pub name: String,
}

#[derive(Default)]
struct Built {
    css: String,
    url: String,
    choices: Vec<Choice>,
    /// (address name, light palette, dark palette) of every scheme.
    palettes: Vec<(String, Value, Value)>,
    /// The schemes whose light and dark palettes the site shows by default.
    light: String,
    dark: String,
}

static CURRENT: LazyLock<RwLock<Arc<Built>>> = LazyLock::new(Default::default);

fn current() -> Arc<Built> {
    CURRENT.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

/// A scheme as stored.
pub struct Source<'a> {
    pub slug: &'a str,
    pub name: &'a str,
    pub light: &'a Value,
    pub dark: &'a Value,
    pub selectable: bool,
}

pub fn is_hex(c: &str) -> bool {
    c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|c| c.is_ascii_hexdigit())
}

/// Checks a palette from the editor and returns it cleaned: every base colour present as a
/// lower-case `#rrggbb`, nothing else.
pub fn clean_palette(v: &Value) -> Result<Value, String> {
    let m = v.as_object().ok_or("A palette must be an object of colours.")?;
    let mut out = Map::new();
    for (key, _, label) in BASE {
        let c = m.get(key).and_then(Value::as_str).unwrap_or("").trim().to_ascii_lowercase();
        if !is_hex(&c) {
            return Err(format!("{label} must be a colour like #1a2b3c."));
        }
        out.insert(key.to_owned(), Value::String(c));
    }
    Ok(Value::Object(out))
}

fn channel(c: &str, i: usize) -> f64 {
    let v = f64::from(u8::from_str_radix(&c[i..i + 2], 16).unwrap_or(0)) / 255.0;
    if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

fn luminance(c: &str) -> f64 {
    0.2126 * channel(c, 1) + 0.7152 * channel(c, 3) + 0.0722 * channel(c, 5)
}

/// WCAG contrast ratio of two `#rrggbb` colours.
pub fn contrast(a: &str, b: &str) -> f64 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// Text that reads on a button of this colour: white or near-black, whichever contrasts more.
pub fn on(accent: &str) -> &'static str {
    if contrast(accent, "#ffffff") >= contrast(accent, "#0b0f14") { "#ffffff" } else { "#0b0f14" }
}

fn get<'a>(m: &'a Value, key: &str) -> &'a str {
    m.get(key).and_then(Value::as_str).filter(|c| is_hex(c)).unwrap_or("#808080")
}

/// The declarations of a light and a dark palette: every base colour as `light-dark(light, dark)`,
/// then the derived ones. The values were checked when saved, and are checked again here. The two
/// palettes may come from different schemes: that is how a site shows one scheme in light mode
/// and another in dark.
pub fn declarations(light: &Value, dark: &Value) -> String {
    let mut css = String::new();
    for (key, token, _) in BASE {
        css.push_str(&format!("--bf-color-{token}:light-dark({},{});", get(light, key), get(dark, key)));
    }
    css.push_str(&format!(
        "--bf-color-accent-text:light-dark({},{});--bf-color-focus:var(--bf-color-accent);",
        on(get(light, "accent")),
        on(get(dark, "accent"))
    ));
    for (token, formula) in DERIVED {
        css.push_str(&format!("--bf-color-{token}:{formula};"));
    }
    css
}

/// The stylesheet for a set of schemes. The default light and dark palettes also apply to `:root`,
/// so everyone who has not picked a scheme gets them.
pub fn render(list: &[Source], light: &str, dark: &str) -> String {
    let mut css = String::new();
    if let (Some(l), Some(d)) = (list.iter().find(|s| s.slug == light), list.iter().find(|s| s.slug == dark)) {
        css.push_str(&format!(":root{{{}}}\n", declarations(l.light, d.dark)));
    }
    for s in list {
        css.push_str(&format!(":root[data-scheme=\"{0}\"],[data-scheme=\"{0}\"]{{{1}}}\n", s.slug, declarations(s.light, s.dark)));
    }
    css
}

/// Rebuilds what pages use from the stored schemes and the slugs of the default light and dark ones.
pub fn install(list: &[Source], light: &str, dark: &str) {
    let css = render(list, light, dark);
    let hash: String = Sha256::digest(css.as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect();
    let built = Built {
        url: format!("/scheme/{hash}.css"),
        choices: list
            .iter()
            .filter(|s| s.selectable || s.slug == light || s.slug == dark)
            .map(|s| Choice { slug: s.slug.to_owned(), name: s.name.to_owned() })
            .collect(),
        palettes: list.iter().map(|s| (s.slug.to_owned(), s.light.clone(), s.dark.clone())).collect(),
        light: light.to_owned(),
        dark: dark.to_owned(),
        css,
    };
    *CURRENT.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(built);
}

/// The palettes a theme shows: the light palette of the scheme chosen for light mode and the dark
/// palette of the one chosen for dark, each falling back to the site's default. None when the
/// theme chose neither, so it follows the site.
pub fn pair(light: Option<&str>, dark: Option<&str>) -> Option<(Value, Value)> {
    if light.is_none() && dark.is_none() {
        return None;
    }
    let c = current();
    let find = |slug: &str| c.palettes.iter().find(|(s, ..)| s == slug);
    let l = find(light.unwrap_or(&c.light))?;
    let d = find(dark.unwrap_or(&c.dark))?;
    Some((l.1.clone(), d.2.clone()))
}

/// The stylesheet's address, for the page's `<link>`; None while no scheme is stored.
pub fn url() -> Option<String> {
    let c = current();
    (!c.css.is_empty()).then(|| c.url.clone())
}

/// The stylesheet at this address, if it is the current one.
pub fn stylesheet(path: &str) -> Option<String> {
    let c = current();
    (c.url == path).then(|| c.css.clone())
}

/// Schemes a member can pick, and the slugs of the site's default light and dark ones.
pub fn choices() -> (Vec<Choice>, String, String) {
    let c = current();
    (c.choices.clone(), c.light.clone(), c.dark.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn palette(accent: &str) -> Value {
        json!({"background":"#f5f6f8","surface":"#ffffff","text":"#1a1f27","muted":"#596371","border":"#e1e4e9","accent":accent,
               "header_background":"#ffffff","header_text":"#1a1f27","success":"#17693c","warning":"#8a5300","danger":"#b42318"})
    }

    #[test]
    fn a_palette_needs_every_colour_as_hex() {
        assert!(clean_palette(&palette("#0B64C0")).unwrap()["accent"] == "#0b64c0");
        assert!(clean_palette(&palette("red")).is_err());
        assert!(clean_palette(&palette("#fff")).is_err());
        assert!(clean_palette(&palette("#0b64c0;}body{display:none")).is_err());
        let mut missing = palette("#0b64c0");
        missing.as_object_mut().unwrap().remove("danger");
        assert!(clean_palette(&missing).is_err());
    }

    #[test]
    fn button_text_follows_the_button() {
        assert_eq!(on("#0b64c0"), "#ffffff");
        assert_eq!(on("#6fb1f5"), "#0b0f14");
        assert_eq!(on("#ffffff"), "#0b0f14");
        assert!(contrast("#000000", "#ffffff") > 20.0);
    }

    #[test]
    fn the_default_scheme_reaches_everyone_and_each_one_its_chooser() {
        let (a, b) = (palette("#0b64c0"), palette("#17693c"));
        let list = [
            Source { slug: "standard", name: "Standard", light: &a, dark: &a, selectable: true },
            Source { slug: "forest", name: "Forest", light: &b, dark: &b, selectable: false },
        ];
        let css = render(&list, "forest", "forest");
        assert!(css.starts_with(":root{--bf-color-bg:light-dark(#f5f6f8,#f5f6f8);"), "{css}");
        assert!(css.contains("[data-scheme=\"standard\"]") && css.contains("[data-scheme=\"forest\"]"));
        assert!(css.contains("--bf-color-accent:light-dark(#17693c,#17693c)"));
        assert!(css.contains("--bf-color-accent-soft:color-mix("));
        // Light mode can show one scheme and dark mode another.
        let mixed = render(&list, "standard", "forest");
        assert!(mixed.starts_with(":root{") && mixed.contains("--bf-color-accent:light-dark(#0b64c0,#17693c)"), "{mixed}");
        install(&list, "forest", "forest");
        let (choices, default, _) = choices();
        assert_eq!(default, "forest");
        let (l, d) = pair(Some("standard"), None).unwrap();
        assert_eq!((l["accent"].as_str(), d["accent"].as_str()), (Some("#0b64c0"), Some("#17693c")));
        assert!(pair(None, None).is_none() && pair(Some("missing"), None).is_none());
        // A scheme that is not for members to pick is still listed while it is the default.
        assert_eq!(choices.len(), 2);
        assert!(url().unwrap().starts_with("/scheme/") && stylesheet(&url().unwrap()).is_some());
    }
}
