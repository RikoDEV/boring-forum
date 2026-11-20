//! The "is this output inert?" check, shared by the XSS corpus test and the fuzz targets
//!.
//!
//! It lives in the library rather than in a test file so there is one definition of what the
//! pipeline guarantees. Both callers assert the same thing, and a future caller -- a plugin
//! conformance suite, a theme template check -- gets it for free.
//!
//! The rule: whatever the input, cooked output must contain no script or other executing
//! element, no event-handler attribute, and no attribute value that begins with an executable
//! URL scheme. The check is intentionally blunt, because sanitizer bugs usually show up as
//! *something* on that list surviving rather than as a precisely predictable string.

/// Elements that must never appear. Matched together with the opening `<`, so the escaped text
/// `&lt;script` inside a code block does not count: escaped text is inert, and producing it is
/// exactly what the pipeline is for.
const FORBIDDEN_TAGS: &[(&str, &str)] = &[
    ("<script", "a script element survived"),
    ("</script", "a script end tag survived"),
    ("<iframe", "an iframe survived"),
    ("<object", "an object element survived"),
    ("<embed", "an embed element survived"),
    ("<base", "a base element survived"),
    ("<meta", "a meta element survived"),
    ("<link", "a link element survived"),
    ("<style", "a style element survived"),
    ("<form", "a form element survived"),
    ("<keygen", "a keygen element survived"),
    ("<isindex", "an isindex element survived"),
    ("<marquee", "a marquee element survived"),
];

/// Attribute names that must never appear inside a tag.
const FORBIDDEN_ATTRS: &[(&str, &str)] = &[
    ("srcdoc", "an srcdoc attribute survived"),
    ("formaction", "a formaction attribute survived"),
    ("expression(", "a CSS expression survived"),
    ("xlink:href", "an xlink:href survived"),
];

/// URL schemes that must never begin an attribute value.
const FORBIDDEN_SCHEMES: &[&str] = &["javascript:", "vbscript:", "data:text/html"];

/// The contents of each tag, `<` and `>` excluded. Only what is inside a tag can execute: the
/// same bytes in text content have been escaped. Sanitized output has no comments or CDATA left,
/// so splitting on the delimiters is enough.
fn tags(lower: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = lower;
    while let Some(start) = rest.find('<') {
        rest = &rest[start + 1..];
        if let Some(end) = rest.find('>') {
            out.push(&rest[..end]);
            rest = &rest[end + 1..];
        } else {
            out.push(rest);
            break;
        }
    }
    out
}

/// True when `at` begins an attribute value: the text before it ends with `=`, allowing for a
/// quote and whitespace. This separates `src="javascript:…"`, which executes, from
/// ``src="`javascript:…"``, which a browser reads as a relative path -- the backtick is part of
/// the value, so what follows is not a scheme at all.
fn starts_an_attribute_value(tag: &str, at: usize) -> bool {
    let before = tag[..at].trim_end();
    let before = before.strip_suffix('"').or_else(|| before.strip_suffix('\'')).unwrap_or(before);
    before.trim_end().ends_with('=')
}

/// Every way `html` would execute, as readable sentences. Empty means inert.
pub fn violations(html: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let mut found = Vec::new();

    for (needle, why) in FORBIDDEN_TAGS {
        if lower.contains(needle) {
            found.push((*why).to_owned());
        }
    }

    for tag in tags(&lower) {
        for (needle, why) in FORBIDDEN_ATTRS {
            if tag.contains(needle) {
                found.push((*why).to_owned());
            }
        }
        for scheme in FORBIDDEN_SCHEMES {
            for (at, _) in tag.match_indices(scheme) {
                if starts_an_attribute_value(tag, at) {
                    found.push(format!("an attribute value starting with {scheme} survived"));
                }
            }
        }
        // Any `on…=` is an event handler. Checked by shape rather than by listing the hundreds
        // of handler names, so one added in a future HTML version is covered too.
        for (i, _) in tag.match_indices(" on") {
            let rest = &tag[i + 3..];
            let name_len = rest.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(rest.len());
            if name_len > 0 && rest[name_len..].trim_start().starts_with('=') {
                found.push(format!("the event handler attribute on{} survived", &rest[..name_len]));
            }
        }
    }

    found.sort();
    found.dedup();
    found
}

/// Panics with the offending input and output when `html` is not inert.
///
/// # Panics
/// When `html` contains any executable construct.
pub fn assert_inert(html: &str, input: &str) {
    let violations = violations(html);
    assert!(
        violations.is_empty(),
        "cooked output is not inert: {}\n  input:  {input:?}\n  output: {html:?}",
        violations.join(", ")
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_markup_is_inert() {
        assert!(violations("<p>hello <strong>world</strong></p>").is_empty());
        assert!(violations(r#"<a href="https://example.com" rel="nofollow">x</a>"#).is_empty());
        assert!(violations("").is_empty());
    }

    #[test]
    fn escaped_text_is_inert() {
        // What the pipeline produces for a payload, and the reason the check is tag-aware.
        assert!(violations("<p>&lt;script&gt;alert(1)&lt;/script&gt;</p>").is_empty());
        assert!(violations("<p>&lt;IMG SRC=javascript:alert('XSS')&gt;</p>").is_empty());
    }

    #[test]
    fn catches_the_real_thing() {
        assert!(!violations("<script>alert(1)</script>").is_empty());
        assert!(!violations(r"<img src=x onerror=alert(1)>").is_empty());
        assert!(!violations(r#"<a href="javascript:alert(1)">x</a>"#).is_empty());
        assert!(!violations(r"<a href=javascript:alert(1)>x</a>").is_empty());
        assert!(!violations(r#"<iframe srcdoc="x">"#).is_empty());
    }

    #[test]
    fn a_backtick_makes_the_scheme_a_relative_path() {
        // ammonia leaves this; a browser requests a relative URL, so it does not execute.
        assert!(violations("<img src=\"`javascript:alert(1)\">").is_empty());
    }

    #[test]
    fn an_attribute_merely_named_on_something_is_not_a_handler() {
        assert!(violations(r#"<p data-on="x">y</p>"#).is_empty());
        assert!(violations(r#"<p class="only">y</p>"#).is_empty());
    }
}
