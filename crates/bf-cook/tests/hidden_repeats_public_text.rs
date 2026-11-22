//! A post may repeat, outside a `[hide]` block, the very text it hides. That copy is public by the
//! writer's choice, so it must stay in the public HTML; only the block itself is removed. The
//! `hidden` fuzz target found this input and called the public copy a leak.

const CRASH: &str = "```hid```hide\u{4}d_`hispande\u{4}d_``WZ```hide\u{4}d_`hispande\u{4}d_`````e\u{4}d_`hispande\u{4}d_``WZ```Eide\u{4}d_`hispande\u{4}d_`````````````";

#[test]
fn a_public_repeat_of_hidden_text_is_not_a_leak() {
    let (public_source, hidden) = bf_cook::split_hidden(CRASH);
    assert!(!hidden.is_empty());
    assert!(public_source.contains(hidden[0].trim()), "the repeat is in the public source");
    for block in &hidden {
        let trimmed = block.trim();
        if trimmed.len() >= 12 && !public_source.contains(trimmed) {
            assert!(!bf_cook::cook(&public_source).html.contains(trimmed));
        }
    }
    assert_eq!(bf_cook::strip_hidden(CRASH), public_source);
}
