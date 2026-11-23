//! A moment on a page: "3 h ago", "Oct 2026", "2026-11-08". In a template it renders as
//! `<time datetime="…" data-tip="2026-11-08 14:55 UTC" data-format="…">3 h ago</time>`; the islands
//! (`localTimes` in main.js, `timeText` in dom.js) then restate the words in the reader's language
//! and the full date on hover in their own time zone, so the text built here is only the English
//! fallback. In JSON it is just that text; APIs send the moment beside it (`*_at`).
//!
//! Every date the site shows goes through this type (or, in the islands, `when()` in dom.js).

use chrono::{DateTime, Utc};
use std::fmt;

/// How the words are written: relative ("3 h ago"), or one of the fixed formats.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Format {
    #[default]
    Relative,
    /// "Oct 2026"
    Month,
    /// "October 2026"
    MonthLong,
    /// "2026-11-08", shown as the reader's medium date ("Nov 8, 2026").
    Day,
    /// Date and time.
    Stamp,
}

impl Format {
    fn attr(self) -> &'static str {
        match self {
            Self::Relative => "",
            Self::Month => " data-format=\"month\"",
            Self::MonthLong => " data-format=\"month-long\"",
            Self::Day => " data-format=\"day\"",
            Self::Stamp => " data-format=\"stamp\"",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ago {
    text: String,
    /// None for a word standing where a date would, such as "never".
    at: Option<DateTime<Utc>>,
    format: Format,
}

impl Ago {
    /// "3 h ago"; older than 30 days, the date.
    pub fn new(at: DateTime<Utc>) -> Self {
        let s = (Utc::now() - at).num_seconds().max(0);
        let text = match s {
            0..60 => "just now".into(),
            60..3600 => format!("{} min ago", s / 60),
            3600..86400 => format!("{} h ago", s / 3600),
            86400..2_592_000 => format!("{} days ago", s / 86400),
            _ => at.format("%Y-%m-%d").to_string(),
        };
        Self { text, at: Some(at), format: Format::Relative }
    }

    /// "Oct 2026"
    pub fn month(at: DateTime<Utc>) -> Self {
        Self { text: at.format("%b %Y").to_string(), at: Some(at), format: Format::Month }
    }

    /// "October 2026"
    pub fn month_long(at: DateTime<Utc>) -> Self {
        Self { text: at.format("%B %Y").to_string(), at: Some(at), format: Format::MonthLong }
    }

    /// A day: "2026-11-08", in the reader's own date format once the page has loaded.
    pub fn day(at: DateTime<Utc>) -> Self {
        Self { text: at.format("%Y-%m-%d").to_string(), at: Some(at), format: Format::Day }
    }

    /// A date and time: "2026-11-08 14:55 UTC", in the reader's zone once the page has loaded.
    pub fn stamp(at: DateTime<Utc>) -> Self {
        Self { text: at.format("%Y-%m-%d %H:%M UTC").to_string(), at: Some(at), format: Format::Stamp }
    }

    /// Words standing where a date would ("never"), already in the reader's language.
    pub fn plain(text: impl Into<String>) -> Self {
        Self { text: text.into(), at: None, format: Format::Relative }
    }

    /// The English words alone, for plain text: mail, attributes, logs.
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn at(&self) -> Option<DateTime<Utc>> {
        self.at
    }

    /// RFC 3339, for scripts that show the date themselves; empty for a fixed word.
    pub fn iso(&self) -> String {
        self.at.map(|a| a.to_rfc3339()).unwrap_or_default()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

/// HTML. The text is built here from numbers and fixed words, and the dates are formatted by
/// chrono; a plain word is escaped.
impl fmt::Display for Ago {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.at {
            Some(at) => write!(
                f,
                r#"<time datetime="{}" data-tip="{}"{}>{}</time>"#,
                at.to_rfc3339(),
                at.format("%Y-%m-%d %H:%M UTC"),
                self.format.attr(),
                self.text
            ),
            None => f.write_str(&bf_core::html::esc(&self.text)),
        }
    }
}

impl askama::filters::HtmlSafe for Ago {}

impl serde::Serialize for Ago {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.text)
    }
}
