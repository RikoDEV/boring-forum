//! Typed forms a plugin declares: its `settings.json`, `[[admin_pages]]` and the settings of a
//! `[[widgets]]` kind. One field description serves all three, and the admin panel draws it with
//! the same controls as its own pages.
//!
//! A field is `{key, type, label, label_key, help, help_key, default, options, min, max, rows}`.
//! `type` is `text`, `textarea`, `number`, `switch`, `select`, `url`, `email`, `date` or `color`;
//! `options` (for `select`) are `{value, label, label_key}`. `settings.json` may be written as a
//! JSON Schema instead (`properties` with `type`, `title`, `description`, `default`, `enum`,
//! `minimum`, `maximum`), with `title_key`, `description_key` and `enum_label_keys` for
//! translations; [`settings_fields`] turns either into the list above.

use bf_core::i18n::en;
use serde_json::{Map, Value, json};

const MAX_FIELDS: usize = 40;
const MAX_TEXT: usize = 10_000;

pub(super) const BAD_NUMBER: &str = en("{key} must be a number in its allowed range.");
pub(super) const BAD_CHOICE: &str = en("{key} must be one of the listed choices.");
pub(super) const BAD_TEXT: &str = en("{key} must be text of up to 10000 characters.");
pub(super) const BAD_SWITCH: &str = en("{key} must be on or off.");
pub(super) const BAD_LINK: &str = en("{key} must be a web address or a path on this site.");

fn good_key(k: &str) -> bool {
    !k.is_empty() && k.len() <= 40 && k.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
}

fn text(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or_default().chars().take(300).collect()
}

fn opt_text(v: &Value, k: &str) -> Value {
    v.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map_or(Value::Null, |s| json!(s))
}

fn options(v: &Value) -> Vec<Value> {
    let Some(list) = v.get("options").or_else(|| v.get("enum")).and_then(Value::as_array) else { return Vec::new() };
    let keys = v.get("enum_label_keys").and_then(Value::as_object);
    let labels = v.get("enum_labels").and_then(Value::as_object);
    list.iter()
        .take(100)
        .filter_map(|o| {
            if let Some(s) = o.as_str() {
                let label = labels.and_then(|m| m.get(s)).and_then(Value::as_str).unwrap_or(s);
                let key = keys.and_then(|m| m.get(s)).cloned().unwrap_or(Value::Null);
                return Some(json!({ "value": s, "label": label, "label_key": key }));
            }
            let value = o.get("value").and_then(Value::as_str)?;
            let label = o.get("label").and_then(Value::as_str).unwrap_or(value);
            Some(json!({ "value": value, "label": label, "label_key": opt_text(o, "label_key") }))
        })
        .collect()
}

/// One field from either style, or `None` when it is not a usable field.
fn field(key: &str, v: &Value) -> Option<Value> {
    if !good_key(key) {
        return None;
    }
    let declared = v.get("type").and_then(Value::as_str).unwrap_or("text");
    let opts = options(v);
    let ty = match (declared, v.get("format").and_then(Value::as_str)) {
        _ if !opts.is_empty() => "select",
        ("boolean" | "switch", _) => "switch",
        ("integer" | "number", _) => "number",
        ("textarea", _) | (_, Some("textarea")) => "textarea",
        ("url", _) | (_, Some("uri" | "url")) => "url",
        ("email", _) | (_, Some("email")) => "email",
        ("date", _) | (_, Some("date")) => "date",
        ("color", _) | (_, Some("color")) => "color",
        ("string" | "text", _) => "text",
        _ => return None,
    };
    let label = v.get("label").or_else(|| v.get("title")).and_then(Value::as_str).unwrap_or(key);
    let label_key = v.get("label_key").or_else(|| v.get("title_key"));
    let help = v.get("help").or_else(|| v.get("description")).and_then(Value::as_str).unwrap_or_default();
    let help_key = v.get("help_key").or_else(|| v.get("description_key"));
    Some(json!({
        "key": key, "type": ty, "label": label.chars().take(120).collect::<String>(),
        "label_key": label_key.and_then(Value::as_str).filter(|s| !s.is_empty()),
        "help": help.chars().take(300).collect::<String>(),
        "help_key": help_key.and_then(Value::as_str).filter(|s| !s.is_empty()),
        "default": v.get("default").cloned().unwrap_or(Value::Null),
        "options": opts,
        "min": v.get("min").or_else(|| v.get("minimum")).cloned().unwrap_or(Value::Null),
        "max": v.get("max").or_else(|| v.get("maximum")).cloned().unwrap_or(Value::Null),
        "rows": v.get("rows").and_then(Value::as_u64).unwrap_or(if ty == "textarea" { 4 } else { 0 }).min(30),
    }))
}

/// Normalises a manifest's `fields = [...]`: each entry has a `key`.
pub(super) fn declared(fields: &[Value]) -> Result<Vec<Value>, String> {
    if fields.len() > MAX_FIELDS {
        return Err(en("Too many form fields.").into());
    }
    let mut out: Vec<Value> = Vec::new();
    for f in fields {
        let key = text(f, "key");
        let Some(n) = field(&key, f) else { return Err(format!("Bad form field {key}.")) };
        if out.iter().any(|o| o["key"] == n["key"]) {
            return Err(format!("Form field {key} is declared twice."));
        }
        out.push(n);
    }
    Ok(out)
}

/// The fields of a `settings.json`, whichever style it is written in.
pub(super) fn settings_fields(schema: &Value) -> Vec<Value> {
    if let Some(list) = schema.get("fields").and_then(Value::as_array) {
        return declared(list).unwrap_or_default();
    }
    let Some(props) = schema.get("properties").and_then(Value::as_object) else { return Vec::new() };
    props.iter().take(MAX_FIELDS).filter_map(|(k, v)| field(k, v)).collect()
}

/// Checks and coerces a saved object against `fields`. Keys that are not fields are kept as they
/// are (older settings, or ones the plugin manages itself). The error is the message template
/// and the key it is about.
pub(super) fn validate(fields: &[Value], values: &Value) -> Result<Value, (&'static str, String)> {
    let mut out: Map<String, Value> = values.as_object().cloned().unwrap_or_default();
    for f in fields {
        let key = f["key"].as_str().unwrap_or_default();
        let Some(v) = out.get(key).cloned() else { continue };
        let bad = |t: &'static str| (t, key.to_owned());
        let clean = match f["type"].as_str().unwrap_or("text") {
            "switch" => Value::Bool(v.as_bool().ok_or_else(|| bad(BAD_SWITCH))?),
            "number" => {
                let n = v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())).ok_or_else(|| bad(BAD_NUMBER))?;
                let low = f["min"].as_f64().unwrap_or(f64::MIN);
                let high = f["max"].as_f64().unwrap_or(f64::MAX);
                if !n.is_finite() || n < low || n > high {
                    return Err(bad(BAD_NUMBER));
                }
                if n.fract() == 0.0 && n.abs() < 9e15 { json!(n as i64) } else { json!(n) }
            }
            "select" => {
                let s = v.as_str().ok_or_else(|| bad(BAD_CHOICE))?;
                if !f["options"].as_array().is_some_and(|o| o.iter().any(|o| o["value"] == s)) {
                    return Err(bad(BAD_CHOICE));
                }
                v
            }
            ty => {
                let s = v.as_str().ok_or_else(|| bad(BAD_TEXT))?;
                if s.chars().count() > MAX_TEXT {
                    return Err(bad(BAD_TEXT));
                }
                let link = !s.is_empty() && (s.starts_with("https://") || s.starts_with("http://") || super::local_path(s));
                if ty == "url" && !s.is_empty() && !link {
                    return Err(bad(BAD_LINK));
                }
                v
            }
        };
        out.insert(key.to_owned(), clean);
    }
    Ok(Value::Object(out))
}

/// Saved values with the schema's defaults under them, so a plugin sees a value for every field
/// even before an administrator has saved anything.
pub(super) fn with_defaults(saved: Value, schema: Option<&Value>) -> Value {
    let Some(schema) = schema else { return saved };
    let mut all = saved.as_object().cloned().unwrap_or_default();
    let pages = schema.get("x-admin-pages").and_then(Value::as_array).into_iter().flatten();
    let page_fields = pages.flat_map(|p| p.get("fields").and_then(Value::as_array).cloned().unwrap_or_default());
    for f in settings_fields(schema).into_iter().chain(page_fields) {
        let key = f["key"].as_str().unwrap_or_default();
        if !all.contains_key(key) && !f["default"].is_null() {
            all.insert(key.to_owned(), f["default"].clone());
        }
    }
    Value::Object(all)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_schema_properties_become_fields() {
        let schema = json!({ "properties": {
            "mode": { "type": "string", "enum": ["a", "b"], "title": "Mode", "title_key": "k.mode", "default": "a" },
            "n": { "type": "integer", "minimum": 1, "maximum": 5 },
            "on": { "type": "boolean", "default": true },
            "bio": { "type": "string", "format": "textarea" },
        } });
        let f = settings_fields(&schema);
        let ty = |k: &str| f.iter().find(|x| x["key"] == k).map(|x| x["type"].clone());
        assert_eq!(ty("mode"), Some(json!("select")));
        assert_eq!(ty("n"), Some(json!("number")));
        assert_eq!(ty("on"), Some(json!("switch")));
        assert_eq!(ty("bio"), Some(json!("textarea")));
        assert_eq!(f.iter().find(|x| x["key"] == "mode").unwrap()["label_key"], "k.mode");
    }

    #[test]
    fn values_are_checked_and_defaults_fill_in() {
        let schema = json!({ "properties": {
            "mode": { "enum": ["a", "b"], "default": "a" },
            "n": { "type": "integer", "minimum": 1, "maximum": 5, "default": 2 },
        } });
        let f = settings_fields(&schema);
        assert!(validate(&f, &json!({ "mode": "c" })).is_err());
        assert!(validate(&f, &json!({ "n": 9 })).is_err());
        assert_eq!(validate(&f, &json!({ "n": "3", "other": 1 })).unwrap(), json!({ "n": 3, "other": 1 }));
        assert_eq!(with_defaults(json!({ "n": 4 }), Some(&schema)), json!({ "n": 4, "mode": "a" }));
    }

    #[test]
    fn declared_fields_are_vetted() {
        let ok = declared(&[json!({ "key": "title", "type": "text", "label": "Title" })]).unwrap();
        assert_eq!(ok[0]["type"], "text");
        assert!(declared(&[json!({ "key": "bad key" })]).is_err());
        assert!(declared(&[json!({ "key": "a" }), json!({ "key": "a" })]).is_err());
    }
}
