//! Fuzzes the BBCode → Markdown converter.
//!
//! Two reasons it earns a target of its own. It runs on every post when BBCode is enabled, and
//! it runs on *every* post of an imported IPS or phpBB forum -- a body of input nobody wrote
//! with this parser in mind, including a decade of broken and nested markup. And it sits before
//! the sanitizer, so a converter that emits raw HTML would be handing the sanitizer something it
//! is meant to be protected from.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(raw) = std::str::from_utf8(data) else { return };
    if raw.len() > 64 * 1024 {
        return;
    }

    let markdown = bf_cook::bbcode_to_markdown(raw);

    // Converting, then cooking, is the real path for a BBCode post and for an import. The result
    // must be as inert as any other post.
    bf_cook::inert::assert_inert(&bf_cook::cook(&markdown).html, raw);

    // Deterministic: an import that runs twice must not produce two different posts.
    assert_eq!(bf_cook::bbcode_to_markdown(raw), markdown, "bbcode conversion is not deterministic for {raw:?}");
});
