//! Fuzzes `[hide]` handling.
//!
//! A `[hide]` block is content a member is not allowed to see yet. The stored HTML must not
//! contain it at all, because feeds, excerpts, search results, API responses and plugins all
//! read that HTML and none of them know about the permission. So the property under test is a
//! privacy one rather than an XSS one: whatever the input, the hidden text must not survive into
//! the public HTML.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(raw) = std::str::from_utf8(data) else { return };
    if raw.len() > 64 * 1024 {
        return;
    }

    let (public_source, hidden) = bf_cook::split_hidden(raw);
    let public_html = bf_cook::cook(&public_source).html;

    // Whatever was hidden must be absent from what everyone can read. Short fragments are
    // skipped: a one-character hidden block legitimately shares characters with normal text.
    // So are blocks whose text the public source also holds outside any block (a post may repeat
    // a sentence both inside and outside a `[hide]`): that copy is public by the writer's choice.
    for block in &hidden {
        let trimmed = block.trim();
        if trimmed.len() >= 12 && !public_source.contains(trimmed) {
            assert!(
                !public_html.contains(trimmed),
                "a hidden block leaked into public HTML\n  hidden: {trimmed:?}\n  input:  {raw:?}"
            );
        }
    }

    // `strip_hidden` must agree with `split_hidden` about what is public. The database has its
    // own copy of this logic (`bf_strip_hidden`), and the two drifting apart is how a leak would
    // appear in search results while the topic page looks correct.
    assert_eq!(bf_cook::strip_hidden(raw), public_source, "strip_hidden disagrees with split_hidden for {raw:?}");

    // Revealing for a member who may see it must still be inert.
    bf_cook::inert::assert_inert(&bf_cook::reveal_hidden(&public_html, raw), raw);
});
