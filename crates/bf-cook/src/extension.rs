//! Declarative syntax that plugins add (`[[cook_rules]]` in a plugin manifest): a literal
//! opening and closing marker around text becomes an HTML template, or is handed to the plugin.
//!
//! There is no regular expression and no code here, so a rule cannot be made to backtrack: a
//! text node is scanned once per rule with `str::find`, and both the number of matches and the
//! size of the output are capped. The result is an AST node like any other, and the sanitizer
//! still runs over the finished page, so a template cannot add what a post cannot.
//!
//! A marker pair may sit in one run of text (`||secret||`) or span inline formatting
//! (`||some **bold** words||`): then the template is split at its `{}` and the formatted
//! content stays in between, as ordinary inline nodes.
//!
//! A rule with a `callback` name does not carry a template: the text between the markers is
//! recorded in [`crate::Cooked::callbacks`] and a placeholder stands in the HTML until the
//! server has asked the plugin (the `cook.rule` hook) and put its answer there.

use comrak::Arena;
use comrak::nodes::{AstNode, NodeValue};
use std::cell::RefCell;
use std::sync::{LazyLock, RwLock};

/// One rule. `inline` rules work inside a paragraph's text (`||secret||`); `block` rules need a
/// paragraph that is exactly `open … close` (`:::note some text:::`) and replace the paragraph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub open: String,
    pub close: String,
    /// `{}` stands for the text between the markers, HTML-escaped.
    pub html: String,
    pub block: bool,
    /// The plugin decides the HTML (name sent with the `cook.rule` hook); `html` is ignored.
    pub callback: Option<String>,
}

const MAX_RULES: usize = 32;
const MAX_MARKER: usize = 32;
const MAX_TEMPLATE: usize = 2048;
/// Matches replaced in one text node; the rest of the node stays as written.
const MAX_MATCHES: usize = 100;
/// Longest text a marker pair may wrap.
const MAX_INNER: usize = 2000;
/// How many nodes a marker pair may span.
const MAX_SPAN: usize = 50;
/// Callbacks recorded for one post; the rest stay as written.
pub const MAX_CALLBACKS: usize = 20;

/// Where a callback's answer goes in the HTML: `PLACEHOLDER_OPEN`, its number, `PLACEHOLDER_CLOSE`.
pub const PLACEHOLDER_OPEN: char = '\u{e002}';
pub const PLACEHOLDER_CLOSE: char = '\u{e003}';

static RULES: LazyLock<RwLock<Vec<Rule>>> = LazyLock::new(Default::default);

thread_local! {
    static CALLBACKS: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
}

/// Starts a post's callback list (the outermost cook only: nested ones share it).
pub(crate) fn begin() {
    CALLBACKS.with(|c| c.borrow_mut().clear());
}

/// The callbacks recorded since [`begin`]: (rule name, text).
pub(crate) fn take() -> Vec<(String, String)> {
    CALLBACKS.with(|c| std::mem::take(&mut *c.borrow_mut()))
}

/// Whether a rule is acceptable: markers of 1 to 32 characters without line breaks, and a
/// template of up to 2 KB (or a callback name). Returns a problem for an admin to read.
pub fn check(rule: &Rule) -> Result<(), &'static str> {
    let marker = |m: &str| !m.is_empty() && m.chars().count() <= MAX_MARKER && !m.contains(['\n', '\r']);
    if !marker(&rule.open) || !marker(&rule.close) {
        return Err("A marker must be 1 to 32 characters on one line.");
    }
    match &rule.callback {
        Some(name) if name.is_empty() || name.len() > 40 => Err("A callback needs a name of up to 40 characters."),
        None if rule.html.len() > MAX_TEMPLATE || rule.html.is_empty() => Err("A template must be 1 to 2048 bytes."),
        _ => Ok(()),
    }
}

/// Replaces the rules the cooker applies. Rules that fail `check` are dropped.
pub fn set_extensions(rules: Vec<Rule>) {
    let rules: Vec<Rule> = rules.into_iter().filter(|r| check(r).is_ok()).take(MAX_RULES).collect();
    *RULES.write().unwrap_or_else(std::sync::PoisonError::into_inner) = rules;
}

fn rules() -> Vec<Rule> {
    RULES.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

fn fill(template: &str, inner: &str) -> String {
    template.replace("{}", &esc(inner))
}

/// What a match turns into: the template filled in, or a placeholder for the plugin's answer.
fn output(rule: &Rule, inner: &str) -> String {
    let Some(name) = &rule.callback else { return fill(&rule.html, inner) };
    CALLBACKS.with(|c| {
        let mut list = c.borrow_mut();
        if list.len() >= MAX_CALLBACKS {
            return esc(inner);
        }
        list.push((name.clone(), inner.to_owned()));
        format!("{PLACEHOLDER_OPEN}{}{PLACEHOLDER_CLOSE}", list.len() - 1)
    })
}

/// Splits `text` into (is_match_output, string) parts, or `None` when nothing matched.
fn apply_inline(text: &str, rule: &Rule) -> Option<Vec<(bool, String)>> {
    let mut parts = Vec::new();
    let mut rest = text;
    let mut count = 0;
    while count < MAX_MATCHES {
        let Some(start) = rest.find(&rule.open) else { break };
        let after = &rest[start + rule.open.len()..];
        let Some(end) = after.find(&rule.close) else { break };
        let inner = &after[..end];
        if inner.is_empty() || inner.len() > MAX_INNER {
            // Not a pair worth replacing: keep the opener as text and look past it.
            let keep = start + rule.open.len();
            parts.push((false, rest[..keep].to_owned()));
            rest = &rest[keep..];
            continue;
        }
        if start > 0 {
            parts.push((false, rest[..start].to_owned()));
        }
        parts.push((true, output(rule, inner)));
        rest = &after[end + rule.close.len()..];
        count += 1;
    }
    if count == 0 {
        return None;
    }
    if !rest.is_empty() {
        parts.push((false, rest.to_owned()));
    }
    Some(parts)
}

fn plain_text<'a>(node: &'a AstNode<'a>) -> Option<String> {
    let mut kids = node.children();
    let (Some(only), None) = (kids.next(), kids.next()) else { return None };
    text_of(only)
}

fn text_of<'a>(node: &'a AstNode<'a>) -> Option<String> {
    match &node.data.borrow().value {
        NodeValue::Text(t) => Some(t.to_string()),
        _ => None,
    }
}

fn text_node<'a>(arena: &'a Arena<'a>, s: &str) -> &'a AstNode<'a> {
    arena.alloc(NodeValue::Text(s.to_owned().into()).into())
}

fn raw_node<'a>(arena: &'a Arena<'a>, s: String) -> &'a AstNode<'a> {
    arena.alloc(NodeValue::Raw(s).into())
}

type Span<'a> = (&'a AstNode<'a>, String, usize, &'a AstNode<'a>, String, usize);

/// The first pair whose markers sit in different nodes of one parent, as (opening node, its text,
/// where the opener starts, closing node, its text, where the closer starts).
fn find_span<'a>(parent: &'a AstNode<'a>, rule: &Rule) -> Option<Span<'a>> {
    let kids: Vec<_> = parent.children().collect();
    for (i, a) in kids.iter().enumerate() {
        let Some(ta) = text_of(a) else { continue };
        let Some(p) = ta.find(&rule.open) else { continue };
        if ta[p + rule.open.len()..].contains(&rule.close) {
            continue; // one node: the plain pass handles it
        }
        for b in kids.iter().skip(i + 1).take(MAX_SPAN) {
            let Some(tb) = text_of(b) else { continue };
            if let Some(q) = tb.find(&rule.close) {
                return Some((a, ta, p, b, tb, q));
            }
        }
    }
    None
}

/// Pairs whose markers sit in different nodes of one parent (`||a **b** c||`): the template is
/// split at its single `{}` and the nodes in between stay where they are.
fn apply_spanning<'a>(arena: &'a Arena<'a>, root: &'a AstNode<'a>, rule: &Rule) {
    if rule.callback.is_some() {
        return;
    }
    let Some((head, tail)) = rule.html.split_once("{}").filter(|(_, t)| !t.contains("{}")) else { return };
    let parents: Vec<_> = root
        .descendants()
        .filter(|n| n.first_child().is_some() && !super::inside_link(n))
        .filter(|n| !matches!(n.data.borrow().value, NodeValue::CodeBlock(_) | NodeValue::Link(_)))
        .collect();
    for parent in parents {
        for _ in 0..MAX_MATCHES {
            let Some((a, ta, p, b, tb, q)) = find_span(parent, rule) else { break };
            if p > 0 {
                a.insert_before(text_node(arena, &ta[..p]));
            }
            a.insert_before(raw_node(arena, head.to_owned()));
            let inner_start = &ta[p + rule.open.len()..];
            if !inner_start.is_empty() {
                a.insert_before(text_node(arena, inner_start));
            }
            a.detach();
            if q > 0 {
                b.insert_before(text_node(arena, &tb[..q]));
            }
            b.insert_before(raw_node(arena, tail.to_owned()));
            let after = &tb[q + rule.close.len()..];
            if !after.is_empty() {
                b.insert_before(text_node(arena, after));
            }
            b.detach();
        }
    }
}

/// Runs the plugin rules over the document. Code and links are left alone.
pub(crate) fn apply<'a>(arena: &'a Arena<'a>, root: &'a AstNode<'a>) {
    let rules = rules();
    if rules.is_empty() {
        return;
    }
    for rule in rules.iter().filter(|r| r.block) {
        let paras: Vec<_> =
            root.descendants().filter(|n| matches!(n.data.borrow().value, NodeValue::Paragraph)).collect();
        for p in paras {
            let Some(text) = plain_text(p) else { continue };
            let t = text.trim();
            let inner = t.strip_prefix(rule.open.as_str()).and_then(|r| r.strip_suffix(rule.close.as_str()));
            if let Some(inner) = inner.filter(|i| !i.trim().is_empty() && i.len() <= MAX_INNER) {
                p.insert_before(raw_node(arena, output(rule, inner.trim())));
                p.detach();
            }
        }
    }
    for rule in rules.iter().filter(|r| !r.block) {
        let texts: Vec<_> = root
            .descendants()
            .filter(|n| matches!(&n.data.borrow().value, NodeValue::Text(t) if t.contains(rule.open.as_str())))
            .filter(|n| !super::inside_link(n))
            .collect();
        for node in texts {
            let Some(text) = text_of(node) else { continue };
            let Some(parts) = apply_inline(&text, rule) else { continue };
            for (is_html, s) in parts {
                node.insert_before(if is_html { raw_node(arena, s) } else { text_node(arena, &s) });
            }
            node.detach();
        }
        apply_spanning(arena, root, rule);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rules are global, so tests that set them run one at a time.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn rule(open: &str, close: &str, html: &str, block: bool) -> Rule {
        Rule { open: open.into(), close: close.into(), html: html.into(), block, callback: None }
    }

    #[test]
    fn markers_become_templates() {
        let _one_at_a_time = SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        set_extensions(vec![
            rule("\u{e000}", "\u{e001}", "<span class=\"plug-spoiler\">{}</span>", false),
            rule("\u{e000}:", "\u{e000}:", "<aside class=\"plug-note\">{}</aside>", true),
        ]);
        let html = crate::cook("a \u{e000}x < y \"q\"\u{e001} b").html;
        assert!(html.contains("<span class=\"plug-spoiler\">x &lt; y \"q\"</span>"), "{html}");
        let html = crate::cook("\u{e000}:careful now\u{e000}:").html;
        assert!(html.contains("<aside class=\"plug-note\">careful now</aside>"), "{html}");
        // The sanitizer still runs: a template cannot carry a script.
        set_extensions(vec![rule("[x]", "[/x]", "<script>alert(1)</script>{}", false)]);
        assert!(!crate::cook("[x]hi[/x]").html.contains("script"));
        // Unclosed markers, empty pairs and code stay as written.
        set_extensions(vec![rule("\u{e000}", "\u{e001}", "<i>{}</i>", false)]);
        assert!(!crate::cook("a \u{e000} b").html.contains("<i>"));
        assert!(!crate::cook("`\u{e000}x\u{e001}`").html.contains("<i>"));
        set_extensions(vec![]);
    }

    #[test]
    fn rules_are_bounded() {
        let _one_at_a_time = SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(check(&rule("", "x", "y", false)).is_err());
        assert!(check(&rule(&"a".repeat(33), "x", "y", false)).is_err());
        assert!(check(&rule("a", "b", &"x".repeat(3000), false)).is_err());
        // Many markers in one line cost one pass each, capped.
        set_extensions(vec![rule("\u{e000}\u{e000}", "\u{e001}\u{e001}", "<i>{}</i>", false)]);
        let long = "\u{e000}\u{e000}c\u{e001}\u{e001}".repeat(50_000);
        assert!(crate::cook(&long).html.len() < 2_000_000);
        set_extensions(vec![]);
    }

    #[test]
    fn pairs_span_formatting_and_callbacks_wait() {
        let _one_at_a_time = SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        set_extensions(vec![rule("\u{e000}", "\u{e001}", "<span class=\"plug-s\">{}</span>", false)]);
        let html = crate::cook("a \u{e000}one **two** three\u{e001} b").html;
        assert!(html.contains("<span class=\"plug-s\">one <strong>two</strong> three</span>"), "{html}");
        let mut cb = rule("%%", "%%", "", false);
        cb.callback = Some("x".into());
        set_extensions(vec![cb]);
        let c = crate::cook("hi %%abc%% there");
        assert_eq!(c.callbacks, vec![("x".to_owned(), "abc".to_owned())]);
        let marker = format!("{}0{}", crate::PLACEHOLDER_OPEN, crate::PLACEHOLDER_CLOSE);
        assert!(c.html.contains(&marker), "{}", c.html);
        set_extensions(vec![]);
    }
}
