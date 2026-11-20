//! Small text helpers every part of the site shares: escaping for HTML, percent-encoding for
//! URLs and headers, and file sizes for people. One copy of each, so a fix lands everywhere.

/// Text made safe for HTML element content and quoted attribute values (also XML).
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// RFC 3986 percent-encoding: everything but letters, digits and `-._~` is encoded, so the
/// result fits any URL component, a query value or an RFC 5987 header parameter.
pub fn url_encode(s: &str) -> String {
    url_encode_keeping(s, b"")
}

/// The same, also leaving the bytes in `keep` as they are (`/` for a path).
pub fn url_encode_keeping(s: &str, keep: &[u8]) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) || keep.contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `Content-Disposition: attachment` with the file name RFC 5987-encoded, so any name is safe.
pub fn attachment(filename: &str) -> String {
    format!("attachment; filename*=UTF-8''{}", url_encode(filename))
}

/// Characters that show nothing or change how the text around them is laid out: zero-width space
/// and non-joiner, word joiner, byte-order mark, and the left-to-right/right-to-left marks,
/// embeddings and overrides. A name or title made of them looks empty, and one with an override
/// can show other text reversed. The zero-width joiner stays: emoji sequences need it.
pub fn is_hidden_or_bidi(c: char) -> bool {
    matches!(c, '\u{200B}' | '\u{200C}' | '\u{2060}' | '\u{FEFF}' | '\u{061C}' | '\u{200E}' | '\u{200F}')
        || ('\u{202A}'..='\u{202E}').contains(&c)
        || ('\u{2066}'..='\u{2069}').contains(&c)
}

/// `text` without those characters.
pub fn strip_hidden_chars(text: &str) -> String {
    text.chars().filter(|c| !is_hidden_or_bidi(*c)).collect()
}

/// A file size as people read it: `512 B`, `34 KB`, `2.5 MB`.
pub fn human_size(bytes: i64) -> String {
    match bytes {
        ..1024 => format!("{} B", bytes.max(0)),
        1024..1_048_576 => format!("{} KB", bytes / 1024),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_every_markup_character() {
        assert_eq!(esc(r#"<a href="x">'&'</a>"#), "&lt;a href=&quot;x&quot;&gt;&#39;&amp;&#39;&lt;/a&gt;");
        assert_eq!(esc("zażółć"), "zażółć");
    }

    #[test]
    fn percent_encoding() {
        assert_eq!(url_encode("a b/ć~"), "a%20b%2F%C4%87~");
        assert_eq!(url_encode_keeping("/t/a b", b"/"), "/t/a%20b");
        assert_eq!(attachment("my file.zip"), "attachment; filename*=UTF-8''my%20file.zip");
    }

    #[test]
    fn sizes() {
        assert_eq!(human_size(10), "10 B");
        assert_eq!(human_size(4096), "4 KB");
        assert_eq!(human_size(3 * 1_048_576 / 2), "1.5 MB");
    }
}
