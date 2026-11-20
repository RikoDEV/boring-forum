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

    // 2. Stable. Sanitizing the output again must not walk toward something executable -- the
    //    mutation-XSS class that broke several HTML sanitizers in other languages. Strict
    //    idempotence is too much: a malformed anchor nested in an anchor comes out nested once and as
    //    siblings the next time (HTML forbids nesting them), and both are inert. So the second pass
    //    must be inert too, and the third must change nothing.
    let again = bf_cook::sanitize(&clean);
    bf_cook::inert::assert_inert(&again, html);
    assert_eq!(bf_cook::sanitize(&again), again, "sanitize does not settle\n  input: {html:?}");
});
