//! Post-processing of the rendered HTML: lazy images, text colours, underline and the sanitizer.

/// Widths of generated thumbnails, served at `/uploads/{sha}-{width}.{ext}`.
pub const THUMB_WIDTHS: [u32; 2] = [480, 960];

/// Lazy loading for every image. Uploads link as `/uploads/{key}#{width}x{height}`, which
/// also gives them a size (no layout shift) and a `srcset` of the smaller thumbnails.
pub(crate) fn images(html: &str) -> String {
    const SRC: &str = "<img src=\"";
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(i) = rest.find(SRC) {
        let start = i + SRC.len();
        out.push_str(&rest[..i]);
        out.push_str("<img loading=\"lazy\"");
        let Some(end) = rest[start..].find('"').map(|e| start + e) else {
            rest = &rest[i + SRC.len()..];
            out.push_str(" src=\"");
            continue;
        };
        let url = &rest[start..end];
        match upload_image(url) {
            Some((path, w, h)) => {
                out.push_str(&format!(" width=\"{w}\" height=\"{h}\" src=\"{path}\""));
                if !path.ends_with(".gif") {
                    let (stem, ext) = path.rsplit_once('.').unwrap_or((path, ""));
                    let mut set: Vec<String> =
                        THUMB_WIDTHS.iter().filter(|t| **t < w).map(|t| format!("{stem}-{t}.{ext} {t}w")).collect();
                    if !set.is_empty() {
                        set.push(format!("{path} {w}w"));
                        out.push_str(&format!(
                            " srcset=\"{}\" sizes=\"(max-width: 72rem) 100vw, 72rem\"",
                            set.join(", ")
                        ));
                    }
                }
            }
            None => match sized_image(url) {
                Some((path, w, h)) => out.push_str(&format!(" width=\"{w}\" height=\"{h}\" src=\"{path}\"")),
                None => out.push_str(&format!(" src=\"{url}\"")),
            },
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

/// The largest width or height a post may ask an image to be drawn at.
const MAX_IMAGE_SIDE: u32 = 4096;

/// (url, width, height) of any other image with a size, as the editor's resize handle writes it.
fn sized_image(url: &str) -> Option<(&str, u32, u32)> {
    let (path, size) = url.split_once('#')?;
    let (w, h) = size.split_once('x')?;
    let (w, h): (u32, u32) = (w.parse().ok()?, h.parse().ok()?);
    ((1..=MAX_IMAGE_SIDE).contains(&w) && (1..=MAX_IMAGE_SIDE).contains(&h)).then_some((path, w, h))
}

/// (path, width, height) of `/uploads/{64 hex}.{image ext}#{w}x{h}`.
fn upload_image(url: &str) -> Option<(&str, u32, u32)> {
    let (path, size) = url.split_once('#')?;
    let (w, h) = size.split_once('x')?;
    let (w, h): (u32, u32) = (w.parse().ok()?, h.parse().ok()?);
    let (sha, ext) = path.strip_prefix("/uploads/")?.split_once('.')?;
    let hex = sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_hexdigit());
    (hex && matches!(ext, "png" | "jpg" | "gif" | "webp")
        && (1..=MAX_IMAGE_SIDE).contains(&w)
        && (1..=MAX_IMAGE_SIDE).contains(&h))
    .then_some((path, w, h))
}

/// Text colours members can apply with `[text]{colour}`; also the `clr-` classes the sanitizer
/// keeps.
const COLORS: &[&str] = &["red", "orange", "yellow", "green", "blue", "purple", "gray"];

/// Underline (`++text++`) and coloured text (`[text]{colour}`), applied to the cooked HTML before
/// sanitizing. Transforms run only on text outside `<pre>`/`<code>`, so code samples keep the raw
/// markers; they emit only `<u>` and `<span class="clr-…">`, both on the sanitizer's allow-list.
pub(crate) fn inline_styles(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    let mut code_depth = 0usize;
    loop {
        if let Some(lt) = rest.find('<') {
            push_text(&mut out, &rest[..lt], code_depth);
            let gt = rest[lt..].find('>').map_or(rest.len(), |i| lt + i + 1);
            let tag = &rest[lt..gt];
            out.push_str(tag);
            let lower = tag.to_ascii_lowercase();
            if lower.starts_with("<code") || lower.starts_with("<pre") {
                code_depth += 1;
            } else if lower.starts_with("</code") || lower.starts_with("</pre") {
                code_depth = code_depth.saturating_sub(1);
            }
            rest = &rest[gt..];
        } else {
            push_text(&mut out, rest, code_depth);
            break;
        }
    }
    out
}

fn push_text(out: &mut String, text: &str, code_depth: usize) {
    if code_depth > 0 || text.is_empty() {
        out.push_str(text);
    } else {
        out.push_str(&color_spans(&underline(text)));
    }
}

/// `++text++` → `<u>text</u>`; an unmatched marker stays literal.
fn underline(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("++") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        match after.find("++") {
            Some(j) if j > 0 => {
                out.push_str("<u>");
                out.push_str(&after[..j]);
                out.push_str("</u>");
                rest = &after[j + 2..];
            }
            _ => {
                out.push_str("++");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// `[text]{colour}` → `<span class="clr-colour">text</span>` for the known colours. One linear
/// pass: track the last `[`, and when a `]{…}` closes it, emit the span. The `}` look-ahead is
/// bounded (colour names are short), so a string of brackets cannot make it quadratic.
fn color_spans(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut copied = 0;
    let mut open: Option<usize> = None; // byte index of the most recent unmatched '['
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'[' => open = Some(i),
            b']' => {
                // `]{` with no `}` within a short window, or an unknown colour, is left as text.
                if let Some(start) = open
                    && b.get(i + 1) == Some(&b'{')
                {
                    let end = (i + 2 + 10).min(b.len());
                    if let Some(rel) = b[i + 2..end].iter().position(|&c| c == b'}') {
                        let color = &s[i + 2..i + 2 + rel];
                        let inner = &s[start + 1..i];
                        if !inner.is_empty() && COLORS.contains(&color) {
                            out.push_str(&s[copied..start]);
                            out.push_str("<span class=\"clr-");
                            out.push_str(color);
                            out.push_str("\">");
                            out.push_str(inner);
                            out.push_str("</span>");
                            i = i + 2 + rel + 1;
                            copied = i;
                            open = None;
                            continue;
                        }
                    }
                }
                open = None;
            }
            _ => {}
        }
        i += 1;
    }
    out.push_str(&s[copied..]);
    out
}

pub fn sanitize(html: &str) -> String {
    ammonia::Builder::default()
        .url_schemes(["http", "https", "mailto"].into())
        .link_rel(Some("nofollow ugc noopener"))
        .add_tag_attributes("img", ["loading", "srcset", "sizes"])
        .add_tag_attributes("code", ["class", "data-math-style"])
        .add_tag_attributes("span", ["class", "data-math-style"])
        // Classes from plugin cook rules (`[[cook_rules]]`): the `plug-` prefix is theirs alone.
        .add_generic_attributes(["class"])
        .add_tag_attributes("input", ["type", "checked", "disabled"])
        .add_tags(["input", "details", "summary", "u"])
        .add_tag_attributes("h1", ["id"])
        .add_tag_attributes("h2", ["id"])
        .add_tag_attributes("h3", ["id"])
        .add_tag_attributes("h4", ["id"])
        .add_tag_attributes("h5", ["id"])
        .add_tag_attributes("h6", ["id"])
        .add_tag_attributes("a", ["id"])
        .add_tag_attributes("li", ["id"])
        .attribute_filter(|tag, attr, value| match (tag, attr) {
            ("code", "class") => value.starts_with("language-").then(|| value.into()),
            // Highlighter classes only; anything else is dropped.
            ("span", "class") => {
                let ok = value.split_whitespace().all(|c| {
                    ((c.starts_with("hl-") || c.starts_with("plug-")) && c.len() < 40)
                        || c.strip_prefix("clr-").is_some_and(|n| COLORS.contains(&n))
                });
                ok.then(|| value.into())
            }
            // The picture grid (`[grid]`) is the one first-party class a post may carry on a block.
            ("div", "class") => (matches!(
                value,
                "bf-grid"
                    | "bf-grid bf-grid-2"
                    | "bf-grid bf-grid-3"
                    | "bf-grid bf-grid-4"
                    | "bf-grid bf-grid-5"
                    | "bf-grid bf-grid-6"
            ) || value.split_whitespace().all(|c| c.starts_with("plug-") && c.len() < 40))
            .then(|| value.into()),
            (_, "class") => {
                value.split_whitespace().all(|c| c.starts_with("plug-") && c.len() < 40).then(|| value.into())
            }
            // Generated anchors only; a prefix stops ids clobbering the page's own.
            (_, "id") => (value.starts_with("h-") || value.starts_with("fn-") || value.starts_with("fnref-"))
                .then(|| value.into()),
            (_, "data-math-style") => matches!(value, "inline" | "display").then(|| value.into()),
            ("input", "type") => (value == "checkbox").then(|| value.into()),
            ("img", "loading") => (value == "lazy").then(|| value.into()),
            // Only what `images` generates: uploads, and the fixed `sizes` value.
            ("img", "srcset") => {
                let ok = value.split(", ").all(|c| {
                    c.starts_with("/uploads/") && c.bytes().all(|b| b.is_ascii_alphanumeric() || b"/-. ".contains(&b))
                });
                ok.then(|| value.into())
            }
            ("img", "sizes") => (value == "(max-width: 72rem) 100vw, 72rem").then(|| value.into()),
            _ => Some(value.into()),
        })
        .clean(html)
        .to_string()
}
