//! The built-in settings schema, declared by area. The registry keys them by name, so file
//! order carries no meaning.

mod ai;
mod basics;
mod chat;
mod integrations;
mod logins;
mod policy;
mod security;
mod share;
mod spam;

use super::{SettingDef, SettingType};
use serde_json::{Value, json};

const fn def(key: &'static str, ty: SettingType, public: bool, description: &'static str) -> SettingDef {
    SettingDef { key, ty, default: Value::Null, min: None, max: None, public, description }
}

/// Numbers are staff-only: they are mostly anti-abuse thresholds and limits. No script or
/// template reads one from the public settings; a setting that must be public uses `def` directly.
fn int(key: &'static str, default: i64, min: i64, max: i64, description: &'static str) -> SettingDef {
    SettingDef {
        default: json!(default),
        min: Some(min),
        max: Some(max),
        ..def(key, SettingType::Int, false, description)
    }
}

/// Every built-in setting, gathered from the per-area files.
pub(super) fn builtin() -> Vec<SettingDef> {
    [
        basics::defs(),
        chat::defs(),
        policy::defs(),
        security::defs(),
        share::defs(),
        integrations::defs(),
        logins::defs(),
        ai::defs(),
        spam::defs(),
    ]
    .concat()
}
