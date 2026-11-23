//! Tests for the post cooker: math, mentions, sanitising and the no-panic property test.

use super::*;

#[test]
fn math_is_marked_only_when_enabled() {
    assert!(!cook("$x^2$").html.contains("data-math-style"));
    set_math(true);
    let html = cook(r"Euler: $e^{i\pi} + 1 = 0$ and $$\int_0^1 x\,dx$$").html;
    set_math(false);
    assert!(html.contains("<span data-math-style=\"inline\">"), "{html}");
    assert!(html.contains("<span data-math-style=\"display\">"), "{html}");
    assert!(!sanitize("<span data-math-style=\"x\" onclick=\"a()\">t</span>").contains("data-math-style"));
}

#[test]
fn images_keep_a_chosen_size() {
    let html = cook("![a](https://example.com/a.png#320x200)").html;
    assert!(html.contains("width=\"320\"") && html.contains("height=\"200\""), "{html}");
    assert!(!html.contains("#320x200"), "{html}");
}

#[test]
fn non_ascii_text_does_not_panic() {
    assert_eq!(cook("— zażółć gęślą @bob 👍").mentions, ["bob"]);
}

#[test]
fn hidden_blocks_never_reach_stored_html() {
    let raw = "open [hide]secret one[/hide] mid\n\n```hide\nsecret two\n```\n\nend";
    let c = cook(raw);
    assert!(!c.html.contains("secret"));
    assert_eq!(c.html.matches(HIDDEN_MARKER).count(), 2);
    let shown = reveal_hidden(&c.html, raw);
    assert!(shown.contains("secret one") && shown.contains("secret two"));
    assert!(!strip_hidden(raw).contains("secret"));
    assert!(strip_hidden("an [hide] with no end").contains("[hide]"));
}

#[test]
fn hidden_blocks_are_sanitized_when_shown() {
    let raw = "[hide]<script>alert(1)</script>ok[/hide]";
    assert!(!reveal_hidden(&cook(raw).html, raw).contains("<script"));
}

#[test]
fn upload_images_get_size_and_srcset() {
    let sha = "a".repeat(64);
    let html = cook(&format!("![x](/uploads/{sha}.png#1000x500)")).html;
    assert!(html.contains("width=\"1000\""), "{html}");
    assert!(html.contains("loading=\"lazy\""), "{html}");
    assert!(
        html.contains(&format!("/uploads/{sha}-480.png 480w, /uploads/{sha}-960.png 960w, /uploads/{sha}.png 1000w")),
        "{html}"
    );
    assert!(!html.contains("#1000x500"), "{html}");
    // No thumbnails bigger than the image, none for GIFs, none for foreign images.
    assert!(cook(&format!("![x](/uploads/{sha}.png#300x300)")).html.contains("width=\"300\""));
    assert!(!cook(&format!("![x](/uploads/{sha}.png#300x300)")).html.contains("srcset"));
    assert!(!cook(&format!("![x](/uploads/{sha}.gif#2000x300)")).html.contains("srcset"));
    let foreign = cook("![x](https://example.com/a.png#1000x500)").html;
    assert!(foreign.contains("loading=\"lazy\"") && !foreign.contains("srcset"), "{foreign}");
}

#[test]
fn pathological_posts_cook_quickly() {
    // The largest post the settings allow, on a small stack, in a debug build.
    for unit in
        [">", "> ", "- ", "[", "](", "*", "**a", "`", "_", "<", "![", "\n\n- ", "| ", "[^1]", ":a:", "[b]", "[quote]"]
    {
        let raw = unit.repeat(200_000 / unit.len());
        let started = std::time::Instant::now();
        let done = std::thread::Builder::new().stack_size(2 * 1024 * 1024).spawn(move || cook(&raw)).unwrap().join();
        assert!(done.is_ok(), "{unit:?} crashed");
        assert!(started.elapsed().as_secs() < 30, "{unit:?} took {:?}", started.elapsed());
    }
}

#[test]
fn normal_quotes_are_untouched() {
    assert_eq!(limit_quote_depth("> a\n>> b\n"), "> a\n>> b\n");
    assert_eq!(limit_quote_depth(&">".repeat(40)).matches('>').count(), MAX_QUOTE_DEPTH);
}

#[test]
fn image_attributes_cannot_be_injected() {
    let html = cook("![x](</uploads/a\" onerror=\"alert(1).png#1x1>)").html;
    assert!(html.contains("<img"), "must parse as an image: {html}");
    assert!(!html.contains("onerror=\""), "{html}");
}

#[test]
fn basic_markdown() {
    let c = cook("**bold** and `code`\n\n- a\n- b");
    assert!(c.html.contains("<strong>bold</strong>"));
    assert!(c.html.contains("<code>code</code>"));
    assert!(c.html.contains("<li>a</li>"));
}

#[test]
fn raw_html_and_scripts_are_removed() {
    for evil in [
        "<script>alert(1)</script>",
        "<img src=x onerror=alert(1)>",
        "[x](javascript:alert(1))",
        "<a href=\"javascript:alert(1)\">x</a>",
        "![x](javascript:alert(1))",
    ] {
        let html = cook(evil).html;
        assert!(!html.contains("<script"), "{evil} -> {html}");
        assert!(!html.contains("onerror"), "{evil} -> {html}");
        assert!(!html.contains("javascript:"), "{evil} -> {html}");
    }
}

#[test]
fn links_get_ugc_rel_and_are_collected() {
    let c = cook("see https://example.com/a and [b](https://example.org)");
    assert!(c.html.contains("rel=\"nofollow ugc noopener\""));
    assert_eq!(c.links, ["https://example.com/a", "https://example.org"]);
}

#[test]
fn mentions_are_linked_outside_code_and_links() {
    let c = cook("hi @Alice and @bob. mail a@b.c `@carol` [@dave](https://x.y)\n\n```\n@erin\n```");
    assert_eq!(c.mentions, ["alice", "bob"]);
    assert!(c.html.contains("<a href=\"/u/alice\" rel=\"nofollow ugc noopener\">@Alice</a>"));
    assert!(c.html.contains("<code>@carol</code>"));
    assert!(!c.html.contains("/u/carol") && !c.html.contains("/u/erin") && !c.html.contains("/u/dave"));
}

#[test]
fn code_language_class_kept_other_classes_dropped() {
    let html = cook("```rust\nfn main() {}\n```").html;
    assert!(html.contains("class=\"language-rust\""), "{html}");
}

#[test]
fn code_is_highlighted_with_classes_only() {
    let html = cook(
        "```rust
fn main() { let x = 1; }
```",
    )
    .html;
    assert!(html.contains("class=\"hl-"), "{html}");
    assert!(!html.contains("style="), "{html}");
    // Unknown languages and plain fences still escape their content.
    let plain = cook(
        "```nosuchlang
<script>x</script>
```",
    )
    .html;
    assert!(plain.contains("&lt;script&gt;"), "{plain}");
}

#[test]
fn foreign_span_classes_are_dropped() {
    assert!(!cook("<span class=\"evil\">x</span>").html.contains("evil"));
}

#[test]
fn youtube_links_become_thumbnail_cards() {
    let html = cook("https://www.youtube.com/watch?v=dQw4w9WgXcQ").html;
    assert!(html.contains("src=\"https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg\""), "{html}");
    assert!(html.contains("href=\"https://www.youtube.com/watch?v=dQw4w9WgXcQ\""));
    assert!(!html.contains("<iframe"));
    let short = cook("https://youtu.be/dQw4w9WgXcQ?t=5").html;
    assert!(short.contains("i.ytimg.com/vi/dQw4w9WgXcQ/"), "{short}");
    // Inside a sentence, or with a bad id, it stays a normal link.
    assert!(!cook("see https://youtu.be/dQw4w9WgXcQ here").html.contains("ytimg"));
    assert!(!cook("https://youtu.be/short").html.contains("ytimg"));
}

#[test]
fn anchors_footnotes_and_spoilers() {
    assert!(cook("# Hello there").html.contains("id=\"h-hello-there\""));
    let f = cook("Claim[^1]\n\n[^1]: Source");
    assert!(f.html.contains("Source") && f.html.contains("href=\"#fn-1\""), "{}", f.html);
    let s = cook("```spoiler\n**secret** <script>x</script>\n```").html;
    assert!(s.contains("<details>") && s.contains("<strong>secret</strong>") && !s.contains("<script"), "{s}");
}

#[test]
fn emoji_shortcodes() {
    assert!(cook("hello :smile:").html.contains('😄'));
}

#[test]
fn tables_and_tasklists() {
    let html = cook("| a | b |\n|---|---|\n| 1 | 2 |\n\n- [x] done").html;
    assert!(html.contains("<table>"));
    assert!(html.contains("type=\"checkbox\""));
}

/// Spoiler fences cook their inner text again; nesting is bounded, and the cost stays small.
#[test]
fn nested_spoilers_are_bounded() {
    let mut raw = String::from("deep <script>x</script>");
    for level in 0..200 {
        let fence = "`".repeat(3 + level + 1);
        raw = format!(
            "{fence}spoiler
{raw}
{fence}"
        );
    }
    let html = cook(&raw).html;
    assert!(html.matches("<details>").count() <= 9, "nesting is capped");
    assert!(!html.contains("<script"));
    // The depth counter is restored: a second post is not affected.
    assert!(
        cook(
            "```spoiler
hi
```"
        )
        .html
        .contains("<details>")
    );
}

/// Random mixes of the markup the converters scan for, plus multi-byte text, must never panic:
/// a panic in a request path aborts the whole server.
#[test]
fn random_markup_never_panics() {
    const TOKENS: &[&str] = &[
        "[",
        "]",
        "[/",
        "[hide]",
        "[/hide]",
        "```hide",
        "```spoiler",
        "````spoiler",
        "````",
        "```",
        "`",
        "[url=",
        "[/url]",
        "[img]",
        "[/img]",
        "[quote=\"",
        "[/quote]",
        "[spoiler]",
        "[/spoiler]",
        "[b]",
        "[/b]",
        "=",
        "\"",
        "'",
        ":",
        ":smile:",
        "@",
        "@bob",
        "{",
        "}",
        "{#",
        "{#ff0000}",
        "<",
        ">",
        "</",
        "<img src=\"",
        "<span style=\"",
        "/uploads/",
        ".png",
        "#10x20",
        "<!--",
        "-->",
        "\u{130}",
        "\u{1f44d}",
        "\u{e9}",
        "\u{200b}",
        "\u{fb03}",
        "\n",
        " ",
        "a",
        "$",
        "$$",
        "![",
        "](",
        ">",
        "> ",
        "[^1]",
        "||",
        "++",
        "~~",
        "!!",
    ];
    let mut seed = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for _ in 0..4000 {
        let n = (next() % 24) as usize;
        let raw: String = (0..n).map(|_| TOKENS[(next() % TOKENS.len() as u64) as usize]).collect();
        let done = std::panic::catch_unwind(|| {
            let c = cook(&raw);
            let _ = (split_hidden(&raw), strip_hidden(&raw), reveal_hidden(&c.html, &raw), sanitize(&raw));
            let _ = (bbcode::to_markdown(&raw), html::images(&raw), html::inline_styles(&raw), inert::violations(&raw));
        });
        assert!(done.is_ok(), "panicked on {raw:?}");
    }
}

#[test]
fn grid_blocks_put_pictures_side_by_side() {
    let sha = "a".repeat(64);
    let html = cook(&format!(
        "before\n\n[grid]\n![a](/uploads/{sha}.png#100x80)\n![b](https://example.org/b.png)\n[/grid]\n\nafter"
    ))
    .html;
    assert!(html.contains("<div class=\"bf-grid\">"), "{html}");
    assert_eq!(html.matches("<img").count(), 2, "{html}");
    assert!(html.contains("before") && html.contains("after"), "{html}");
    assert!(!html.contains("[grid]") && !html.contains("[/grid]"), "{html}");
}

#[test]
fn grid_markers_stay_text_without_a_partner_or_inside_code() {
    let alone = cook("[grid]\n![a](https://example.org/a.png)").html;
    assert!(!alone.contains("bf-grid") && alone.contains("[grid]"), "{alone}");
    let fenced = cook("```\n[grid]\n![a](https://example.org/a.png)\n[/grid]\n```").html;
    assert!(!fenced.contains("bf-grid"), "{fenced}");
}

#[test]
fn only_the_grid_class_survives_on_a_block() {
    assert!(crate::html::sanitize("<div class=\"bf-grid\">x</div>").contains("class=\"bf-grid\""));
    assert!(!crate::html::sanitize("<div class=\"admin-only\">x</div>").contains("class="));
}

#[test]
fn a_grid_can_say_how_many_columns() {
    let three = cook("[grid=3]\n![a](https://example.org/a.png)\n![b](https://example.org/b.png)\n[/grid]").html;
    assert!(three.contains("<div class=\"bf-grid bf-grid-3\">"), "{three}");
    // Out of range, or not a number: an automatic grid is not made from it, the text stays.
    let odd = cook("[grid=9]\n![a](https://example.org/a.png)\n[/grid]").html;
    assert!(!odd.contains("bf-grid"), "{odd}");
    let plain = cook("[grid]\n![a](https://example.org/a.png)\n[/grid]").html;
    assert!(plain.contains("<div class=\"bf-grid\">"), "{plain}");
}

#[test]
fn malformed_anchors_settle_after_one_more_pass_and_stay_inert() {
    // Found by the fuzzer. The parser keeps a second `<a>` inside the first here, and the next parse
    // makes them siblings (an anchor cannot contain an anchor). Both forms are inert; what must hold is
    // that the output is stable from the second pass on, and that no pass is executable.
    let input = "<A\n\0&\u{6}#>b\u{19}\0\0\0/\ts\u{8}=\0\0/\t;/o><\u{4}<marquee>\0\0\0/>&<;/><A\n</\0>\0o";
    let once = sanitize(input);
    let twice = sanitize(&once);
    assert_eq!(sanitize(&twice), twice, "{once:?} -> {twice:?}");
    inert::assert_inert(&once, input);
    inert::assert_inert(&twice, input);
}

#[test]
fn a_code_fence_named_like_a_css_expression_is_inert() {
    // Found by the fuzzers (bbcode and hidden): the language of a fence becomes the class of the code
    // element, and "expression(" in a class is only text.
    for md in ["~~~\u{c}expression(", "```hexpression(\n@~@<\0\n:/1\r-*1\r\r[*~Ɍ\n&a`h\n"] {
        let html = cook(md).html;
        inert::assert_inert(&html, md);
    }
}

#[test]
fn an_address_in_angle_brackets_stays_a_plain_link() {
    // A bare address alone in its paragraph is a candidate for a link preview card: the link is the
    // paragraph's only child.
    let bare = cook("https://example.com/page").html;
    assert!(bare.starts_with("<p><a "), "{bare}");
    // In angle brackets it is wrapped, so the page's script leaves it alone.
    let angle = cook("<https://example.com/page>").html;
    assert!(angle.starts_with("<p><u><a "), "{angle}");
    assert!(angle.contains("href=\"https://example.com/page\""), "{angle}");
    // No YouTube thumbnail either.
    let video = cook("<https://www.youtube.com/watch?v=dQw4w9WgXcQ>").html;
    assert!(!video.contains("<img"), "{video}");
    // A link with its own words, or one inside a sentence, is not affected.
    assert!(cook("a <https://example.com/page> b").html.contains("<a "));
}
