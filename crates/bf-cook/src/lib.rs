//! Markdown → sanitized HTML ("cooking", done once on save, never per view).
//!
//! Pipeline: comrak (CommonMark + GFM, raw HTML disabled) → AST passes (mentions)
//! → HTML → ammonia allow-list sanitizer. The sanitizer runs last, so a bug in an
//! earlier pass can never emit unsafe markup.

mod bbcode;
/// The BBCode → Markdown converter, exposed for the fuzz targets and the importers.
/// Posting goes through [`cook`], which runs this first when BBCode is enabled.
pub use bbcode::to_markdown as bbcode_to_markdown;
pub use bbcode::{set_bbcode, set_custom_emoji, set_plugin_emoji};
mod extension;
pub use extension::{MAX_CALLBACKS, PLACEHOLDER_CLOSE, PLACEHOLDER_OPEN, Rule as ExtensionRule, check as check_extension, set_extensions};
mod highlight;
mod html;
pub mod inert;

pub use html::{THUMB_WIDTHS, sanitize};
use html::{images, inline_styles};

use comrak::nodes::{AstNode, NodeLink, NodeValue};
use comrak::options::Plugins;
use comrak::{Arena, Options, format_html_with_plugins, parse_document};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};

static MATH: AtomicBool = AtomicBool::new(false);

/// `$…$`, `$$…$$` and ```` ```math ```` (the `math_enabled` setting). The cooker only marks the
/// TeX source; the browser typesets it with KaTeX as MathML, so nothing is evaluated here.
pub fn set_math(on: bool) {
    MATH.store(on, Ordering::Relaxed);
}

/// Bump when the output format changes so stored posts can be re-cooked by a job.
pub const COOK_VERSION: i32 = 5;

#[derive(Debug, Default, PartialEq)]
pub struct Cooked {
    pub html: String,
    /// Lowercased usernames mentioned with `@name`, for notifications.
    pub mentions: Vec<String>,
    /// Absolute link targets, for link limits and duplicate-link warnings.
    pub links: Vec<String>,
    /// Text a plugin's `callback` cook rules wait for: (rule name, text). Each stands in `html` as
    /// `PLACEHOLDER_OPEN`, its index, `PLACEHOLDER_CLOSE` until the answer replaces it.
    pub callbacks: Vec<(String, String)>,
}

fn options() -> Options<'static> {
    let mut o = Options::default();
    o.extension.strikethrough = true;
    o.extension.table = true;
    o.extension.autolink = true;
    o.extension.tasklist = true;
    o.extension.shortcodes = true;
    o.extension.footnotes = true;
    o.extension.header_id_prefix = Some("h-".into());
    let math = MATH.load(Ordering::Relaxed);
    o.extension.math_dollars = math;
    o.extension.math_code = math;
    o.render.r#unsafe = false;
    o
}

/// What a `[hide]` block leaves in a post's stored HTML. The hidden text is not in that HTML at
/// all, so feeds, excerpts, API answers and plugins cannot leak it; a topic page swaps the
/// marker for the block, cooked from the source, for members allowed to see it.
pub const HIDDEN_MARKER: &str = "⟦hidden content: reply to see it⟧";

/// Splits out `[hide]…[/hide]` and "```hide" fences. Must match `bf_strip_hidden` in the database.
pub fn split_hidden(raw: &str) -> (String, Vec<String>) {
    let mut blocks = Vec::new();
    let mut rest = raw;
    let mut out = String::with_capacity(raw.len());
    loop {
        let tag = rest.find("[hide]");
        let fence = rest
            .match_indices("```hide")
            .map(|(i, _)| i)
            .find(|i| !rest[i + 7..].starts_with(|c: char| c.is_alphanumeric() || c == '_'));
        let (start, is_fence) = match (tag, fence) {
            (Some(t), Some(f)) if f < t => (f, true),
            (Some(t), _) => (t, false),
            (None, Some(f)) => (f, true),
            (None, None) => break,
        };
        let (open, close) = if is_fence { (7, "```") } else { (6, "[/hide]") };
        let body = &rest[start + open..];
        let (inner, after) = match body.find(close) {
            Some(end) => (&body[..end], &body[end + close.len()..]),
            // An unclosed fence runs to the end; an unclosed tag is just text.
            None if is_fence => (body, ""),
            None => {
                out.push_str(&rest[..start + open]);
                rest = body;
                continue;
            }
        };
        out.push_str(&rest[..start]);
        out.push_str(HIDDEN_MARKER);
        blocks.push(inner.trim().to_owned());
        rest = after;
    }
    out.push_str(rest);
    (out, blocks)
}

/// A post's source without its hidden blocks, for anyone not allowed to see them.
pub fn strip_hidden(raw: &str) -> String {
    split_hidden(raw).0
}

/// The stored HTML with each marker replaced by its block, cooked.
pub fn reveal_hidden(cooked: &str, raw: &str) -> String {
    let (_, blocks) = split_hidden(raw);
    let mut html = cooked.to_owned();
    for block in blocks {
        let shown = format!("<blockquote>{}</blockquote>", cook(&block).html);
        html = html.replacen(HIDDEN_MARKER, &shown, 1);
    }
    // The marker may sit inside an attribute; the result is only safe once parsed again.
    sanitize(&html)
}

pub fn cook(raw: &str) -> Cooked {
    // Nested cooks (spoilers) add to the outermost post's callback list.
    let outermost = SPOILER_DEPTH.with(std::cell::Cell::get) == 0;
    if outermost {
        extension::begin();
    }
    // The callback placeholder's characters never come from a writer: a typed one would be taken
    // for the cooker's own, and a plugin's answer would land wherever it was typed (an attribute).
    // The hidden-content marker is the cooker's own too: a typed one would take the place of a
    // real block when it is shown.
    let raw = strip_hidden(&raw.replace(HIDDEN_MARKER, "")).replace(
        [extension::PLACEHOLDER_OPEN, extension::PLACEHOLDER_CLOSE],
        "",
    );
    let arena = Arena::new();
    let opts = options();
    let raw = grid_fences(&bbcode::to_markdown(&limit_quote_depth(&raw)));
    let root = parse_document(&arena, &raw, &opts);

    let mentions = link_mentions(&arena, root);
    let links = collect_links(root);
    oneboxes(&arena, root);
    extension::apply(&arena, root);

    let mut plugins = Plugins::default();
    plugins.render.codefence_syntax_highlighter = Some(&highlight::Highlighter);
    plugins.render.codefence_renderers.insert("spoiler".to_owned(), &Spoiler);
    plugins.render.codefence_renderers.insert("grid".to_owned(), &Grid);
    let mut html = String::new();
    #[expect(clippy::expect_used, reason = "the formatter only fails when the writer does, and a String never does")]
    format_html_with_plugins(root, &opts, &mut html, &plugins).expect("formatting into a String cannot fail");
    let callbacks = if outermost { extension::take() } else { Vec::new() };
    Cooked { html: sanitize(&inline_styles(&images(&html))), mentions: mentions.into_iter().collect(), links, callbacks }
}

/// `[grid]` (or `[grid=3]` for three columns, 2 to 6) and `[/grid]` on lines of their own (a block of
/// pictures shown side by side, as Discourse writes it) become a ```` ```grid ```` fence. A marker without its partner, or inside a code fence,
/// stays the text it is.
fn grid_fences(raw: &str) -> String {
    if !raw.contains("[grid") {
        return raw.to_owned();
    }
    let lines: Vec<&str> = raw.split('\n').collect();
    // What each line is: `Open(columns)` starts a grid, `Close` ends it.
    #[derive(Clone, Copy)]
    enum Mark {
        Open(Option<u8>),
        Close,
    }
    let mut mark: Vec<Option<Mark>> = vec![None; lines.len()];
    let mut fence: Option<(char, usize)> = None;
    let mut open: Option<(usize, Option<u8>)> = None;
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        let run = |c: char| t.chars().take_while(|x| *x == c).count();
        if let Some((c, n)) = fence {
            if run(c) >= n && t.trim_start_matches(c).trim().is_empty() {
                fence = None;
            }
            continue;
        }
        let (ticks, tildes) = (run('`'), run('~'));
        if ticks >= 3 {
            fence = Some(('`', ticks));
            continue;
        }
        if tildes >= 3 {
            fence = Some(('~', tildes));
            continue;
        }
        let text = line.trim();
        let columns = text
            .strip_prefix("[grid=")
            .and_then(|r| r.strip_suffix(']'))
            .and_then(|n| n.parse::<u8>().ok())
            .filter(|n| (2..=6).contains(n));
        if text == "[grid]" {
            open = Some((i, None));
        } else if columns.is_some() {
            open = Some((i, columns));
        } else if text == "[/grid]"
            && let Some((o, columns)) = open.take()
        {
            mark[o] = Some(Mark::Open(columns));
            mark[i] = Some(Mark::Close);
        }
    }
    let out: Vec<String> = lines
        .iter()
        .zip(&mark)
        .map(|(line, m)| match m {
            Some(Mark::Open(Some(n))) => format!("```grid {n}"),
            Some(Mark::Open(None)) => "```grid".to_owned(),
            Some(Mark::Close) => "```".to_owned(),
            None => (*line).to_owned(),
        })
        .collect();
    out.join("\n")
}

/// ```` ```grid ```` fences render the pictures inside side by side. The inner text goes through the
/// whole cooker like a spoiler's, and the outer sanitizer sees it again.
struct Grid;

impl comrak::adapters::CodefenceRendererAdapter for Grid {
    fn write(
        &self,
        out: &mut dyn std::fmt::Write,
        _lang: &str,
        meta: &str,
        code: &str,
        _pos: Option<comrak::nodes::Sourcepos>,
    ) -> std::fmt::Result {
        let depth = SPOILER_DEPTH.with(std::cell::Cell::get);
        if depth >= MAX_SPOILER_DEPTH {
            out.write_str("<pre>")?;
            highlight::escape(out, code)?;
            return out.write_str("</pre>");
        }
        SPOILER_DEPTH.with(|d| d.set(depth + 1));
        let inner = cook(code).html;
        SPOILER_DEPTH.with(|d| d.set(depth));
        // The number of columns, when the writer chose one.
        match meta.trim().parse::<u8>().ok().filter(|n| (2..=6).contains(n)) {
            Some(n) => write!(out, "<div class=\"bf-grid bf-grid-{n}\">{inner}</div>"),
            None => write!(out, "<div class=\"bf-grid\">{inner}</div>"),
        }
    }
}

/// ```` ```spoiler ```` fences render as a collapsed block with Markdown inside.
/// The inner text goes through the whole cooker, and the outer sanitizer sees it again.
struct Spoiler;

/// How many spoilers may sit inside each other. Deeper fences are shown as plain text, so a
/// post of nested fences cannot overflow the stack or cost quadratic work.
const MAX_SPOILER_DEPTH: usize = 8;

thread_local! {
    static SPOILER_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl comrak::adapters::CodefenceRendererAdapter for Spoiler {
    fn write(
        &self,
        out: &mut dyn std::fmt::Write,
        _lang: &str,
        _meta: &str,
        code: &str,
        _pos: Option<comrak::nodes::Sourcepos>,
    ) -> std::fmt::Result {
        let depth = SPOILER_DEPTH.with(std::cell::Cell::get);
        if depth >= MAX_SPOILER_DEPTH {
            out.write_str("<pre>")?;
            highlight::escape(out, code)?;
            return out.write_str("</pre>");
        }
        SPOILER_DEPTH.with(|d| d.set(depth + 1));
        let inner = cook(code).html;
        SPOILER_DEPTH.with(|d| d.set(depth));
        write!(out, "<details><summary>Spoiler</summary>{inner}</details>")
    }
}

fn inside_link<'a>(node: &'a AstNode<'a>) -> bool {
    node.ancestors().any(|n| matches!(n.data.borrow().value, NodeValue::Link(_) | NodeValue::Image(_)))
}

/// Letters of any script, for sites with `unicode_usernames` on.
fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.')
}

/// Splits text into plain runs and `@name` mentions.
fn split_mentions(text: &str) -> Vec<(bool, &str)> {
    let mut out = Vec::new();
    let mut last = 0;
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Only an `@` is sure to be a char boundary; `i` can be inside a multi-byte character.
        if bytes[i] == b'@' && (i == 0 || !is_name_char(text[..i].chars().next_back().unwrap_or(' '))) {
            let start = i + 1;
            let len = text[start..].find(|c: char| !is_name_char(c)).unwrap_or(text.len() - start);
            // Names may not end with punctuation: "@bob." mentions "bob".
            let name = text[start..start + len].trim_end_matches(['.', '-']);
            if (2..=30).contains(&name.len()) {
                if last < i {
                    out.push((false, &text[last..i]));
                }
                out.push((true, name));
                i = start + name.len();
                last = i;
                continue;
            }
        }
        i += 1;
    }
    if last < text.len() {
        out.push((false, &text[last..]));
    }
    out
}

fn link_mentions<'a>(arena: &'a Arena<'a>, root: &'a AstNode<'a>) -> BTreeSet<String> {
    let texts: Vec<_> = root
        .descendants()
        .filter(|n| matches!(&n.data.borrow().value, NodeValue::Text(t) if t.contains('@')))
        .filter(|n| !inside_link(n))
        .collect();
    let mut found = BTreeSet::new();
    for node in texts {
        let text = match &node.data.borrow().value {
            NodeValue::Text(t) => t.to_string(),
            _ => continue,
        };
        let parts = split_mentions(&text);
        if !parts.iter().any(|(m, _)| *m) {
            continue;
        }
        for (is_mention, s) in parts {
            let new = if is_mention {
                let lower = s.to_lowercase();
                let link: &AstNode = arena.alloc(
                    NodeValue::Link(Box::new(NodeLink { url: format!("/u/{lower}"), title: String::new() })).into(),
                );
                link.append(arena.alloc(NodeValue::Text(format!("@{s}").into()).into()));
                found.insert(lower);
                link
            } else {
                &*arena.alloc(NodeValue::Text(s.to_owned().into()).into())
            };
            node.insert_before(new);
        }
        node.detach();
    }
    found
}

/// The 11-character id of a YouTube watch or short link.
fn youtube_id(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let id = if let Some(r) = rest.strip_prefix("youtu.be/") {
        r.split(['?', '&', '#']).next()?
    } else {
        let r = rest.strip_prefix("www.").unwrap_or(rest).strip_prefix("youtube.com/watch?")?;
        r.split('&').find_map(|kv| kv.strip_prefix("v="))?.split('#').next()?
    };
    (id.len() == 11 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')).then_some(id)
}

/// Oneboxes for allow-listed sites: a paragraph that is only a bare YouTube
/// link becomes a thumbnail card linking to the video. Built from AST nodes, so no
/// raw HTML (and no iframe) is ever produced; nothing is fetched at cook time.
fn oneboxes<'a>(arena: &'a Arena<'a>, root: &'a AstNode<'a>) {
    let paras: Vec<_> = root.children().filter(|n| matches!(n.data.borrow().value, NodeValue::Paragraph)).collect();
    for p in paras {
        let mut kids = p.children();
        let (Some(link), None) = (kids.next(), kids.next()) else { continue };
        let url = match &link.data.borrow().value {
            NodeValue::Link(l) => l.url.clone(),
            _ => continue,
        };
        let text: String = link
            .descendants()
            .filter_map(|n| match &n.data.borrow().value {
                NodeValue::Text(t) => Some(t.to_string()),
                _ => None,
            })
            .collect();
        if text != url {
            continue; // a link with its own words stays a link
        }
        let Some(id) = youtube_id(&url) else { continue };
        for c in link.children().collect::<Vec<_>>() {
            c.detach();
        }
        let img: &AstNode = arena.alloc(
            NodeValue::Image(Box::new(NodeLink {
                url: format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg"),
                title: String::new(),
            }))
            .into(),
        );
        img.append(arena.alloc(NodeValue::Text("YouTube video".into()).into()));
        link.append(img);
    }
}

/// Schemes are case-insensitive and `//host` inherits one; browsers follow all of them.
fn is_web_link(url: &str) -> bool {
    let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://") || url.starts_with("//")
}

fn collect_links<'a>(root: &'a AstNode<'a>) -> Vec<String> {
    let mut links: Vec<String> = root
        .descendants()
        .filter_map(|n| match &n.data.borrow().value {
            NodeValue::Link(l) if is_web_link(&l.url) => Some(l.url.clone()),
            _ => None,
        })
        .collect();
    links.dedup();
    links
}

/// Deeper block quotes than anyone writes by hand. Parsing cost grows with the square of the
/// depth, so a post of nothing but `>` would otherwise tie up a CPU for seconds.
const MAX_QUOTE_DEPTH: usize = 16;

fn limit_quote_depth(raw: &str) -> std::borrow::Cow<'_, str> {
    let too_deep = |line: &str| {
        line.bytes().take_while(|b| matches!(b, b'>' | b' ')).filter(|b| *b == b'>').count() > MAX_QUOTE_DEPTH
    };
    if !raw.lines().any(too_deep) {
        return raw.into();
    }
    let mut out = String::with_capacity(raw.len());
    for line in raw.split_inclusive('\n') {
        if too_deep(line) {
            let start = line.bytes().take_while(|b| matches!(b, b'>' | b' ')).count();
            out.push_str(&">".repeat(MAX_QUOTE_DEPTH));
            out.push(' ');
            out.push_str(&line[start..]);
        } else {
            out.push_str(line);
        }
    }
    out.into()
}

#[cfg(test)]
mod tests;
