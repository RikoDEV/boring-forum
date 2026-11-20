//! Regression tests for the cook pipeline's hardening fixes.

use bf_cook::{HIDDEN_MARKER, cook};

#[test]
fn private_use_pairs_after_code_do_not_amplify_output() {
    let raw = format!("[code]{}[/code]{}", "a".repeat(16_000), "\u{E000}0\u{E001}".repeat(5_000));
    assert!(raw.chars().count() < 32_000);
    let started = std::time::Instant::now();
    let html = cook(&raw).html;
    assert!(html.len() < 2 * 1024 * 1024, "output was {} bytes", html.len());
    assert!(started.elapsed().as_secs() < 30);
}

#[test]
fn typed_hidden_marker_is_not_kept_as_a_marker() {
    let html = cook(&format!("before {HIDDEN_MARKER} after")).html;
    assert!(!html.contains(HIDDEN_MARKER), "{html}");
    assert!(!html.contains("hidden content: reply to see it"), "{html}");
}

#[test]
fn real_hide_block_still_leaves_exactly_one_marker() {
    let html = cook("a [hide]secret[/hide] b").html;
    assert_eq!(html.matches(HIDDEN_MARKER).count(), 1, "{html}");
    assert!(!html.contains("secret"), "{html}");
}

#[test]
fn upload_image_size_is_capped_at_4096() {
    let sha = "a".repeat(64);
    let html = cook(&format!("![x](/uploads/{sha}.png#20000x20000)")).html;
    assert!(!html.contains("width=\"20000\"") && !html.contains("height=\"20000\""), "{html}");
    let ok = cook(&format!("![x](/uploads/{sha}.png#4096x4096)")).html;
    assert!(ok.contains("width=\"4096\""), "{ok}");
}

#[test]
fn javascript_links_and_data_images_stay_inert() {
    for raw in [
        "[x](javascript:alert(1))",
        "<a href=\"javascript:alert(1)\">x</a>",
        "![x](javascript:alert(1))",
        "![x](data:image/svg+xml;base64,PHN2ZyBvbmxvYWQ9YWxlcnQoMSk+)",
        "<img src=\"data:text/html,<script>alert(1)</script>\">",
    ] {
        let html = cook(raw).html.to_ascii_lowercase();
        assert!(!html.contains("javascript:"), "{raw} -> {html}");
        assert!(!html.contains("src=\"data:"), "{raw} -> {html}");
        assert!(!html.contains("<script"), "{raw} -> {html}");
    }
}
