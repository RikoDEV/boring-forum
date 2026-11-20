//! BBCode compatibility for imported content: `[b] [i] [u] [s] [url] [img] [quote]
//! [code] [spoiler]`, rewritten to Markdown before parsing, so nothing bypasses the sanitizer.
//! Text in Markdown code fences and code spans is left alone. Custom emoji are expanded here too.

use std::collections::BTreeMap;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};

fn tag_pair(s: &str, open: &str, close: &str, f: impl Fn(&str, Option<&str>) -> String) -> String {
    let (open_lc, close_lc) = (format!("[{open}"), format!("[/{close}]"));
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    loop {
        let lower = rest.to_ascii_lowercase();
        // ASCII lowercasing keeps byte offsets valid.
        let Some(i) = lower.find(&open_lc) else { break };
        let after = &rest[i + open_lc.len()..];
        let (attr, inner_start) = match after.chars().next() {
            Some(']') => (None, 1),
            Some('=') => match after.find(']') {
                Some(e) => (Some(after[1..e].trim_matches(['"', '\''])), e + 1),
                None => break,
            },
            _ => {
                out.push_str(&rest[..i + open_lc.len()]);
                rest = after;
                continue;
            }
        };
        let body = &after[inner_start..];
        let Some(e) = body.to_ascii_lowercase().find(&close_lc) else { break };
        out.push_str(&rest[..i]);
        out.push_str(&f(&body[..e], attr));
        rest = &body[e + close_lc.len()..];
    }
    out.push_str(rest);
    out
}

static ENABLED: AtomicBool = AtomicBool::new(true);

/// The `bbcode_enabled` setting. Posts already cooked change on the next re-cook.
pub fn set_bbcode(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Marks where a `[code]` block was set aside. A placeholder also carries a number chosen anew
/// for each call (`nonce`) that a writer cannot know, so text that merely looks like a
/// placeholder is never replaced by a block: otherwise one block could be copied once per typed
/// look-alike, and a short post would cook into megabytes. (The characters themselves stay open
/// to plugin rules, which may use them as their own markers.)
const CODE_OPEN: char = '\u{e000}';
const CODE_CLOSE: char = '\u{e001}';

/// A number that differs from call to call and cannot be predicted from outside.
fn nonce() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    std::collections::hash_map::RandomState::new().build_hasher().finish()
}

fn convert(s: &str) -> String {
    if !ENABLED.load(Ordering::Relaxed) {
        return s.to_owned();
    }
    // Code is set aside first, so the passes below cannot touch what is inside it.
    let code = std::cell::RefCell::new(Vec::new());
    let nonce = nonce();
    let mut t = tag_pair(s, "code", "code", |b, _| {
        let mut held = code.borrow_mut();
        held.push(format!("\n```\n{}\n```\n", b.trim_matches('\n')));
        format!("{CODE_OPEN}{nonce:x}.{}{CODE_CLOSE}", held.len() - 1)
    });
    for (tag, md) in [("b", "**"), ("i", "*"), ("s", "~~")] {
        t = tag_pair(&t, tag, tag, |b, _| format!("{md}{b}{md}"));
    }
    t = tag_pair(&t, "u", "u", |b, _| b.to_owned());
    t = tag_pair(&t, "img", "img", |b, _| format!("![]({})", b.trim()));
    t = tag_pair(&t, "url", "url", |b, a| format!("[{}]({})", b, a.unwrap_or(b).trim()));
    t = tag_pair(&t, "spoiler", "spoiler", |b, _| format!("\n```spoiler\n{b}\n```\n"));
    t = tag_pair(&t, "quote", "quote", |b, a| {
        let q: Vec<String> = b.trim().lines().map(|l| format!("> {l}")).collect();
        let by = a.map(|a| format!("\n>\n> — {a}")).unwrap_or_default();
        format!("\n{}{by}\n\n", q.join("\n"))
    });
    for (i, block) in code.borrow().iter().enumerate() {
        t = t.replacen(&format!("{CODE_OPEN}{nonce:x}.{i}{CODE_CLOSE}"), block, 1);
    }
    t
}

static CUSTOM_EMOJI: RwLock<BTreeMap<String, String>> = RwLock::new(BTreeMap::new());

/// Replaces the custom emoji set (name → upload URL). Kept in memory, since cooking is pure and sync.
pub fn set_custom_emoji(map: BTreeMap<String, String>) {
    *CUSTOM_EMOJI.write().unwrap_or_else(std::sync::PoisonError::into_inner) = map;
}

static PLUGIN_EMOJI: RwLock<BTreeMap<String, String>> = RwLock::new(BTreeMap::new());

/// Replaces the emoji plugins ship (`[[emoji]]`): name to the address the server gives the image.
/// The site's own custom emoji win when a name is in both.
pub fn set_plugin_emoji(map: BTreeMap<String, String>) {
    *PLUGIN_EMOJI.write().unwrap_or_else(std::sync::PoisonError::into_inner) = map;
}

fn emoji(s: &str) -> String {
    let map = CUSTOM_EMOJI.read().unwrap_or_else(std::sync::PoisonError::into_inner);
    let plugin = PLUGIN_EMOJI.read().unwrap_or_else(std::sync::PoisonError::into_inner);
    if (map.is_empty() && plugin.is_empty()) || !s.contains(':') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find(':') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        if let Some((e, url)) = after.find(':').and_then(|e| map.get(&after[..e]).or_else(|| plugin.get(&after[..e])).map(|u| (e, u))) {
            out.push_str(&format!("![:{}:]({url})", &after[..e]));
            rest = &after[e + 1..];
        } else {
            out.push(':');
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

pub fn to_markdown(raw: &str) -> String {
    if !raw.contains('[') && !raw.contains(':') {
        return raw.to_owned();
    }
    let mut out = String::with_capacity(raw.len());
    let mut chunk = String::new();
    let mut fenced = false;
    for line in raw.split_inclusive('\n') {
        let fence = line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~");
        if fenced || fence {
            out.push_str(&convert_spans(&std::mem::take(&mut chunk)));
            out.push_str(line);
            if fence {
                fenced = !fenced;
            }
        } else {
            chunk.push_str(line);
        }
    }
    out.push_str(&convert_spans(&chunk));
    out
}

/// Converts outside backtick spans.
fn convert_spans(s: &str) -> String {
    s.split('`')
        .enumerate()
        .map(|(i, part)| if i % 2 == 0 { emoji(&convert(part)) } else { part.to_owned() })
        .collect::<Vec<_>>()
        .join("`")
}

#[cfg(test)]
mod tests {
    use super::to_markdown;

    #[test]
    fn converts_common_tags() {
        assert_eq!(to_markdown("[b]x[/b] [I]y[/I]"), "**x** *y*");
        assert_eq!(to_markdown("[url=https://a.example]t[/url]"), "[t](https://a.example)");
        assert_eq!(to_markdown("[url]https://a.example[/url]"), "[https://a.example](https://a.example)");
        assert_eq!(to_markdown("[img]https://a.example/x.png[/img]"), "![](https://a.example/x.png)");
        assert!(to_markdown("[quote=bob]hi[/quote]").contains("> hi\n>\n> — bob"));
        assert!(to_markdown("[code]a[b]c[/b][/code]").contains("```\na"));
    }

    #[test]
    fn leaves_code_and_plain_brackets_alone() {
        assert_eq!(to_markdown("`[b]x[/b]`"), "`[b]x[/b]`");
        assert_eq!(to_markdown("```\n[b]x[/b]\n```\n"), "```\n[b]x[/b]\n```\n");
        assert_eq!(to_markdown("[link](https://a.example) [ref] [b]open"), "[link](https://a.example) [ref] [b]open");
    }

    #[test]
    fn scripts_still_get_sanitized() {
        let html = crate::cook("[url=javascript:alert(1)]x[/url] [img]javascript:alert(1)[/img]").html;
        assert!(!html.contains("javascript:"), "{html}");
    }
}
