// The panel's messages, in the admin's own language (catalog keys under `admin.` in
// crates/bf-core/locales): fetched once with the site's header (/api/v1/admin/shell) before the
// first page renders, so `t()` can be called anywhere, also outside components.
import KEYS from "./keys.json";

let shellPromise = null;
let messages = {};
let lang = "en";

/** The shell response (site header, styles, messages); fetched once. */
export function loadShell() {
  shellPromise ??= fetch("/api/v1/admin/shell")
    .then((r) => (r.ok ? r.json() : null))
    .catch(() => null)
    .then((s) => {
      messages = s?.admin_messages ?? {};
      lang = s?.locale ?? "en";
      document.documentElement.lang = lang;
      document.documentElement.dir = s?.dir ?? "ltr";
      return s;
    });
  return shellPromise;
}

// Compiled Fluent messages (the server's JSON): a string, or an array of parts: a string,
// {v: name}, {v: name, num: true} (a number in the locale's format) or
// {sel: name, cases: {key: message}, def: key}.
const RULES = new Map();
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

// The catalog key of English text (`admin.` plus its name).
/** @param {string} text */
const textKey = (text) => `admin.${KEYS.texts.admin?.[text] ?? slug(text)}`;

// English source text with `{name}` placeholders, as a compiled message (the fallback when the catalog has no entry).
const source = (text) => text.split(/{(w+)}/).map((s, i) => (i % 2 ? { v: s } : s));

/** Text written in English, in the admin's language: `tr(english, { name })`.
 *  scripts/i18n.py collects the literal first argument of every call into the catalogs, so the argument must be a literal. */
export function tr(english, vars = {}) {
  return fmt(messages[textKey(english)] ?? source(english), vars, lang);
}

/** A message by key: `t("admin.settings.title")`, with `{name}` placeholders from `vars`. A missing key
 *  shows as itself, so a gap is visible but never breaks the page. */
export function t(key, vars = {}) {
  return fmt(messages[key] ?? key, vars, lang);
}

/** A counted message: a plural or selector message under one key, with `n` in `vars`. */
export function tn(key, n, vars = {}) {
  return t(key, { n, ...vars });
}

/** The admin's language, for Intl formatting. */
export const locale = () => lang;
