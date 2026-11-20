//! Fuzzes the search query parser.
//!
//! The parser turns `@user #forum tags:a,b before:2026-01-01 likes:3` into the struct that
//! builds a SQL query. Two things must hold for every input: it must not panic (the query string
//! comes straight from a URL, so a panic is a denial of service anyone can trigger), and the
//! fields it fills must stay within what the query builder can bind safely.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(q) = std::str::from_utf8(data) else { return };
    // Search boxes are short; the route caps the length before parsing.
    if q.len() > 4096 {
        return;
    }

    let parsed = bf_core::search::parse(q);

    // Dates reach a date comparison, so anything that is not a plain `YYYY-MM-DD` must have
    // been rejected by the parser rather than passed along to be interpreted later.
    for date in [&parsed.before, &parsed.after].into_iter().flatten() {
        assert!(
            date.len() == 10 && date.as_bytes().iter().enumerate().all(|(i, b)| match i {
                4 | 7 => *b == b'-',
                _ => b.is_ascii_digit(),
            }),
            "parse accepted a date it cannot have validated: {date:?} from {q:?}"
        );
    }

    // A tag is matched against the tag table; an empty one would match everything.
    for tag in &parsed.tags {
        assert!(!tag.is_empty(), "parse produced an empty tag from {q:?}");
    }

    // Parsing is deterministic and total: the same query always gives the same filters.
    assert_eq!(bf_core::search::parse(q), parsed, "search parse is not deterministic for {q:?}");
});
