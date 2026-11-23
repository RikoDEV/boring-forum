// Small DOM helpers shared by the guest islands.

import KEYS from "./keys.json" with { type: "json" };

/**
 * @typedef {string | {v?: string, num?: boolean, sel?: string, cases?: Record<string, Message>, def?: string}} MessagePart
 * @typedef {string | MessagePart[]} Message  a compiled Fluent message from the server's JSON
 * @typedef {Record<string, unknown>} Vars  values for `{name}` placeholders
 */

/**
 * Builds an element with attributes (all set as strings) and optional text.
 * @template {keyof HTMLElementTagNameMap} K
 * @param {K} tag
 * @param {Record<string, string | number | boolean>} [attrs]
 * @param {string} [text]
 * @returns {HTMLElementTagNameMap[K]}
 */
export const el = (tag, attrs = {}, text) => {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, String(v));
  if (text !== undefined) e.textContent = text;
  return e;
};

/** Builds an element from DOM properties (`className`, `textContent`, `type`, …) and children. */
export const mk = (tag, props = {}, ...kids) => {
  const e = Object.assign(document.createElement(tag), props);
  e.append(...kids);
  return e;
};

/** `mk` for a class and optional text. */
export const mkc = (tag, cls, text) => mk(tag, cls ? { className: cls } : {}, ...(text === undefined ? [] : [text]));

/** Gives an element the site's hover bubble (`data-tip`, see tooltip.js) and returns it. @template {Element} T @param {T} e @param {string} text @returns {T} */
export const tipped = (e, text) => {
  if (text) e.setAttribute("data-tip", text);
  return e;
};

/** The page's CSRF token, from the layout's meta tag. */
export const csrf = () => document.querySelector('meta[name="csrf-token"]')?.content ?? "";

/** Headers for a JSON request that changes data. @param {Record<string, string>} [extra] */
export const jsonHeaders = (extra) => ({ "Content-Type": "application/json", "X-CSRF-Token": csrf(), ...extra });

/** A form's fields as a urlencoded body (FormData is accepted by URLSearchParams; the DOM typings say no). @param {HTMLFormElement} form */
export const formParams = (form) => new URLSearchParams(/** @type {any} */ (new FormData(form)));

// The page's messages (catalog keys under `js.`, in the viewer's language), from the JSON the
// layout puts in #bf-i18n. A missing key shows as itself, so a gap is visible but harmless.
/** @type {Record<string, Message> | null} */
let MESSAGES = null;
const messages = () => {
  if (!MESSAGES) {
    try {
      MESSAGES = JSON.parse(document.getElementById("bf-i18n")?.textContent || "{}");
    } catch {
      MESSAGES = {};
    }
  }
  return MESSAGES;
};
// Compiled Fluent messages (the server's JSON): a string, or an array of parts: a string,
// {v: name}, {v: name, num: true} (a number in the locale's format) or
// {sel: name, cases: {key: message}, def: key}.
const RULES = new Map();
/** @param {string | undefined} locale */
const rules = (locale) => {
  let r = RULES.get(locale);
  if (!r) {
    try {
      r = new Intl.PluralRules(locale || undefined);
    } catch {
      r = new Intl.PluralRules();
    }
    RULES.set(locale, r);
  }
  return r;
};
/**
 * @param {Message | undefined} message
 * @param {Vars} vars
 * @param {string} [locale]
 * @returns {string}
 */
export function fmt(message, vars, locale) {
  if (typeof message === "string") return message;
  if (!Array.isArray(message)) return "";
  let out = "";
  for (const part of message) {
    if (typeof part === "string") out += part;
    else if (part.sel !== undefined) {
      const v = vars[part.sel];
      let key = part.def;
      if (typeof v === "number") {
        const exact = Object.keys(part.cases).find((k) => k.trim() !== "" && Number(k) === v);
        key = exact ?? (rules(locale).select(v) in part.cases ? rules(locale).select(v) : part.def);
      } else if (v !== undefined && v !== null && String(v) in part.cases) key = String(v);
      out += fmt(part.cases[key] ?? "", vars, locale);
    } else if (part.v !== undefined) {
      const v = vars[part.v];
      if (v === undefined || v === null) out += `{${part.v}}`;
      else if (part.num && typeof v === "number") out += new Intl.NumberFormat(locale || undefined).format(v);
      else out += String(v);
    }
  }
  return out;
}

/**
 * True while a key press belongs to an IME composition (Japanese, Chinese, Korean...): Enter then
 * picks a candidate and must not run the field's own command. Safari ends the composition before
 * the keydown arrives, but still reports keyCode 229.
 * @param {KeyboardEvent} e
 */
export const composing = (e) => e.isComposing || e.keyCode === 229;

/** The page's language (<html lang>), or undefined for the browser's own. */
export const LOCALE = () => document.documentElement.lang || undefined;

// A text's readable name: its words in lower case joined by underscores (`First like` is
// `first_like`), as scripts/i18n.py and bf_core::i18n compute it. keys.json is written by
// `python scripts/i18n.py sync`: the plain spelling of accented letters, and the explicit names of
// texts that read the same once case and punctuation are gone.
const NAME_MAX = 56;
/** @param {string} text */
const fnv8 = (text) => {
  let h = 0x811c9dc5;
  for (const b of new TextEncoder().encode(text)) h = Math.imul(h ^ b, 0x01000193) >>> 0;
  return h.toString(16).padStart(8, "0");
};
/** @param {string} text */
const slug = (text) => {
  /** @type {string[]} */
  const words = [];
  let cur = "";
  for (const ch of text) {
    for (const c of KEYS.translit[ch] ?? ch) {
      if (/^[A-Za-z0-9]$/.test(c)) cur += c.toLowerCase();
      else if (cur) {
        words.push(cur);
        cur = "";
      }
    }
  }
  if (cur) words.push(cur);
  const name = words.join("_");
  if (!name) return `text_${fnv8(text)}`;
  if (name.length > NAME_MAX) {
    const cut = name.slice(0, NAME_MAX);
    const i = cut.lastIndexOf("_");
    return i >= 0 ? cut.slice(0, i) : cut;
  }
  return name;
};

// The catalog key of English text in `prefix` (`js` here).
/**
 * @param {string} prefix
 * @param {string} text
 */
const textKey = (prefix, text) => `${prefix}.${KEYS.texts[prefix]?.[text] ?? slug(text)}`;

// English source text with `{name}` placeholders, as a compiled message (the fallback when the catalog has no entry).
/** @param {string} text */
const source = (text) => text.split(/{(\w+)}/).map((s, i) => (i % 2 ? { v: s } : s));

/** Text written in English, in the page's language: `tr("Following {name}", { name })`.
 *  scripts/i18n.py collects the literal first argument of every call into the catalogs, so the argument must be a literal. */
/** @param {string} english @param {Vars} [vars] */
export function tr(english, vars = {}) {
  return fmt(messages()[textKey("js", english)] ?? source(english), vars, LOCALE());
}

/** A counted phrase split around its number, for a figure styled apart from its words:
 *  `countParts("count.posts", 3)` gives `["", " posts"]`. */
/** @param {string} key @param {number} n @returns {[string, string]} */
export function countParts(key, n) {
  const text = tn(key, n);
  for (const num of [new Intl.NumberFormat(LOCALE()).format(n), String(n)]) {
    const i = text.indexOf(num);
    if (i >= 0) return [text.slice(0, i), text.slice(i + num.length)];
  }
  return [text, ""];
}

/** A message by its catalog key: `t("js.card.joined")`, with `{name}` placeholders from `vars`. */
/** @param {string} key @param {Vars} [vars] */
function t(key, vars = {}) {
  return fmt(messages()[key] ?? key, vars, LOCALE());
}

/** A counted message: a plural or selector message under one key, with `n` (and `{n}` formatted) in `vars`. */
/** @param {string} key @param {number} n @param {Vars} [vars] */
export function tn(key, n, vars = {}) {
  return t(key, { n, ...vars });
}

// Dates in the page's language (<html lang>), in the reader's own time zone.
const LANG = document.documentElement.lang || undefined;
const FULL = new Intl.DateTimeFormat(LANG, { dateStyle: "full", timeStyle: "short" });
const RELATIVE = new Intl.RelativeTimeFormat(LANG, { numeric: "auto", style: "short" });
const FORMATS = {
  month: new Intl.DateTimeFormat(LANG, { month: "short", year: "numeric" }),
  "month-long": new Intl.DateTimeFormat(LANG, { month: "long", year: "numeric" }),
  day: new Intl.DateTimeFormat(LANG, { dateStyle: "medium" }),
  stamp: new Intl.DateTimeFormat(LANG, { dateStyle: "medium", timeStyle: "short" }),
  time: new Intl.DateTimeFormat(LANG, { timeStyle: "short" }),
};
/** @param {string | null | undefined} iso */
const parse = (iso) => {
  const d = new Date(iso);
  return iso && !Number.isNaN(+d) ? d : null;
};
/** The full date and time of a moment, in the reader's language; "" for none. @param {string | null | undefined} iso */
export const fullDate = (iso) => {
  const d = parse(iso);
  return d ? FULL.format(d) : "";
};

// The words for a moment, like the server's `Ago`: relative ("3 hr. ago") within 30 days, then
// the date; or one of the fixed formats (`data-format`: month, month-long, day, stamp).
/** @param {string | null | undefined} iso @param {string} [format] */
export function timeText(iso, format = "") {
  const d = parse(iso);
  if (!d) return "";
  if (FORMATS[format]) return FORMATS[format].format(d);
  const s = Math.max(0, (Date.now() - d.getTime()) / 1000);
  if (s < 60) return RELATIVE.format(0, "second");
  if (s < 3600) return RELATIVE.format(-Math.floor(s / 60), "minute");
  if (s < 86400) return RELATIVE.format(-Math.floor(s / 3600), "hour");
  if (s < 2_592_000) return RELATIVE.format(-Math.floor(s / 86400), "day");
  return FORMATS.day.format(d);
}

// Every date the islands show: the words in the reader's language and the full date on hover.
// `text` stands when there is no moment (a fixed word such as "never").
/** @param {string | null | undefined} text @param {string | null | undefined} iso @param {Record<string, string>} [attrs] @returns {Node} */
export function when(text, iso, attrs = {}) {
  const full = fullDate(iso);
  if (!full) return document.createTextNode(text ?? "");
  return el("time", { ...attrs, datetime: iso, "data-tip": full }, timeText(iso, attrs["data-format"]));
}

// Every member's name the islands show, like the server's `layout.user_link`: a link to the
// profile (which also opens their card on hover). "deleted" and empty stay plain text.
/** @param {string | null | undefined} name @param {Record<string, string>} [attrs] @returns {Node} */
export function userLink(name, attrs = {}) {
  if (!name || name === "deleted") return document.createTextNode(name || "deleted");
  return el("a", { class: "bf-user-link", ...attrs, href: `/u/${encodeURIComponent(name.toLowerCase())}` }, name);
}

const SVG = "http://www.w3.org/2000/svg";
/** An inline sprite icon. @param {string} name @returns {SVGSVGElement} */
export function icon(name) {
  const sprite = document.querySelector(".bf-icon use")?.getAttribute("href")?.split("#")[0];
  const svg = document.createElementNS(SVG, "svg");
  svg.setAttribute("class", "bf-icon");
  svg.setAttribute("aria-hidden", "true");
  const use = document.createElementNS(SVG, "use");
  use.setAttribute("href", `${sprite}#${name}`);
  svg.append(use);
  return svg;
}

// A short message over the page. It is read out politely; an error (`urgent`) interrupts.
/** @param {string} text @param {boolean} [urgent] */
export function toast(text, urgent = false) {
  let box = document.querySelector(".bf-toasts");
  if (!box) {
    box = el("div", { class: "bf-toasts", role: "status", "aria-live": "polite" });
    document.body.append(box);
  }
  const item = el("div", urgent ? { class: "bf-toast", role: "alert" } : { class: "bf-toast" }, text);
  box.append(item);
  setTimeout(() => item.remove(), 6000);
}

// Reads a message out through the page's live region (layout.html), so screen-reader
// users hear what sighted members see change. Polite waits for a pause; urgent interrupts.
/** @param {string} text @param {boolean} [urgent] */
export function announce(text, urgent = false) {
  const region = document.getElementById(urgent ? "bf-alert" : "bf-live");
  if (!region) return;
  // Clearing first makes a repeated message count as a change.
  region.textContent = "";
  setTimeout(() => (region.textContent = text), 50);
}
