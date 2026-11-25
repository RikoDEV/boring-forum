// Syntax colouring for the panel's code boxes (custom CSS, <head> HTML, theme CSS). No library:
// a small scanner cuts the text into classed pieces, and a Svelte action draws them in a layer
// right over the textarea, whose own text is made transparent. The textarea stays the real
// control, so typing, selection, undo and `bind:value` are the browser's own.

const JS_WORDS = new Set(
  (
    "async await break case catch class const continue debugger default delete do else export extends " +
    "finally for function if import in instanceof let new of return static super switch this throw try " +
    "typeof var void while with yield"
  ).split(" "),
);
const JS_CONSTS = new Set(["true", "false", "null", "undefined", "NaN", "Infinity"]);

/// Pieces of `text` as [class, string] pairs that join back to exactly `text`.
/// Classes: c comment, s string, k keyword or tag, a attribute or property, n number or constant.
export function tokens(text, lang) {
  const out = [];
  const put = (cls, s) => {
    if (s) out.push([cls, s]);
  };
  if (lang === "html") html(text, put);
  else if (lang === "css") css(text, put);
  else if (lang === "js") js(text, put);
  else if (lang === "robots") lines(text, put, true);
  else if (lang === "hosts") lines(text, put, false);
  else put("", text);
  return out;
}

const word = /[A-Za-z_$][\w$-]*/y;

function match(re, text, i) {
  re.lastIndex = i;
  const m = re.exec(text);
  return m ? m[0] : null;
}

function quoted(text, i) {
  const q = text[i];
  let j = i + 1;
  while (j < text.length && text[j] !== q && !(q !== "`" && text[j] === "\n")) j += text[j] === "\\" ? 2 : 1;
  return Math.min(j + 1, text.length);
}

// One entry per line, `#` starts a comment: robots.txt (`Directive: value`, the directive is a
// keyword and a URL value a string) and the lists of hosts (each line a string).
function lines(text, put, directives) {
  for (const line of text.split(/(?<=\n)/)) {
    const hash = line.indexOf("#");
    const body = hash < 0 ? line : line.slice(0, hash);
    const colon = directives ? body.indexOf(":") : -1;
    if (colon > 0 && /^[A-Za-z-]+$/.test(body.slice(0, colon).trim())) {
      const name = body.slice(0, colon).trimEnd();
      put("k", name);
      put("", body.slice(name.length, colon + 1));
      const value = body.slice(colon + 1);
      const lead = value.length - value.trimStart().length;
      const url = /^https?:\/\//.test(value.trim());
      put("", value.slice(0, lead));
      put(url ? "s" : "", value.slice(lead).trimEnd());
      put("", value.slice(value.trimEnd().length));
    } else if (!directives && body.trim()) {
      const lead = body.length - body.trimStart().length;
      put("", body.slice(0, lead));
      put("s", body.trim());
      put("", body.slice(body.trimEnd().length));
    } else put("", body);
    if (hash >= 0) put("c", line.slice(hash));
  }
}

function css(text, put) {
  let i = 0;
  let depth = 0;
  let plain = 0;
  const flush = (to) => {
    put("", text.slice(plain, to));
    plain = to;
  };
  const take = (cls, to) => {
    flush(i);
    put(cls, text.slice(i, to));
    i = plain = to;
  };
  while (i < text.length) {
    const c = text[i];
    if (c === "/" && text[i + 1] === "*") {
      const e = text.indexOf("*/", i + 2);
      take("c", e < 0 ? text.length : e + 2);
    } else if (c === '"' || c === "'") take("s", quoted(text, i));
    else if (c === "{") {
      depth++;
      i++;
    } else if (c === "}") {
      depth = Math.max(0, depth - 1);
      i++;
    } else if (c === "@") take("k", i + 1 + (match(word, text, i + 1) ?? "").length);
    else if (c === "#" && /[0-9a-fA-F]/.test(text[i + 1] ?? "") && depth > 0) {
      take("n", i + 1 + (match(/[0-9a-fA-F]+/y, text, i + 1) ?? "").length);
    } else if (/[0-9]/.test(c) || (c === "." && /[0-9]/.test(text[i + 1] ?? ""))) {
      take("n", i + (match(/(?:\d+\.?\d*|\.\d+)[A-Za-z%]*/y, text, i) ?? c).length);
    } else if (c === "!" && /^!\s*important/.test(text.slice(i, i + 20))) {
      take("k", i + (match(/!\s*important/y, text, i) ?? "!").length);
    } else if (/[A-Za-z_-]/.test(c)) {
      const w = match(/[\w-]+/y, text, i);
      // Inside a rule, a word before a colon is a property (or a custom property).
      if (
        depth > 0 &&
        /^\s*:/.test(text.slice(i + w.length, i + w.length + 40)) &&
        !/^\s*:\s*[\w-]*\s*\{/.test(text.slice(i + w.length, i + w.length + 80))
      ) {
        take("a", i + w.length);
      } else i += w.length;
    } else i++;
  }
  flush(text.length);
}

function js(text, put) {
  let i = 0;
  let plain = 0;
  const flush = (to) => {
    put("", text.slice(plain, to));
    plain = to;
  };
  const take = (cls, to) => {
    flush(i);
    put(cls, text.slice(i, to));
    i = plain = to;
  };
  while (i < text.length) {
    const c = text[i];
    if (c === "/" && text[i + 1] === "/") {
      const e = text.indexOf("\n", i);
      take("c", e < 0 ? text.length : e);
    } else if (c === "/" && text[i + 1] === "*") {
      const e = text.indexOf("*/", i + 2);
      take("c", e < 0 ? text.length : e + 2);
    } else if (c === '"' || c === "'" || c === "`") take("s", quoted(text, i));
    else if (/[0-9]/.test(c)) take("n", i + (match(/\d[\w.]*/y, text, i) ?? c).length);
    else if (/[A-Za-z_$]/.test(c)) {
      const w = match(/[\w$]+/y, text, i);
      if (JS_WORDS.has(w)) take("k", i + w.length);
      else if (JS_CONSTS.has(w)) take("n", i + w.length);
      else i += w.length;
    } else i++;
  }
  flush(text.length);
}

function html(text, put) {
  let i = 0;
  while (i < text.length) {
    const lt = text.indexOf("<", i);
    if (lt < 0) break;
    put("", text.slice(i, lt));
    i = lt;
    if (text.startsWith("<!--", i)) {
      const e = text.indexOf("-->", i + 4);
      const end = e < 0 ? text.length : e + 3;
      put("c", text.slice(i, end));
      i = end;
      continue;
    }
    const open = match(/<\/?[A-Za-z!][\w:-]*/y, text, i);
    if (!open) {
      put("", "<");
      i++;
      continue;
    }
    put("k", open);
    i += open.length;
    // Attributes up to the closing `>`.
    while (i < text.length && text[i] !== ">") {
      const c = text[i];
      if (c === '"' || c === "'") {
        const e = text.indexOf(c, i + 1);
        const end = e < 0 ? text.length : e + 1;
        put("s", text.slice(i, end));
        i = end;
      } else {
        const name = match(/[^\s=>/"']+/y, text, i);
        if (name) {
          put("a", name);
          i += name.length;
        } else {
          put("", c);
          i++;
        }
      }
    }
    if (i < text.length) {
      put("k", ">");
      i++;
    }
    // Raw text of script and style is another language, up to its end tag.
    const tag = open.slice(1).toLowerCase();
    if (tag === "script" || tag === "style") {
      const e = text.toLowerCase().indexOf("</" + tag, i);
      const end = e < 0 ? text.length : e;
      const body = text.slice(i, end);
      const lang = tag === "style" ? "css" : "js";
      for (const [cls, s] of tokens(body, lang)) put(cls, s);
      i = end;
    }
  }
  put("", text.slice(i));
}

/// Svelte action: `use:code={{ lang: "css", value }}` on a textarea. Pass the bound value too, so
/// the layer follows changes the page makes itself; typing is picked up from `input`.
export function code(textarea, param) {
  if (!param?.lang) return {};
  const pre = document.createElement("pre");
  pre.className = "acp-hl";
  pre.setAttribute("aria-hidden", "true");
  const body = document.createElement("code");
  pre.append(body);
  textarea.before(pre);
  textarea.parentElement?.classList.add("acp-hl-host");
  textarea.classList.add("acp-hl-on");
  let lang = param?.lang;

  // `text` is the bound value the page passes: the textarea may not hold it yet when this runs.
  const draw = (text = textarea.value) => {
    body.replaceChildren(
      ...tokens(text, lang).map(([cls, s]) => {
        const e = document.createElement("span");
        if (cls) e.className = "tk-" + cls;
        e.textContent = s;
        return e;
      }),
      // A final newline keeps the last line's height.
      document.createTextNode("\n"),
    );
    sync();
  };
  // The layer takes the textarea's box and type from its computed style (set through CSSOM, which
  // the page's policy allows), so it cannot drift from whatever rule styles the textarea.
  const place = () => {
    const cs = getComputedStyle(textarea);
    const s = pre.style;
    s.top = textarea.offsetTop + "px";
    s.left = textarea.offsetLeft + "px";
    s.width = textarea.offsetWidth + "px";
    s.height = textarea.offsetHeight + "px";
    // Longhands, not the `font` shorthand: Chrome answers it with "" for some fonts, and the layer
    // would fall back to another size and drift from the text it covers.
    for (const p of [
      "fontFamily",
      "fontSize",
      "fontWeight",
      "fontStyle",
      "fontStretch",
      "fontKerning",
      "fontVariantLigatures",
      "fontFeatureSettings",
      "lineHeight",
      "letterSpacing",
      "wordSpacing",
      "tabSize",
      "textIndent",
      "textTransform",
    ])
      s[p] = cs[p];
    s.padding = cs.padding;
    s.borderWidth = cs.borderWidth;
    sync();
  };
  const sync = () => {
    pre.scrollTop = textarea.scrollTop;
    pre.scrollLeft = textarea.scrollLeft;
  };
  const seen = new ResizeObserver(place);
  seen.observe(textarea);
  const typed = () => draw();
  textarea.addEventListener("input", typed);
  textarea.addEventListener("scroll", sync);
  place();
  draw(typeof param?.value === "string" ? param.value : undefined);
  return {
    update(next) {
      lang = next?.lang;
      draw(typeof next?.value === "string" ? next.value : undefined);
    },
    destroy() {
      seen.disconnect();
      textarea.removeEventListener("input", typed);
      textarea.removeEventListener("scroll", sync);
      textarea.classList.remove("acp-hl-on");
      pre.remove();
    },
  };
}
