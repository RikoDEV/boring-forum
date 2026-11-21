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

/// Elements that must never appear, by name. The check reads parsed tags, not raw text: the escaped
/// text `&lt;script` in a code block is inert (producing it is what the pipeline is for), and so is
/// `<script` inside a quoted attribute value such as a title.
const FORBIDDEN_ELEMENTS: &[(&str, &str)] = &[
    ("script", "a script element survived"),
    ("iframe", "an iframe survived"),
    ("object", "an object element survived"),
    ("embed", "an embed element survived"),
    ("base", "a base element survived"),
    ("meta", "a meta element survived"),
    ("link", "a link element survived"),
    ("style", "a style element survived"),
    ("form", "a form element survived"),
    ("keygen", "a keygen element survived"),
    ("isindex", "an isindex element survived"),
    ("marquee", "a marquee element survived"),
];

/// Attribute names that must never appear on a tag. Matched against the names only: a heading's
/// generated `id="h-srcdoc"` is a value, not an attribute.
const FORBIDDEN_ATTRS: &[(&str, &str)] = &[
    ("srcdoc", "an srcdoc attribute survived"),
    ("formaction", "a formaction attribute survived"),
    ("xlink:href", "an xlink:href survived"),
];

/// The attributes of a tag's contents (`a href="x" srcdoc=y`) as (name, value). What a browser would
/// read as a name is a name; quoted and unquoted values are values, so text inside one (a heading's
/// `id="h-srcdoc"`, a code block's `class="language-hexpression("`) is never mistaken for an attribute.
fn attributes(tag: &str) -> Vec<(&str, &str)> {
    let bytes = tag.as_bytes();
    let mut attrs = Vec::new();
    let mut i = 0;
    // The tag name itself.
    while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'/' {
        i += 1;
    }
    while i < bytes.len() {
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b'/') {
            i += 1;
        }
        let start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'=' && bytes[i] != b'/' {
            i += 1;
        }
        let name = &tag[start..i];
        let mut value = "";
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i < bytes.len() && bytes[i] == b'=' {
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            match bytes.get(i) {
                Some(&q) if q == b'"' || q == b'\'' => {
                    i += 1;
                    let from = i;
                    while i < bytes.len() && bytes[i] != q {
                        i += 1;
                    }
                    value = &tag[from..i];
                    i += 1;
                }
                _ => {
                    let from = i;
                    while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                        i += 1;
                    }
                    value = &tag[from..i];
                }
            }
        }
        if !name.is_empty() {
            attrs.push((name, value));
        }
    }
    attrs
}

/// URL schemes that must never begin the value of an attribute that holds a URL.
const FORBIDDEN_SCHEMES: &[&str] = &["javascript:", "vbscript:", "data:text/html"];

/// Attributes a browser reads as a URL. A script scheme runs from these; as the text of a `title` or an
/// `alt` it is only text.
const URL_ATTRS: &[&str] = &[
    "href",
    "src",
    "srcset",
    "action",
    "formaction",
    "data",
    "poster",
    "background",
    "cite",
    "longdesc",
    "ping",
    "xlink:href",
    "codebase",
    "manifest",
    "icon",
    "lowsrc",
    "dynsrc",
    "usemap",
    "profile",
    "classid",
    "archive",
];

/// The contents of each tag, `<` and `>` excluded. Only what is inside a tag can execute: the same
/// bytes in text content have been escaped. A `>` or `<` inside a quoted attribute value belongs to
/// the value, so the scan follows quotes (a quote opens only right after `=`, as in HTML).
fn tags(lower: &str) -> Vec<&str> {
    let bytes = lower.as_bytes();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(off) = lower[from..].find('<') {
        let start = from + off + 1;
        let mut i = start;
        let mut quote: Option<u8> = None;
        let mut expect_value = false;
        while i < bytes.len() {
            let b = bytes[i];
            if let Some(q) = quote {
                if b == q {
                    quote = None;
                }
            } else if b == b'>' {
                break;
            } else if b == b'=' {
                expect_value = true;
            } else if expect_value && !b.is_ascii_whitespace() {
                if b == b'"' || b == 39 {
                    quote = Some(b);
                }
                expect_value = false;
            }
            i += 1;
        }
        out.push(&lower[start..i]);
        from = i + 1;
        if from > lower.len() {
            break;
        }
    }
    out
}

/// A tag's name, and whether it is an end tag: `/script x` is (`script`, true).
fn element(tag: &str) -> (&str, bool) {
    let end_tag = tag.starts_with('/');
    let body = tag.strip_prefix('/').unwrap_or(tag);
    let name = body.split(|c: char| c.is_ascii_whitespace() || c == '/').next().unwrap_or("");
    (name, end_tag)
}

/// Every way `html` would execute, as readable sentences. Empty means inert.
pub fn violations(html: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let mut found = Vec::new();

    for tag in tags(&lower) {
        let (name, end_tag) = element(tag);
        for (forbidden, why) in FORBIDDEN_ELEMENTS {
            if name == *forbidden {
                found.push((*why).to_owned());
                if end_tag && name == "script" {
                    found.push("a script end tag survived".to_owned());
                }
            }
        }
        let attrs = attributes(tag);
        for (forbidden, why) in FORBIDDEN_ATTRS {
            if attrs.iter().any(|(attr, _)| attr == forbidden) {
                found.push((*why).to_owned());
            }
        }
        for (attr, value) in &attrs {
            // A CSS expression runs only from a style (the sanitizer keeps none); as text in some other
            // attribute, such as the class of a code block, it is inert.
            if *attr == "style" && value.contains("expression(") {
                found.push("a CSS expression survived".to_owned());
            }
            // A browser drops tabs and newlines inside a URL and leading control characters and spaces
            // before it, so `jav&#9;ascript:` and ` javascript:` are the same scheme.
            let url: String = value.chars().filter(|c| !matches!(c, '\t' | '\n' | '\r')).collect();
            let url = url.trim_start_matches(|c: char| c <= ' ');
            for scheme in FORBIDDEN_SCHEMES {
                if URL_ATTRS.contains(attr) && url.starts_with(scheme) {
                    found.push(format!("an attribute value starting with {scheme} survived"));
                }
            }
            // Any `on…=` is an event handler. Checked by shape rather than by listing the hundreds
            // of handler names, so one added in a future HTML version is covered too.
            if let Some(handler) = attr.strip_prefix("on")
                && !handler.is_empty()
                && handler.chars().all(|c| c.is_ascii_alphabetic())
            {
                found.push(format!("the event handler attribute on{handler} survived"));
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
    fn an_attribute_value_that_spells_a_forbidden_name_is_not_that_attribute() {
        // A heading called "srcdoc" gets the anchor `h-srcdoc`: a value, found by the fuzzer.
        assert!(violations(r##"<h1 id="h-srcdoc">srcdoc<a href="#srcdoc"></a></h1>"##).is_empty());
        // The attribute itself is still caught, quoted, unquoted, after other attributes or after a slash.
        for bad in [
            r#"<iframe srcdoc="x">"#,
            "<iframe srcdoc=x>",
            r#"<a title="t" srcdoc='y'>"#,
            "<iframe/srcdoc=x>",
            "<button formaction=x>",
        ] {
            assert!(!violations(bad).is_empty(), "{bad}");
        }
    }

    #[test]
    fn a_css_expression_matters_only_in_a_style_attribute() {
        // Found by the fuzzer: a fenced code block's language ends up in `class`, where the text is inert.
        assert!(violations(r#"<pre><code class="language-hexpression("></code></pre>"#).is_empty());
        assert!(violations("<p>expression(</p>").is_empty());
        for bad in [
            r#"<div style="width: expression(alert(1))">"#,
            "<div style='width:expression(1)'>",
            "<div style=expression(1)>",
        ] {
            assert!(!violations(bad).is_empty(), "{bad}");
        }
    }

    #[test]
    fn text_that_looks_like_markup_inside_a_value_is_not_markup() {
        // Quoted values may hold `<`, `>` and words that would be dangerous as tags or as URLs.
        for ok in [
            r#"<a href="https://example.com/" title="<script>alert(1)</script>">x</a>"#,
            r#"<a href="https://example.com/" title="a one=1 onclick=2">x</a>"#,
            r#"<a href="https://example.com/" title="javascript:alert(1)">x</a>"#,
            r#"<img src="https://example.com/a.png" alt="data:text/html,x">"#,
        ] {
            assert!(violations(ok).is_empty(), "{ok}: {:?}", violations(ok));
        }
        // The same words where they do run.
        for bad in [
            r#"<a href="javascript:alert(1)">x</a>"#,
            "<a href=' jav&#9;ascript:x'>x</a>".replace("&#9;", "	").as_str(),
            r#"<a title="x" onclick="y">x</a>"#,
            "<img src=x onerror=alert(1)>",
            "<SCRIPT>alert(1)</SCRIPT>",
            "</script>",
        ] {
            assert!(!violations(bad).is_empty(), "{bad}");
        }
    }

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
