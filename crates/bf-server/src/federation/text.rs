//! Remote HTML reduced to plain text.

use serde_json::Value;

/// Remote HTML reduced to text, which is then cooked like any member's post: nothing from the
/// remote markup survives except its words and link targets.
pub(super) fn remote_text(object: &Value) -> Result<String, String> {
    let html = object
        .get("content")
        .and_then(Value::as_str)
        .or_else(|| object.pointer("/contentMap").and_then(|m| m.as_object()?.values().next()?.as_str()))
        .ok_or("no content")?;
    let text = html_to_text(html);
    let text = text.trim();
    if text.is_empty() {
        return Err("empty".into());
    }
    if text.chars().count() > 20_000 {
        return Err("too long".into());
    }
    Ok(text.to_owned())
}

pub(crate) fn strip_html(s: &str) -> String {
    html_to_text(s).trim().to_owned()
}

/// Paragraphs and line breaks become newlines; a link keeps its target (mentions and hashtags
/// keep their text); everything else keeps only its text.
pub(super) fn html_to_text(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    let mut link: Option<(String, bool, usize)> = None;
    while let Some(lt) = rest.find('<') {
        out.push_str(&decode_entities(&rest[..lt]));
        let Some(gt) = rest[lt..].find('>') else { break };
        let tag = &rest[lt + 1..lt + gt];
        let lower = tag.to_ascii_lowercase();
        let name =
            lower.trim_start_matches('/').split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or_default();
        let closing = lower.starts_with('/');
        // The contents of these are code or styling, never words: skip to the closing tag.
        if !closing && matches!(name, "script" | "style" | "template" | "noscript" | "iframe" | "svg" | "math") {
            let after = &rest[lt + gt + 1..];
            let close = format!("</{name}");
            rest = match after.to_ascii_lowercase().find(&close) {
                Some(end) => after[end..].find('>').map_or("", |g| &after[end + g + 1..]),
                None => "",
            };
            continue;
        }
        match (name, closing) {
            ("br", _) => out.push('\n'),
            ("p" | "div" | "blockquote" | "li" | "h1" | "h2" | "h3" | "h4", true) => out.push_str("\n\n"),
            ("a", false) => {
                let href = attr(tag, "href").unwrap_or_default();
                let keep_text = lower.contains("mention") || lower.contains("hashtag");
                link = Some((href, keep_text, out.len()));
            }
            ("a", true) => {
                if let Some((href, keep_text, start)) = link.take()
                    && !keep_text
                    && (href.starts_with("https://") || href.starts_with("http://"))
                {
                    out.truncate(start);
                    out.push_str(&href);
                }
            }
            _ => {}
        }
        rest = &rest[lt + gt + 1..];
    }
    out.push_str(&decode_entities(rest));
    // At most one blank line in a row.
    let mut cleaned = String::new();
    let mut blank = 0;
    for line in out.lines() {
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        cleaned.push_str(line.trim_end());
        cleaned.push('\n');
    }
    cleaned
}

pub(super) fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let at = lower.find(&format!("{name}="))? + name.len() + 1;
    let rest = &tag[at..];
    let (quote, rest) = match rest.chars().next()? {
        q @ ('"' | '\'') => (q, &rest[1..]),
        _ => (' ', rest),
    };
    let end = rest.find(quote).unwrap_or(rest.len());
    Some(decode_entities(&rest[..end]))
}

pub(super) fn decode_entities(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_html_becomes_plain_text_with_link_targets() {
        let html = r#"<p><span class="h-card"><a href="https://m.example/@bob" class="u-url mention">@<span>bob</span></a></span> hi &amp; welcome</p><p>see <a href="https://x.example/a?b=1&amp;c=2" rel="nofollow"><span class="invisible">https://</span>x.example/a…</a><br>bye</p><script>alert(1)</script>"#;
        let t = html_to_text(html);
        assert!(t.contains("@bob hi & welcome"), "{t}");
        assert!(t.contains("see https://x.example/a?b=1&c=2\nbye"), "{t}");
        assert!(!t.contains('<') && !t.contains("alert"), "script content dropped: {t}");
    }
}
