//! Primary navigation, stored as JSON in the `nav_links` setting.
//!
//! Every value is checked when it is read, so a bad setting can neither break the page
//! nor put a `javascript:` link in the header.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NavItem {
    /// Built-in link (`forums`, `latest`, `top`, `search`): translated, fixed URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub url: String,
    /// Only members of this group see the link; `None` means everyone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<i64>,
    /// Members of any of these groups see the link; empty means everyone (with `group` unset).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<i64>,
}

fn builtin_url(key: &str) -> Option<&'static str> {
    Some(match key {
        "forums" => "/",
        "latest" => "/latest",
        "top" => "/top",
        "search" => "/search",
        "members" => "/u",
        "groups" => "/g",
        _ => return MODULES.iter().find(|m| m.0 == key).map(|m| m.1),
    })
}

/// Optional modules with a page of their own: (link key, address, the setting that switches it
/// on). Their links show only while the module is on.
pub const MODULES: &[(&str, &str, &str)] =
    &[("clubs", "/clubs", "clubs_enabled"), ("live", "/live", "live_topics_enabled")];

/// The setting a module link depends on, if `key` is a module.
pub fn module_setting(key: &str) -> Option<&'static str> {
    MODULES.iter().find(|m| m.0 == key).map(|m| m.2)
}

/// The module switched on by `setting`, if it is one.
pub fn module_for_setting(setting: &str) -> Option<&'static str> {
    MODULES.iter().find(|m| m.2 == setting).map(|m| m.0)
}

/// `json` with a link to module `key` added at the end, unless it is already there.
pub fn with_link(json: &str, key: &str) -> Option<String> {
    let mut items = parse(json);
    if items.iter().any(|i| i.key.as_deref() == Some(key)) {
        return None;
    }
    let url = builtin_url(key)?.to_owned();
    items.push(NavItem { key: Some(key.to_owned()), label: String::new(), url, group: None, groups: vec![] });
    serde_json::to_string(&items).ok()
}

fn safe_url(url: &str) -> bool {
    let local = url.starts_with('/') && !url.starts_with("//");
    (local || url.starts_with("https://")) && !url.contains(['\\', '\n', '\r', '\t'])
}

fn defaults() -> Vec<NavItem> {
    ["forums", "latest", "top", "search"]
        .iter()
        .map(|k| NavItem {
            key: Some((*k).into()),
            label: String::new(),
            url: builtin_url(k).unwrap_or("/").to_owned(),
            group: None,
            groups: vec![],
        })
        .collect()
}

/// The links in `json`, without invalid ones; the defaults if it is empty or malformed.
pub fn parse(json: &str) -> Vec<NavItem> {
    if json.trim().is_empty() {
        return defaults();
    }
    let Ok(items) = serde_json::from_str::<Vec<NavItem>>(json) else { return defaults() };
    items
        .into_iter()
        .filter_map(|mut i| {
            if let Some(url) = i.key.as_deref().map(builtin_url) {
                url?.clone_into(&mut i.url);
                return Some(i);
            }
            (safe_url(&i.url) && !i.label.trim().is_empty() && i.label.len() <= 60).then_some(i)
        })
        .collect()
}

/// Links this viewer's groups may see.
pub fn visible(items: Vec<NavItem>, groups: &[i64], is_admin: bool) -> Vec<NavItem> {
    items
        .into_iter()
        .filter(|i| {
            let open = i.group.is_none() && i.groups.is_empty();
            is_admin
                || open
                || i.group.is_some_and(|g| groups.contains(&g))
                || i.groups.iter().any(|g| groups.contains(g))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_for_several_groups() {
        let items = parse(
            r#"[{"label":"A","url":"/a","groups":[3,4]},{"label":"B","url":"/b","group":3},{"label":"C","url":"/c"}]"#,
        );
        let names = |g: &[i64]| visible(items.clone(), g, false).into_iter().map(|i| i.label).collect::<Vec<_>>();
        assert_eq!(names(&[4]), ["A", "C"]);
        assert_eq!(names(&[3]), ["A", "B", "C"]);
        assert_eq!(names(&[2]), ["C"]);
    }

    #[test]
    fn default_links_have_addresses() {
        for json in ["", "not json"] {
            let items = parse(json);
            assert_eq!(items.len(), 4);
            assert!(items.iter().all(|i| i.url.starts_with('/')), "{items:?}");
        }
    }

    #[test]
    fn module_links_are_added_once() {
        let once = with_link("", "clubs").expect("added");
        assert!(once.contains("\"clubs\""));
        assert!(with_link(&once, "clubs").is_none());
        assert_eq!(parse(&once).last().map(|i| i.url.as_str()), Some("/clubs"));
        assert_eq!(module_for_setting("clubs_enabled"), Some("clubs"));
    }

    #[test]
    fn empty_or_malformed_gives_defaults() {
        assert_eq!(parse("").len(), 4);
        assert_eq!(parse("{nope").len(), 4);
    }

    #[test]
    fn unsafe_links_are_dropped() {
        let items = parse(
            r#"[{"label":"a","url":"javascript:alert(1)"},{"label":"b","url":"//evil.example"},
                {"label":"c","url":"http://plain.example"},{"label":"","url":"/empty"},
                {"label":"ok","url":"/ok"},{"label":"ext","url":"https://x.example/"},
                {"key":"nope"},{"key":"top","url":"javascript:alert(1)"}]"#,
        );
        let urls: Vec<_> = items.iter().map(|i| i.url.as_str()).collect();
        assert_eq!(urls, ["/ok", "https://x.example/", "/top"]);
    }

    #[test]
    fn group_links_need_membership() {
        let items = parse(r#"[{"label":"staff","url":"/s","group":3},{"label":"all","url":"/a"}]"#);
        assert_eq!(visible(items.clone(), &[1], false).len(), 1);
        assert_eq!(visible(items.clone(), &[2, 3], false).len(), 2);
        assert_eq!(visible(items, &[], true).len(), 2);
    }
}
