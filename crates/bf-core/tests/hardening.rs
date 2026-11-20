//! Regression tests for security hardening in bf-core.

use bf_core::auth::{email_alias_key, is_reserved_skeleton, normalize_email, username_skeleton};
use bf_core::html::strip_hidden_chars;
use bf_core::settings::{SettingError, Settings};
use serde_json::json;

#[test]
fn email_rejects_whitespace_and_control_characters() {
    assert_eq!(normalize_email("a b@example.com"), None);
    assert_eq!(normalize_email("a@exa mple.com"), None);
    assert_eq!(normalize_email("a\u{0}@example.com"), None);
}

#[test]
fn email_rejects_two_at_signs_and_non_ascii() {
    assert_eq!(normalize_email("a@b@example.com"), None);
    assert_eq!(normalize_email("ałice@example.com"), None);
    assert_eq!(normalize_email("alice@exämple.com"), None);
}

#[test]
fn email_rejects_bad_dots() {
    for bad in [".a@example.com", "a.@example.com", "a..b@example.com", "a@example..com", "a@.example.com", "a@example"] {
        assert_eq!(normalize_email(bad), None, "{bad}");
    }
}

#[test]
fn email_accepts_normal_lowercases_domain_and_strips_trailing_dot() {
    assert_eq!(normalize_email("Alice.B+x@Example.COM.").as_deref(), Some("Alice.B+x@example.com"));
    assert_eq!(normalize_email("  bob@example.org ").as_deref(), Some("bob@example.org"));
}

#[test]
fn email_alias_key_strips_tag_and_gmail_dots_only() {
    assert_eq!(email_alias_key("a.b+tag@gmail.com"), ("ab".into(), "gmail.com".into()));
    assert_eq!(email_alias_key("A.B@googlemail.com"), ("ab".into(), "gmail.com".into()));
    assert_eq!(email_alias_key("a.b+tag@example.com"), ("a.b".into(), "example.com".into()));
}

#[test]
fn lookalikes_of_reserved_names_are_reserved_skeletons() {
    // UTS #39 maps 1 to l, so "adm1n" is not a look-alike of "admin"; "ro0t" is one of "root".
    assert!(is_reserved_skeleton(&username_skeleton("ro0t")));
    assert!(is_reserved_skeleton(&username_skeleton("admin")));
    // Cyrillic a as the first letter.
    assert!(is_reserved_skeleton(&username_skeleton("\u{430}dmin")));
    assert!(!is_reserved_skeleton(&username_skeleton("alice")));
}

#[test]
fn strip_hidden_chars_removes_invisible_and_bidi_but_keeps_zwj() {
    assert_eq!(strip_hidden_chars("a\u{200B}b\u{202E}c"), "abc");
    let family = "\u{1F468}\u{200D}\u{1F469}";
    assert_eq!(strip_hidden_chars(family), family);
}

#[test]
fn settings_refuse_unknown_choice_values() {
    let s = Settings::default();
    for key in ["edit_history_visible_to", "hotlink_protection"] {
        assert!(matches!(s.validate(key, &json!("bogus")), Err(SettingError::NotOneOf { .. })), "{key}");
    }
}

#[test]
fn settings_accept_known_choice_values() {
    let s = Settings::default();
    assert!(s.validate("edit_history_visible_to", &json!("staff")).is_ok());
    assert!(s.validate("hotlink_protection", &json!("all")).is_ok());
}

#[test]
fn public_values_hold_no_numeric_settings() {
    let s = Settings::default();
    let public = s.public_values();
    assert!(public.contains_key("edit_history_visible_to"), "a public choice setting is listed");
    for (k, v) in &public {
        assert!(!v.is_number(), "numeric setting {k} is public");
    }
}
