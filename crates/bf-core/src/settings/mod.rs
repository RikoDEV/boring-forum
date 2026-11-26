//! Site settings registry: a typed schema declared in code, values stored as JSON.
//!
//! The schema gives every key a type and a default, so storage only holds overrides
//! and an unknown or mistyped value can never reach the rest of the program.

mod defs;

use defs::builtin;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum SettingType {
    Bool,
    Int,
    String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SettingDef {
    pub key: &'static str,
    #[serde(rename = "type")]
    pub ty: SettingType,
    #[schema(value_type = Object)]
    pub default: Value,
    pub min: Option<i64>,
    pub max: Option<i64>,
    /// Public settings may be sent to guests' browsers; the rest are staff-only.
    pub public: bool,
    pub description: &'static str,
}

/// A setting that lists groups (`translator_groups`, `referral_reward_groups`): ids separated
/// by commas. Anything that is not a positive id is skipped.
pub fn group_ids(value: &str) -> Vec<i64> {
    let mut ids: Vec<i64> = value.split(',').filter_map(|g| g.trim().parse().ok()).filter(|g| *g > 0).collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum SettingError {
    #[error("unknown setting {0}")]
    Unknown(String),
    #[error("{key}: expected {expected:?}")]
    WrongType { key: String, expected: SettingType },
    #[error("{key}: must be between {min} and {max}")]
    OutOfRange { key: String, min: i64, max: i64 },
    #[error("{0}: choose a picture uploaded to this site")]
    NotAnUpload(String),
    #[error("{0}: choose from png, jpg, gif, webp, pdf, zip and txt, separated by commas")]
    NotFileTypes(String),
    #[error("{0}: choose pages from home, topic, forum, tag, profile, page and other, separated by commas")]
    NotPageKinds(String),
    #[error("{0}: write a colour as #rrggbb")]
    NotColour(String),
    #[error("{key}: choose one of {allowed}")]
    NotOneOf { key: String, allowed: String },
    #[error("{0}")]
    BadTrustLevels(&'static str),
}

/// Settings that hold a picture: a path in the site's own uploads, or empty. Only our uploads,
/// so a setting can never make every page load something from another host.
pub const BRANDING_IMAGES: [&str; 8] = [
    "logo_url",
    "logo_dark_url",
    "logo_small_url",
    "favicon_url",
    "large_icon_url",
    "og_image_url",
    "og_background_image_url",
    "mail_logo_url",
];

/// String settings that take one of a closed set of words (an empty string is a word where listed).
/// A value outside the set is refused when saved; the code that reads them treats anything else as
/// the strictest choice.
pub const CHOICES: [(&str, &[&str]); 12] = [
    ("og_template", &crate::og::TEMPLATES),
    ("edit_history_visible_to", &["everyone", "members", "author", "staff"]),
    ("hotlink_protection", &["off", "files", "all"]),
    ("ai_crawlers", &["allow", "block"]),
    ("nofollow_links", &["all", "external", "none"]),
    ("ai_spam_action", &["flag", "hold", "hide"]),
    ("post_style", &["classic", "discourse"]),
    ("forum_layout", &["list", "boxes", "compact", "latest"]),
    ("gif_content_filter", &["off", "low", "medium", "high"]),
    ("captcha_provider", &["", "altcha", "turnstile", "hcaptcha", "recaptcha"]),
    ("ai_api_style", &["openai", "anthropic", "mock"]),
    ("analytics_provider", &["", "plausible", "umami", "ga4", "matomo"]),
];

/// The kinds of file an upload can be, by the names an administrator lists them under.
pub const FILE_TYPES: [&str; 7] = ["png", "jpg", "gif", "webp", "pdf", "zip", "txt"];

/// Settings that list file types.
pub const FILE_TYPE_SETTINGS: [&str; 2] = ["upload_allowed_types", "chat_allowed_types"];

/// The known types in a comma-separated list, each once.
pub fn file_types(list: &str) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for want in list.split(',').map(str::trim) {
        if let Some(t) = FILE_TYPES.iter().find(|t| **t == want)
            && !out.contains(t)
        {
            out.push(t);
        }
    }
    out
}

/// Whether `v` is empty or a path in the uploads (`/uploads/ab12….png`), safe in an attribute.
pub fn upload_path_ok(v: &str) -> bool {
    v.is_empty()
        || (v.starts_with("/uploads/")
            && v.len() <= 200
            && !v.contains("..")
            && v.bytes().all(|c| c.is_ascii_alphanumeric() || b"/._-".contains(&c)))
}

/// The schema plus the current effective values. Cheap to share behind an `Arc`.
#[derive(Debug, Clone)]
pub struct Settings {
    defs: BTreeMap<&'static str, SettingDef>,
    values: BTreeMap<&'static str, Value>,
    /// The trust levels, parsed from the `trust_levels` setting whenever it changes.
    trust: crate::trust::Levels,
}

/// Settings that enabled plugins declare (`plugin:<id>:<key>`), set whenever plugins load. They
/// are part of every `Settings` built afterwards, so stored values and reloads keep them.
static EXTRA: std::sync::RwLock<Vec<SettingDef>> = std::sync::RwLock::new(Vec::new());

/// One plugin setting as declared: (full key, type, default, min, max, description).
pub type ExtraSetting = (String, SettingType, Value, Option<i64>, Option<i64>, String);

/// Replaces the plugins' settings. Strings are leaked: a few bytes per setting, and the same
/// setting is only leaked again if its text changed.
pub fn set_extra(list: Vec<ExtraSetting>) {
    static SEEN: std::sync::Mutex<Option<std::collections::HashMap<String, &'static str>>> =
        std::sync::Mutex::new(None);
    let mut seen = SEEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let seen = seen.get_or_insert_with(Default::default);
    let mut intern = |s: &str| -> &'static str {
        seen.entry(s.to_owned()).or_insert_with(|| Box::leak(s.to_owned().into_boxed_str()))
    };
    let defs = list
        .into_iter()
        .map(|(key, ty, default, min, max, description)| SettingDef {
            key: intern(&key),
            ty,
            default,
            min,
            max,
            public: false,
            description: intern(&description),
        })
        .collect();
    *EXTRA.write().unwrap_or_else(std::sync::PoisonError::into_inner) = defs;
}

impl Default for Settings {
    fn default() -> Self {
        let mut defs: BTreeMap<_, _> = builtin().into_iter().map(|d| (d.key, d)).collect();
        for d in EXTRA.read().unwrap_or_else(std::sync::PoisonError::into_inner).iter() {
            defs.entry(d.key).or_insert_with(|| d.clone());
        }
        let values = defs.values().map(|d| (d.key, d.default.clone())).collect();
        Self { defs, values, trust: crate::trust::Levels::default() }
    }
}

impl Settings {
    /// Builds settings from stored overrides. Invalid stored values are logged and
    /// replaced by defaults so a bad row can never stop the site from booting.
    pub fn from_overrides(overrides: impl IntoIterator<Item = (String, Value)>) -> Self {
        let mut s = Self::default();
        for (key, value) in overrides {
            if let Err(e) = s.set(&key, value) {
                tracing::warn!(error = %e, "ignoring stored setting");
            }
        }
        s
    }

    pub fn validate(&self, key: &str, value: &Value) -> Result<&SettingDef, SettingError> {
        let def = self.defs.get(key).ok_or_else(|| SettingError::Unknown(key.to_owned()))?;
        let wrong = || SettingError::WrongType { key: key.to_owned(), expected: def.ty };
        match def.ty {
            SettingType::Bool => value.as_bool().map(|_| ()).ok_or_else(wrong)?,
            SettingType::String => {
                let s = value.as_str().ok_or_else(wrong)?;
                if BRANDING_IMAGES.contains(&def.key) && !upload_path_ok(s) {
                    return Err(SettingError::NotAnUpload(key.to_owned()));
                }
                if let Some((_, allowed)) = CHOICES.iter().find(|(k, _)| *k == def.key)
                    && !allowed.contains(&s)
                {
                    let list: Vec<&str> = allowed.iter().copied().filter(|a| !a.is_empty()).collect();
                    return Err(SettingError::NotOneOf { key: key.to_owned(), allowed: list.join(", ") });
                }
                if def.key == "og_kinds" && !crate::og::kinds_ok(s) {
                    return Err(SettingError::NotPageKinds(key.to_owned()));
                }
                if crate::og::COLOUR_SETTINGS.contains(&def.key)
                    && !(s.is_empty() && def.key == "og_background_colour_end")
                    && !crate::og::colour_ok(s)
                {
                    return Err(SettingError::NotColour(key.to_owned()));
                }
                if def.key == "mail_accent_colour" && !crate::og::colour_ok(s) {
                    return Err(SettingError::NotColour(key.to_owned()));
                }
                if def.key == "trust_levels" {
                    crate::trust::Levels::parse(s).map_err(SettingError::BadTrustLevels)?;
                }
                if FILE_TYPE_SETTINGS.contains(&def.key)
                    && !s.split(',').map(str::trim).filter(|t| !t.is_empty()).all(|t| FILE_TYPES.contains(&t))
                {
                    return Err(SettingError::NotFileTypes(key.to_owned()));
                }
            }
            SettingType::Int => {
                let n = value.as_i64().ok_or_else(wrong)?;
                let (min, max) = (def.min.unwrap_or(i64::MIN), def.max.unwrap_or(i64::MAX));
                if n < min || n > max {
                    return Err(SettingError::OutOfRange { key: key.to_owned(), min, max });
                }
            }
        }
        Ok(def)
    }

    pub fn set(&mut self, key: &str, value: Value) -> Result<(), SettingError> {
        let key = self.validate(key, &value)?.key;
        if key == "trust_levels" {
            self.trust = crate::trust::Levels::parse(value.as_str().unwrap_or_default()).unwrap_or_default();
        }
        self.values.insert(key, value);
        Ok(())
    }

    /// The trust levels the site has.
    pub fn trust_levels(&self) -> &crate::trust::Levels {
        &self.trust
    }

    /// A lookup for the trust evaluator: a level's requirements by key, and the activity window.
    pub fn trust_value(&self) -> impl Fn(&str) -> i64 + '_ {
        move |key| if key == "trust_recent_days" { self.int(key) } else { self.trust.value(key) }
    }

    pub fn defs(&self) -> impl Iterator<Item = &SettingDef> {
        self.defs.values()
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.values.get(key)
    }

    // Typed getters: a key missing from the schema, or of another type, is a programmer error.
    // It fails loudly in tests and debug builds; a release build answers with the type's empty
    // value, because a panic here would take the whole server down on one request.
    pub fn str(&self, key: &str) -> &str {
        let found = self.values.get(key).and_then(Value::as_str);
        debug_assert!(found.is_some(), "no string setting {key}");
        found.unwrap_or_default()
    }

    pub fn int(&self, key: &str) -> i64 {
        let found = self.values.get(key).and_then(Value::as_i64);
        debug_assert!(found.is_some(), "no integer setting {key}");
        found.unwrap_or_default()
    }

    pub fn bool(&self, key: &str) -> bool {
        let found = self.values.get(key).and_then(Value::as_bool);
        debug_assert!(found.is_some(), "no boolean setting {key}");
        found.unwrap_or_default()
    }

    /// Values that differ from their defaults, for settings export.
    pub fn overrides(&self) -> BTreeMap<&'static str, Value> {
        self.defs
            .values()
            .filter(|d| self.values[d.key] != d.default)
            .map(|d| (d.key, self.values[d.key].clone()))
            .collect()
    }

    pub fn public_values(&self) -> BTreeMap<&'static str, Value> {
        self.defs.values().filter(|d| d.public).map(|d| (d.key, self.values[d.key].clone())).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_are_valid() {
        let s = Settings::default();
        for d in s.defs() {
            s.validate(d.key, &d.default).unwrap();
        }
        assert_eq!(s.int("posts_per_page"), 20);
    }

    #[test]
    fn rejects_bad_values() {
        let mut s = Settings::default();
        assert!(matches!(s.set("nope", json!(1)), Err(SettingError::Unknown(_))));
        assert!(matches!(s.set("posts_per_page", json!("x")), Err(SettingError::WrongType { .. })));
        assert!(matches!(s.set("posts_per_page", json!(1000)), Err(SettingError::OutOfRange { .. })));
        s.set("posts_per_page", json!(50)).unwrap();
        assert_eq!(s.int("posts_per_page"), 50);
    }

    #[test]
    fn bad_stored_override_falls_back_to_default() {
        let s = Settings::from_overrides([("site_name".into(), json!(5))]);
        assert_eq!(s.str("site_name"), "Boring Forum");
    }

    #[test]
    fn file_type_lists_are_checked() {
        let mut s = Settings::default();
        s.set("chat_allowed_types", json!("png, jpg")).unwrap();
        s.set("upload_allowed_types", json!("")).unwrap();
        assert!(matches!(s.set("chat_allowed_types", json!("png,exe")), Err(SettingError::NotFileTypes(_))));
        assert_eq!(file_types("png, jpg,png,exe,,zip"), vec!["png", "jpg", "zip"]);
    }

    #[test]
    fn trust_levels_follow_the_setting() {
        let mut s = Settings::default();
        assert_eq!(s.trust_levels().max_level(), 4);
        s.set("trust_levels", json!(r#"[{},{"auto":true,"posts":3}]"#)).unwrap();
        assert_eq!(s.trust_levels().max_level(), 1);
        assert_eq!(s.trust_value()("trust_level_1_posts"), 3);
        assert_eq!(s.trust_value()("trust_recent_days"), 100);
        assert!(matches!(s.set("trust_levels", json!("[]")), Err(SettingError::BadTrustLevels(_))));
        assert_eq!(s.trust_levels().max_level(), 1, "a refused value changes nothing");
    }

    #[test]
    fn private_settings_are_not_public() {
        assert!(!Settings::default().public_values().contains_key("login_required"));
    }
}
