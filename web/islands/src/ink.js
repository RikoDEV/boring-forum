// Readable text in a chosen colour (group labels): the colour itself where it contrasts enough
// with what is really behind the text, otherwise the same hue lightened or darkened until it
// does (WCAG AA, 4.5:1). Shared by the site's islands and the admin panel's preview.

const AA = 4.5;

/** "#rrggbb" or "rgb(a)(…)" → [r, g, b, a] with r, g, b 0–255 and a 0–1; null if unreadable. */
function parseColour(c) {
  if (!c) return null;
  const hex = /^#([0-9a-f]{6})$/i.exec(c.trim());
  if (hex) {
    const n = parseInt(hex[1], 16);
    return [(n >> 16) & 255, (n >> 8) & 255, n & 255, 1];
  }
  // color-mix() results come back as `color(srgb r g b / a)`, channels 0–1.
  const srgb = /^color\(srgb\s+([^)]+)\)$/.exec(c.trim());
  if (srgb) {
    const p = srgb[1]
      .split(/[\s/]+/)
      .filter(Boolean)
      .map(Number);
    return p.length >= 3 && p.every((x) => !Number.isNaN(x))
      ? [p[0] * 255, p[1] * 255, p[2] * 255, p.length > 3 ? p[3] : 1]
      : null;
  }
  const m = /^rgba?\(([^)]+)\)$/.exec(c.trim());
  if (!m) return null;
  const p = m[1]
    .split(/[\s,/]+/)
    .filter(Boolean)
    .map(Number);
  return p.length >= 3 && p.every((x) => !Number.isNaN(x)) ? [p[0], p[1], p[2], p.length > 3 ? p[3] : 1] : null;
}

const channel = (v) => {
  const s = v / 255;
  return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
};
const luminance = ([r, g, b]) => 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);

/** WCAG contrast ratio of two opaque colours. */
function contrast(a, b) {
  const [x, y] = [luminance(a), luminance(b)].sort((p, q) => q - p);
  return (x + 0.05) / (y + 0.05);
}

const over = (top, bottom) =>
  top
    .slice(0, 3)
    .map((v, i) => v * top[3] + bottom[i] * (1 - top[3]))
    .concat(1);

/** What is painted behind `el`: its own background and its ancestors', blended, opaque. */
export function backgroundOf(el) {
  const layers = [];
  for (let e = el; e; e = e.parentElement) {
    const c = parseColour(getComputedStyle(e).backgroundColor);
    if (c && c[3] > 0) {
      layers.push(c);
      if (c[3] >= 1) break;
    }
  }
  const dark = matchMedia("(prefers-color-scheme: dark)").matches;
  let bg = dark ? [0, 0, 0, 1] : [255, 255, 255, 1];
  for (const layer of layers.reverse()) bg = over(layer, bg);
  return bg;
}

/** `colour` (#rrggbb), moved towards white on a dark background or black on a light one until
 * it reaches 4.5:1 against `bg`. Returns "#rrggbb". */
export function readable(colour, bg) {
  const c = parseColour(colour);
  if (!c) return colour;
  const toward = luminance(bg) < 0.18 ? [255, 255, 255] : [0, 0, 0];
  let out = c;
  for (let t = 0; t <= 1 && contrast(out, bg) < AA; t += 0.05) {
    out = c.slice(0, 3).map((v, i) => Math.round(v + (toward[i] - v) * t));
  }
  return `#${out
    .slice(0, 3)
    .map((v) => v.toString(16).padStart(2, "0"))
    .join("")}`;
}

/** Sets `--bf-group-colour` (the group's own colour, for tints) and `--bf-group-ink` (readable
 * text) on every `[data-group-colour]` under `root`. Run again after the theme changes. */
export function groupColours(root = document) {
  for (const el of root.querySelectorAll("[data-group-colour]")) {
    el.style.setProperty("--bf-group-colour", el.dataset.groupColour);
    el.style.removeProperty("--bf-group-ink"); // measure the background without our own text tint
    el.style.setProperty("--bf-group-ink", readable(el.dataset.groupColour, backgroundOf(el)));
  }
}
