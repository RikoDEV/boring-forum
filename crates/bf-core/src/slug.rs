//! URL slugs and Discourse-style paths: `/t/{slug}/{id}`, `/c/{slug}/{id}`, `/u/{name}`.

/// ASCII slug: lowercase, `-` separated, at most 80 bytes. Falls back to "topic".
pub fn slugify(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    for c in title.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= 80 {
            break;
        }
    }
    let s = out.trim_end_matches('-');
    if s.is_empty() { "topic".to_owned() } else { s.to_owned() }
}

/// Parses `{id}-{slug}` path segments; the slug part is only used for redirects.
pub fn parse_id_slug(seg: &str) -> Option<(i64, &str)> {
    let (id, slug) = seg.split_once('-').unwrap_or((seg, ""));
    id.parse().ok().filter(|n| *n > 0).map(|n| (n, slug))
}

pub fn topic_path(id: i64, slug: &str) -> String {
    format!("/t/{slug}/{id}")
}

pub fn forum_path(id: i64, slug: &str) -> String {
    format!("/c/{slug}/{id}")
}

pub fn profile_path(_id: i64, username: &str) -> String {
    format!("/u/{}", username.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slugify("Hello, World!  Rust 2026"), "hello-world-rust-2026");
        assert_eq!(slugify("¿¿??"), "topic");
        assert!(slugify(&"a".repeat(300)).len() <= 80);
    }

    #[test]
    fn id_slug() {
        assert_eq!(parse_id_slug("12-hello-world"), Some((12, "hello-world")));
        assert_eq!(parse_id_slug("12"), Some((12, "")));
        assert_eq!(parse_id_slug("0-x"), None);
        assert_eq!(parse_id_slug("abc"), None);
    }
}
