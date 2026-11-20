//! XSS corpus: payloads from the OWASP XSS Filter Evasion cheat sheet, the
//! mutation-XSS literature and a few shapes specific to this pipeline, run through `cook` and
//! `sanitize` on every change.
//!
//! The assertion is deliberately crude and deliberately broad: whatever the input, the output
//! must contain no script element, no event-handler attribute, no `javascript:` URL and no
//! other executable sink. A sanitizer bug usually shows up as *something* on that list
//! surviving, so a blunt check over many inputs catches more than a precise check over few.
//!
//! When a payload here starts failing, the fix belongs in the cook pipeline or the sanitizer,
//! never in this file. Adding a case is welcome; weakening one needs the reason in the PR.

use bf_cook::{cook, sanitize};

/// Payloads that must never produce executable output. Markdown, HTML and BBCode mixed, since
/// all three reach the same sanitizer.
const PAYLOADS: &[&str] = &[
    // --- The classics, straight from the cheat sheet ------------------------------------
    r"<script>alert(1)</script>",
    r"<SCRIPT SRC=https://evil.example/x.js></SCRIPT>",
    r#"<IMG SRC="javascript:alert('XSS');">"#,
    r"<IMG SRC=javascript:alert('XSS')>",
    r"<IMG SRC=JaVaScRiPt:alert('XSS')>",
    r#"<IMG SRC=`javascript:alert("RSnake says, 'XSS'")`>"#,
    r#"<IMG """><SCRIPT>alert("XSS")</SCRIPT>">"#,
    r#"<IMG SRC=# onmouseover="alert('xxs')">"#,
    r#"<IMG SRC= onmouseover="alert('xxs')">"#,
    r#"<IMG onmouseover="alert('xxs')">"#,
    // Decimal, hex and mixed entity encodings of `javascript:alert('XSS')`.
    r"<IMG SRC=&#106;&#97;&#118;&#97;&#115;&#99;&#114;&#105;&#112;&#116;&#58;&#97;&#108;&#101;&#114;&#116;&#40;&#39;&#88;&#83;&#83;&#39;&#41;>",
    r"<IMG SRC=&#0000106&#0000097&#0000118&#0000097&#0000115&#0000099&#0000114&#0000105&#0000112&#0000116&#0000058&#0000097&#0000108&#0000101&#0000114&#0000116&#0000040&#0000039&#0000088&#0000083&#0000083&#0000039&#0000041>",
    r"<IMG SRC=&#x6A&#x61&#x76&#x61&#x73&#x63&#x72&#x69&#x70&#x74&#x3A&#x61&#x6C&#x65&#x72&#x74&#x28&#x27&#x58&#x53&#x53&#x27&#x29>",
    // Embedded tab, newline and carriage return inside the scheme.
    "<IMG SRC=\"jav\tascript:alert('XSS');\">",
    "<IMG SRC=\"jav&#x09;ascript:alert('XSS');\">",
    "<IMG SRC=\"jav&#x0A;ascript:alert('XSS');\">",
    "<IMG SRC=\"jav&#x0D;ascript:alert('XSS');\">",
    // Null byte and spaces/meta refresh tricks.
    "<SCR\0IPT>alert(\"XSS\")</SCR\0IPT>",
    r#"<IMG SRC=" &#14;  javascript:alert('XSS');">"#,
    r#"<META HTTP-EQUIV="refresh" CONTENT="0;url=javascript:alert('XSS');">"#,
    r#"<META HTTP-EQUIV="refresh" CONTENT="0;url=data:text/html;base64,PHNjcmlwdD5hbGVydCgnWFNTJyk8L3NjcmlwdD4K">"#,
    // Other elements that execute or navigate.
    r"<BODY ONLOAD=alert('XSS')>",
    r#"<BODY BACKGROUND="javascript:alert('XSS')">"#,
    r"<svg/onload=alert(1)>",
    r"<svg><script>alert(1)</script></svg>",
    r"<svg><animate onbegin=alert(1) attributeName=x dur=1s>",
    r#"<math><mtext><a xlink:href="javascript:alert(1)">x</a></mtext></math>"#,
    r#"<iframe src="javascript:alert('XSS');"></iframe>"#,
    r#"<iframe srcdoc="&lt;script&gt;alert(1)&lt;/script&gt;"></iframe>"#,
    r#"<object data="javascript:alert(1)">"#,
    r#"<embed src="data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIG9ubG9hZD0iYWxlcnQoMSkiLz4=">"#,
    r#"<form action="javascript:alert(1)"><button>x</button></form>"#,
    r#"<button formaction="javascript:alert(1)">x</button>"#,
    r#"<input type="image" src="javascript:alert(1)">"#,
    r#"<a href="javascript:alert(1)">click</a>"#,
    r#"<a href="JaVaScRiPt&colon;alert(1)">click</a>"#,
    r#"<a href="vbscript:msgbox(1)">click</a>"#,
    r#"<a href="data:text/html,<script>alert(1)</script>">click</a>"#,
    r#"<base href="https://evil.example/">"#,
    r#"<link rel=stylesheet href="javascript:alert(1)">"#,
    r"<style>@import 'javascript:alert(1)';</style>",
    r#"<style>body{background:url("javascript:alert(1)")}</style>"#,
    r#"<div style="background-image: url(javascript:alert(1))">x</div>"#,
    r#"<div style="width: expression(alert(1))">x</div>"#,
    r#"<table background="javascript:alert(1)"><tr><td>x</td></tr></table>"#,
    r#"<isindex action="javascript:alert(1)" type=image>"#,
    r"<keygen autofocus onfocus=alert(1)>",
    r#"<video><source onerror="alert(1)"></video>"#,
    r"<audio src=x onerror=alert(1)>",
    r"<details open ontoggle=alert(1)>",
    r"<marquee onstart=alert(1)>x</marquee>",
    r"<xss id=x tabindex=1 onactivate=alert(1)></xss>",
    // --- Mutation XSS: safe-looking markup that a browser re-parses into something else ---
    r#"<noscript><p title="</noscript><img src=x onerror=alert(1)>">"#,
    r#"<svg></p><style><a id="</style><img src=1 onerror=alert(1)>">"#,
    r"<table><math><mtext><table><mglyph><style><math><mtext></table><img src=x onerror=alert(1)>",
    r"<form><math><mtext></form><form><mglyph><style></math><img src onerror=alert(1)>",
    r"<style><style/><img src=x onerror=alert(1)>",
    r"<--`<img/src=` onerror=alert(1)> --!>",
    r"<textarea></textarea><img src=x onerror=alert(1)>",
    r"<title></title><img src=x onerror=alert(1)>",
    // --- Shapes specific to this pipeline ----------------------------------------------
    // Markdown link and image syntax with a script URL.
    r"[click](javascript:alert(1))",
    r"[click](JAVASCRIPT:alert(1))",
    r"![img](javascript:alert(1))",
    r"[click](<javascript:alert(1)>)",
    r"[click](java&#115;cript:alert(1))",
    r"[ref][1]

[1]: javascript:alert(1)",
    // Reference-style image, autolink, and raw HTML inside Markdown emphasis.
    r"<javascript:alert(1)>",
    r"*<img src=x onerror=alert(1)>*",
    // Code fences and inline code must not become a sink either.
    "```html\n<script>alert(1)</script>\n```",
    "`<img src=x onerror=alert(1)>`",
    // A fence whose language is itself a payload, aimed at the highlighter's class attribute.
    "```\"><script>alert(1)</script>\nx\n```",
    "```<img src=x onerror=alert(1)>\nx\n```",
    // BBCode, which is converted to Markdown before cooking.
    r"[url=javascript:alert(1)]click[/url]",
    r"[img]javascript:alert(1)[/img]",
    r#"[url="javascript:alert(1)"]click[/url]"#,
    r#"[color=red" onmouseover="alert(1)]x[/color]"#,
    r"[code]<script>alert(1)</script>[/code]",
    // Unicode line separators, which can break out of a JS string context.
    "a\u{2028}<img src=x onerror=alert(1)>",
    "a\u{2029}<img src=x onerror=alert(1)>",
    // Over-long and nested input, in case a depth or size limit changes the parse.
    r"<div><div><div><div><div><img src=x onerror=alert(1)></div></div></div></div></div>",
];

/// Checks one piece of output, using the library's own definition of inert so the test and the
/// fuzz targets in `fuzz/` cannot drift apart.
fn assert_inert(html: &str, payload: &str, stage: &str) {
    let violations = bf_cook::inert::violations(html);
    assert!(
        violations.is_empty(),
        "{stage}: {}
  payload: {payload:?}
  output:  {html:?}",
        violations.join(", ")
    );
}

#[test]
fn cook_never_emits_executable_html() {
    for payload in PAYLOADS {
        assert_inert(&cook(payload).html, payload, "cook");
    }
}

#[test]
fn sanitize_never_emits_executable_html() {
    for payload in PAYLOADS {
        assert_inert(&sanitize(payload), payload, "sanitize");
    }
}

#[test]
fn sanitize_is_idempotent() {
    // Sanitizing twice must equal sanitizing once. Where it does not, some output of the
    // sanitizer is being re-parsed differently than it was written -- the mutation-XSS shape.
    for payload in PAYLOADS {
        let once = sanitize(payload);
        assert_eq!(sanitize(&once), once, "sanitize is not idempotent for {payload:?}");
    }
}

#[test]
fn cooked_output_survives_a_second_sanitize_unchanged() {
    // The cook pipeline's output is stored and later served; anything that a re-sanitize would
    // strip means the stored HTML contains something the sanitizer does not consider safe.
    for payload in PAYLOADS {
        let cooked = cook(payload).html;
        assert_inert(&sanitize(&cooked), payload, "cook then sanitize");
    }
}

#[test]
fn links_extracted_from_payloads_are_never_script_urls() {
    // `links` feeds link limits, duplicate-link warnings and onebox lookups, so a script URL
    // reaching that list is an injection into those features even if the HTML is clean.
    for payload in PAYLOADS {
        for link in cook(payload).links {
            let lower = link.to_ascii_lowercase();
            assert!(
                !lower.starts_with("javascript:") && !lower.starts_with("vbscript:") && !lower.starts_with("data:"),
                "cook extracted an executable link {link:?} from {payload:?}"
            );
        }
    }
}
