//! Mis-nested markup that the sanitizer needs three passes to settle on (found by the `sanitize`
//! fuzz target). Every pass must be inert and the output must stop changing within a few passes.

use bf_cook::{inert::assert_inert, sanitize};

#[test]
fn mis_nested_headings_and_anchors_settle_and_stay_inert() {
    let html = std::str::from_utf8(include_bytes!("fixtures/sanitize_settle_crash.html")).expect("utf-8 fixture");
    let mut current = sanitize(html);
    assert_inert(&current, html);
    for _ in 0..8 {
        let next = sanitize(&current);
        assert_inert(&next, html);
        if next == current {
            return;
        }
        current = next;
    }
    panic!("the output kept changing for 8 passes");
}
