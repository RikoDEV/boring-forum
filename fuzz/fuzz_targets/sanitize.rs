//! Fuzzes the sanitizer on its own. `cook` only feeds it HTML that comrak produced,
//! but the sanitizer is also the boundary for imported HTML, plugin output and theme-rendered
//! strings, so it has to hold for arbitrary markup rather than only for well-formed input.

#![no_main]

use libfuzzer_sys::fuzz_target;

/// Passes after the first that may still change the output. The worst input found settles in 3.
const MAX_PASSES: usize = 8;

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
    //    idempotence is too much: mis-nested markup (an anchor in an anchor, headings inside
    //    formatting elements) comes out nested one way, then as siblings, then settles, because
    //    each parse re-balances what the last one left open. Every pass must be inert, and the
    //    output must stop changing within a few passes.
    let mut current = clean;
    for _ in 0..MAX_PASSES {
        let next = bf_cook::sanitize(&current);
        bf_cook::inert::assert_inert(&next, html);
        if next == current {
            return;
        }
        current = next;
    }
    panic!("sanitize does not settle in {MAX_PASSES} passes
  input: {html:?}");
});
