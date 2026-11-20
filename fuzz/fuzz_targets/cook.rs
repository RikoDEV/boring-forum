//! Fuzzes the whole cook pipeline, the component that turns the least trusted input in
//! the product -- a post body -- into HTML that is served to everyone.
//!
//! Checks more than "it does not crash". The pipeline's security property is that its output is
//! inert whatever goes in, so this target re-asserts the XSS invariants on every generated
//! input. A crash and an escaped payload are both failures.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(raw) = std::str::from_utf8(data) else { return };
    // Keep inputs to a realistic post size; the limit itself is enforced before cooking.
    if raw.len() > 64 * 1024 {
        return;
    }

    let cooked = bf_cook::cook(raw);

    // 1. The output must contain no executable construct.
    bf_cook::inert::assert_inert(&cooked.html, raw);

    // 2. Sanitizing the output again must not change it. A difference means the stored HTML is
    //    re-parsed differently than it was written, which is the mutation-XSS shape.
    let resanitized = bf_cook::sanitize(&cooked.html);
    assert_eq!(resanitized, cooked.html, "cook output is not stable under sanitize\n  input: {raw:?}");

    // 3. Extracted links feed link limits and onebox lookups, so none may be executable.
    for link in &cooked.links {
        let lower = link.to_ascii_lowercase();
        assert!(
            !lower.starts_with("javascript:") && !lower.starts_with("vbscript:") && !lower.starts_with("data:"),
            "cook extracted an executable link {link:?} from {raw:?}"
        );
    }

    // 4. Mentions become usernames to notify, so they must look like usernames rather than
    //    carrying markup into whatever renders them.
    for mention in &cooked.mentions {
        assert!(
            !mention.contains('<') && !mention.contains('>') && !mention.contains('"'),
            "cook produced a mention containing markup: {mention:?} from {raw:?}"
        );
        assert_eq!(*mention, mention.to_lowercase(), "mentions are expected lowercased: {mention:?}");
    }

    // 5. Cooking is deterministic: the same post must not render differently on two views.
    assert_eq!(bf_cook::cook(raw).html, cooked.html, "cook is not deterministic for {raw:?}");
});
