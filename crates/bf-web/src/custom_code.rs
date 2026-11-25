//! Custom code (ACP → Settings → Custom code): CSS, `<head>` HTML and footer HTML an admin adds
//! to every page, whatever the theme, like Discourse's theme "common" fields.
//!
//! Unlike theme slots this HTML is not sanitized: it exists for scripts (analytics, widgets).
//! The page's policy still allows inline code by hash only, so the HTML is scanned once per
//! change and every inline `<script>` and `<style>` in it is allowed by its hash, and the hosts
//! of external scripts and stylesheets by origin. Safe mode leaves all of it out.

use crate::security::hash;
use bf_core::settings::Settings;
use std::sync::{Arc, LazyLock, RwLock};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct CustomCode {
    /// For a `<style>` element in `<head>`, already safe to place there.
    pub css: String,
    pub head: String,
    pub footer: String,
    /// CSP source lists, each entry ready to join with spaces.
    pub script_src: Vec<String>,
    pub style_src: Vec<String>,
    /// Hosts the scripts may talk to and frame: the script hosts and `custom_code_hosts`.
    pub connect_src: Vec<String>,
    /// The HTML has a script: third-party code rarely works under Trusted Types.
    pub scripts: bool,
}

impl CustomCode {
    pub fn is_empty(&self) -> bool {
        self.css.is_empty() && self.head.is_empty() && self.footer.is_empty()
    }
}

type Cached = Option<((String, String, String, String), Arc<CustomCode>)>;
static CACHE: LazyLock<RwLock<Cached>> = LazyLock::new(Default::default);

/// The parsed custom code for these settings. Parsed once per change, not per request.
pub fn from_settings(s: &Settings) -> Arc<CustomCode> {
    let key = (
        s.str("custom_css").to_owned(),
        s.str("custom_head_html").to_owned(),
        s.str("custom_footer_html").to_owned(),
        s.str("custom_code_hosts").to_owned(),
    );
    if let Some((k, c)) = CACHE.read().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref()
        && *k == key
    {
        return c.clone();
    }
    let code = Arc::new(build(&key.0, &key.1, &key.2, &key.3));
    *CACHE.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Some((key, code.clone()));
    code
}

fn build(css: &str, head: &str, footer: &str, hosts: &str) -> CustomCode {
    let mut c = CustomCode::default();
    // The browser's parser turns CRLF into LF before it hashes an inline element.
    let lf = |s: &str| s.trim().replace("\r\n", "\n").replace('\r', "\n");
    let css = lf(css);
    if !css.is_empty() {
        // `</style` would end the element early; `<\/` means the same inside a CSS string.
        c.css = css.replace("</", "<\\/");
        c.style_src.push(hash(&c.css));
    }
    c.head = lf(head);
    c.footer = lf(footer);
    let extra: Vec<String> = hosts.split_whitespace().filter_map(origin).collect();
    for html in [&c.head, &c.footer] {
        for el in elements(html) {
            match el.name {
                "script" => {
                    c.scripts = true;
                    match el.attr("src") {
                        Some(src) => {
                            if let Some(o) = origin(src) {
                                c.script_src.push(o.clone());
                                c.connect_src.push(o);
                            }
                        }
                        None => c.script_src.push(hash(el.body)),
                    }
                }
                "style" => c.style_src.push(hash(el.body)),
                _ => {
                    // <link rel="stylesheet" href="https://…">
                    let sheet = el
                        .attr("rel")
                        .is_some_and(|r| r.split_whitespace().any(|w| w.eq_ignore_ascii_case("stylesheet")));
                    if let Some(o) = el.attr("href").filter(|_| sheet).and_then(origin) {
                        c.style_src.push(o);
                    }
                }
            }
        }
    }
    // Allowed hosts exist for scripts that a loader pulls in at run time (a consent banner loads its
    // UI from them), with no `<script>` tag here to see: they write style attributes all the same.
    if !extra.is_empty() {
        c.scripts = true;
    }
    for o in extra {
        c.script_src.push(o.clone());
        c.style_src.push(o.clone());
        c.connect_src.push(o);
    }
    for list in [&mut c.script_src, &mut c.style_src, &mut c.connect_src] {
        let mut seen = std::collections::HashSet::new();
        list.retain(|x| seen.insert(x.clone()));
    }
    c
}

/// `https://host[:port]` of an absolute https (or protocol-relative) URL; a wildcard host such as
/// `https://*.example.com` is kept as written. Anything else adds nothing to the policy, so a
/// value cannot break out of its source list. Same-site paths need nothing: `'self'` covers them.
fn origin(url: &str) -> Option<String> {
    let url = url.trim();
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("//"))?;
    let host = rest.split(['/', '?', '#']).next()?.to_ascii_lowercase();
    let ok = !host.is_empty()
        && host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '*'))
        && host.contains('.');
    ok.then(|| format!("https://{host}"))
}

struct Element<'a> {
    name: &'static str,
    attrs: Vec<(String, String)>,
    /// Raw text of a script or style element, exactly what the browser hashes.
    body: &'a str,
}

impl Element<'_> {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

/// The `<script>`, `<style>` and `<link>` elements of an HTML fragment, read the way a browser
/// tokenizes them: script and style content is raw text up to its end tag; comments are skipped.
fn elements(html: &str) -> Vec<Element<'_>> {
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(off) = lower[i..].find('<') {
        let start = i + off;
        if lower[start..].starts_with("<!--") {
            i = lower[start..].find("-->").map_or(html.len(), |e| start + e + 3);
            continue;
        }
        let name = ["script", "style", "link"].into_iter().find(|n| {
            lower[start + 1..].starts_with(n)
                && lower[start + 1 + n.len()..].starts_with(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/')
        });
        let Some(name) = name else {
            i = start + 1;
            continue;
        };
        let (attrs, tag_end) = attributes(html, start + 1 + name.len());
        let mut body = "";
        i = tag_end;
        if name != "link" {
            let close = format!("</{name}");
            let end = lower[tag_end..].find(&close).map_or(html.len(), |e| tag_end + e);
            body = &html[tag_end..end];
            i = lower[end..].find('>').map_or(html.len(), |e| end + e + 1);
        }
        out.push(Element { name, attrs, body });
    }
    out
}

/// Attributes from `from` to the end of the start tag, and the index just after its `>`.
fn attributes(html: &str, from: usize) -> (Vec<(String, String)>, usize) {
    let b = html.as_bytes();
    let mut attrs = Vec::new();
    let mut i = from;
    loop {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b'/') {
            i += 1;
        }
        if i >= b.len() {
            return (attrs, b.len());
        }
        if b[i] == b'>' {
            return (attrs, i + 1);
        }
        let k = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && !matches!(b[i], b'=' | b'>' | b'/') {
            i += 1;
        }
        let key = html[k..i].to_ascii_lowercase();
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut value = String::new();
        if i < b.len() && b[i] == b'=' {
            i += 1;
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
                let q = b[i];
                let v = i + 1;
                i = v;
                while i < b.len() && b[i] != q {
                    i += 1;
                }
                html[v..i].clone_into(&mut value);
                i = (i + 1).min(b.len());
            } else {
                let v = i;
                while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'>' {
                    i += 1;
                }
                html[v..i].clone_into(&mut value);
            }
        }
        if !key.is_empty() {
            attrs.push((key, value.replace("&amp;", "&")));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_code_is_allowed_by_hash_and_hosts_by_origin() {
        let c = build(
            "a { color: red }",
            "<!-- <script>no</script> --><script async src=\"https://cdn.example.com/x.js?v=1\"></script>\n<SCRIPT>gtag('js', 1)</SCRIPT><style>b{}</style><link rel=\"preload stylesheet\" href='https://fonts.example.net/a.css'>",
            "<script type=module>go()</script>",
            "https://api.example.org/ ftp://no.example https://x.example; script-src *",
        );
        assert!(c.scripts);
        assert_eq!(
            c.script_src,
            vec![
                "https://cdn.example.com".to_owned(),
                hash("gtag('js', 1)"),
                hash("go()"),
                "https://api.example.org".into()
            ]
        );
        assert_eq!(
            c.style_src,
            vec![
                hash("a { color: red }"),
                hash("b{}"),
                "https://fonts.example.net".into(),
                "https://api.example.org".into()
            ]
        );
        assert_eq!(c.connect_src, vec!["https://cdn.example.com".to_owned(), "https://api.example.org".into()]);
    }

    #[test]
    fn values_cannot_alter_the_policy() {
        assert_eq!(origin("https://a.example; script-src *"), None);
        assert_eq!(origin("http://a.example/x.js"), None);
        assert_eq!(origin("/local.js"), None);
        assert_eq!(origin("//cdn.example.com/x.js"), Some("https://cdn.example.com".into()));
        assert_eq!(origin("https://*.example.com"), Some("https://*.example.com".into()));
    }

    #[test]
    fn css_cannot_close_its_element() {
        let c = build("a::after { content: \"</style><script>x</script>\" }", "", "", "");
        assert!(!c.css.contains("</"));
        assert!(!c.scripts);
    }

    #[test]
    fn allowed_hosts_alone_count_as_scripts() {
        assert!(build("", "", "", "https://cmp.example.com").scripts);
        assert!(!build("", "", "", "").scripts);
    }
}
