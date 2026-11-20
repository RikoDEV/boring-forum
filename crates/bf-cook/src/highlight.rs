//! Server-side code highlighting. Emits `hl-*` CSS classes instead of inline
//! colours, so themes style code and the sanitizer only has to allow class names.

use comrak::adapters::SyntaxHighlighterAdapter;
use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::{self, Write};
use std::sync::LazyLock;
use syntect::html::{ClassStyle, ClassedHTMLGenerator};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);

/// Longest code block that gets highlighted; longer ones render plain, which keeps a
/// pathological post from costing seconds of CPU at save time.
const MAX_LEN: usize = 64 * 1024;

pub(crate) struct Highlighter;

pub(crate) fn escape(out: &mut dyn Write, s: &str) -> fmt::Result {
    for c in s.chars() {
        match c {
            '&' => out.write_str("&amp;")?,
            '<' => out.write_str("&lt;")?,
            '>' => out.write_str("&gt;")?,
            '"' => out.write_str("&quot;")?,
            c => out.write_char(c)?,
        }
    }
    Ok(())
}

impl SyntaxHighlighterAdapter for Highlighter {
    fn write_highlighted(&self, out: &mut dyn Write, lang: Option<&str>, code: &str) -> fmt::Result {
        let syntax = lang.filter(|_| code.len() <= MAX_LEN).and_then(|l| SYNTAXES.find_syntax_by_token(l.trim()));
        let Some(syntax) = syntax else { return escape(out, code) };
        let mut r#gen =
            ClassedHTMLGenerator::new_with_class_style(syntax, &SYNTAXES, ClassStyle::SpacedPrefixed { prefix: "hl-" });
        for line in LinesWithEndings::from(code) {
            if r#gen.parse_html_for_line_which_includes_newline(line).is_err() {
                return escape(out, code);
            }
        }
        out.write_str(&r#gen.finalize())
    }

    fn write_pre_tag(&self, out: &mut dyn Write, _attrs: HashMap<&'static str, Cow<'_, str>>) -> fmt::Result {
        out.write_str("<pre>")
    }

    fn write_code_tag(&self, out: &mut dyn Write, attrs: HashMap<&'static str, Cow<'_, str>>) -> fmt::Result {
        match attrs.get("class") {
            Some(c) => {
                out.write_str("<code class=\"")?;
                escape(out, c)?;
                out.write_str("\">")
            }
            None => out.write_str("<code>"),
        }
    }
}
