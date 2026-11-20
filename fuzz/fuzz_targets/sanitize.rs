//! Fuzzes the sanitizer on its own. `cook` only feeds it HTML that comrak produced,
//! but the sanitizer is also the boundary for imported HTML, plugin output and theme-rendered
//! strings, so it has to hold for arbitrary markup rather than only for well-formed input.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(html) = std::str::from_utf8(data) else { return };
    if html.len() > 64 * 1024 {
        return;
    }

    let clean = bf_cook::sanitize(html);

    // 1. Whatever went in, what comes out must be inert.
    bf_cook::inert::assert_inert(&clean, html);

    // 2. Idempotent. If sanitizing twice differs, the sanitizer's own output is parsed
    //    differently than it was written -- a mutation-XSS primitive, and the bug class that
    //    broke several HTML sanitizers in other languages.
    assert_eq!(bf_cook::sanitize(&clean), clean, "sanitize is not idempotent\n  input: {html:?}");
});
