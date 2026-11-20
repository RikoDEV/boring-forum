//! Plugin versions and the requirements on them (`api = "^1.2"`, `requires = ["other ~1.0"]`).

use std::cmp::Ordering;

pub(super) type Version = (u64, u64, u64);

/// `1`, `1.2`, `v1.2.3`, `1.2.3-beta`: missing parts are 0 and a suffix is ignored.
pub(super) fn parse(v: &str) -> Option<Version> {
    let core = v.trim().trim_start_matches('v').split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().map_or(Some(0), |p| p.parse().ok())?;
    let patch = parts.next().map_or(Some(0), |p| p.parse().ok())?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

pub(super) fn compare(a: &str, b: &str) -> Option<Ordering> {
    Some(parse(a)?.cmp(&parse(b)?))
}

/// How many parts a requirement wrote (`1` is one, `1.2.3` is three).
fn parts(req: &str) -> usize {
    req.split(['-', '+']).next().unwrap_or_default().split('.').count()
}

/// Whether `version` meets `req`: `*`, `^1.2` (compatible), `~1.2` (same minor), `>=1.2`, `=1.2.3`,
/// or a bare version, which means `^`.
pub(super) fn satisfies(version: &str, req: &str) -> bool {
    let req = req.trim();
    let Some(v) = parse(version) else { return false };
    if req.is_empty() || req == "*" {
        return true;
    }
    let (op, rest) =
        ["^", "~", ">=", "="].iter().find_map(|op| req.strip_prefix(op).map(|r| (*op, r.trim()))).unwrap_or(("^", req));
    let Some(min) = parse(rest) else { return false };
    let n = parts(rest);
    match op {
        ">=" => v >= min,
        "=" => v.0 == min.0 && (n < 2 || v.1 == min.1) && (n < 3 || v.2 == min.2),
        "~" => v >= min && v.0 == min.0 && v.1 == min.1,
        _ => v >= min && if min.0 > 0 { v.0 == min.0 } else { v.0 == 0 && (n < 2 || v.1 == min.1) },
    }
}

/// One entry of `requires`: `other-plugin` or `other-plugin ^1.0`.
pub(super) fn split_requirement(entry: &str) -> (&str, &str) {
    let entry = entry.trim();
    entry.split_once(char::is_whitespace).map_or((entry, "*"), |(id, req)| (id, req.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_number_not_text() {
        assert_eq!(compare("1.10.0", "1.9.0"), Some(Ordering::Greater));
        assert_eq!(compare("1", "1.0.0"), Some(Ordering::Equal));
        assert_eq!(compare("v2.0.0-beta", "1.9"), Some(Ordering::Greater));
        assert_eq!(compare("x", "1"), None);
    }

    #[test]
    fn requirements() {
        assert!(satisfies("1.3.0", "^1.2") && !satisfies("1.1.9", "^1.2") && !satisfies("2.0.0", "^1.2"));
        assert!(satisfies("1.2.9", "~1.2") && !satisfies("1.3.0", "~1.2"));
        assert!(satisfies("0.2.5", "^0.2") && !satisfies("0.3.0", "^0.2"));
        assert!(satisfies("1.4.0", ">=1.2") && satisfies("3.0.0", "*") && satisfies("1.0.0", "1.0"));
        assert!(satisfies("1.2.7", "=1.2") && !satisfies("1.3.0", "=1.2"));
        assert_eq!(split_requirement("blog ^1.0"), ("blog", "^1.0"));
        assert_eq!(split_requirement("blog"), ("blog", "*"));
    }
}
