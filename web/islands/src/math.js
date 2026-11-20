// Math typesetting (`math_enabled`): the cooker marks TeX with data-math-style; KaTeX turns it
// into MathML, which browsers lay out natively, so no fonts or stylesheet are needed.
import katex from "katex";

export function typeset(root) {
  for (const e of root.querySelectorAll(".cooked [data-math-style]")) {
    if (e.dataset.typeset) continue;
    // A ```math fence puts the marker on <code> inside a <pre>; the whole block is replaced.
    const target = e.tagName === "CODE" && e.parentElement?.tagName === "PRE" ? e.parentElement : e;
    const out = document.createElement(e.dataset.mathStyle === "display" ? "div" : "span");
    out.className = "math";
    out.dataset.typeset = "1";
    try {
      katex.render(e.textContent, out, {
        displayMode: e.dataset.mathStyle === "display",
        output: "mathml",
        throwOnError: false,
        // No \href, \url or \includegraphics: posts must not gain links or images this way.
        trust: false,
        maxSize: 50,
        maxExpand: 500,
      });
    } catch {
      continue;
    }
    target.replaceWith(out);
  }
}
