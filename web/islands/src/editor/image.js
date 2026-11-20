// Pictures in the editor: how one is written in Markdown, and the small floating panel that edits
// one (its description, its caption and its size). The widget that shows a picture lives in main.js.
import { mk, mkc, tr } from "../dom.js";

/** `![description](address "caption")`: the description, the address (with its `#WxH` size) and the caption. */
export const IMAGE = /!\[([^\]\n]*)\]\(([^)\s]+)(?:\s+"([^"\n]*)")?\)/g;

// Only an address that can be a picture shows as one: a file with another extension (a zip, a
// PDF) stays a link, even if it was written as `![name](file.zip)`.
const PICTURE = /\.(png|jpe?g|gif|webp|avif|svg|bmp|ico)$/i;
export const isPicture = (url) => {
  const path = url.split(/[#?]/)[0];
  return PICTURE.test(path) || !/\.[a-z0-9]{1,8}$/i.test(path);
};

/** What a match of `IMAGE` says. */
export const readImage = (m) => ({ alt: m[1], url: m[2], caption: (m[3] ?? "").replace(/&quot;/g, '"') });

/** An image written out again. */
export const imageMarkdown = ({ alt, url, caption }) => {
  const words = alt.replace(/[[\]\n]/g, " ").trim();
  const title = caption.trim().replace(/\n/g, " ").replace(/"/g, "&quot;");
  return `![${words}](${url}${title ? ` "${title}"` : ""})`;
};

/** The size an address carries (`…#640x480`), or null. */
export const sizeIn = (url) => {
  const m = /#(\d+)x(\d+)$/.exec(url);
  return m ? [Number(m[1]), Number(m[2])] : null;
};

/**
 * The text of each picture on screen, by its element: `() => { from, to, alt, url, caption } | null`.
 * A right click finds the picture under it here, and the panel re-reads it before it writes.
 * @type {WeakMap<Element, () => ({ from: number, to: number, alt: string, url: string, caption: string } | null)>}
 */
export const finders = new WeakMap();

/** Takes a picture out of the text; one that is alone on its line takes the line with it. */
export function removeImage(view, from, to) {
  const doc = view.state.doc;
  const line = doc.lineAt(from);
  if (line.text.trim() !== doc.sliceString(from, to)) {
    view.dispatch({ changes: { from, to } });
    return;
  }
  // The last line has no newline after it, so it gives up the one before.
  const atEnd = line.to >= doc.length && line.from > 0;
  view.dispatch({
    changes: { from: atEnd ? line.from - 1 : line.from, to: atEnd ? line.to : Math.min(doc.length, line.to + 1) },
  });
}

const PRESETS = [25, 50, 75, 100];

/**
 * A small panel by `anchor` (the picture's element) to change what the picture says: its description, its
 * caption and, unless `size` is false (a tile of a grid has the size of its tile), how large it shows. Enter
 * writes the change, Escape or a click elsewhere drops it.
 */
export function openImageEditor(view, anchor, find, { size = true } = {}) {
  document.querySelector(".composer-image-pop")?.remove();
  const now = find();
  if (!now) return;
  const img = anchor.querySelector("img");
  const [src] = now.url.split("#");
  const given = sizeIn(now.url);
  const natural = img?.naturalWidth ? [img.naturalWidth, img.naturalHeight] : given;
  const shown =
    given ??
    (img && img.naturalWidth
      ? [Math.round(img.getBoundingClientRect().width), Math.round(img.getBoundingClientRect().height)]
      : null);
  const ratio = natural ? natural[1] / natural[0] : 0;

  const pop = mk("form", { className: "composer-image-pop" });
  pop.setAttribute("role", "dialog");
  pop.setAttribute("aria-label", tr("Picture"));
  const field = (label, input) => {
    input.placeholder = " ";
    return mk(
      "label",
      { className: "bf-field" },
      input,
      mk("span", { className: "bf-field-label", textContent: label }),
    );
  };
  const alt = mk("input", { name: "alt", value: now.alt, maxLength: 300 });
  const caption = mk("input", { name: "caption", value: now.caption, maxLength: 300 });
  pop.append(field(tr("Description for people who cannot see it"), alt), field(tr("Caption"), caption));

  let width;
  let height;
  let locked = true;
  if (size && natural && shown) {
    width = mk("input", { name: "width", type: "number", min: "16", max: "8000", value: String(shown[0]) });
    height = mk("input", { name: "height", type: "number", min: "16", max: "8000", value: String(shown[1]) });
    const lock = mk("button", { type: "button", className: "bf-icon-btn composer-image-lock" });
    lock.setAttribute("aria-pressed", "true");
    lock.setAttribute("aria-label", tr("Keep the proportions"));
    lock.dataset.tip = tr("Keep the proportions");
    lock.textContent = "🔗";
    lock.addEventListener("click", () => {
      locked = !locked;
      lock.setAttribute("aria-pressed", String(locked));
    });
    width.addEventListener("input", () => {
      if (locked && ratio && Number(width.value) > 0) height.value = String(Math.round(Number(width.value) * ratio));
    });
    height.addEventListener("input", () => {
      if (locked && ratio && Number(height.value) > 0) width.value = String(Math.round(Number(height.value) / ratio));
    });
    const row = mkc("div", "composer-image-size");
    row.append(
      field(tr("Width"), width),
      mk("span", { textContent: "×", ariaHidden: "true" }),
      field(tr("Height"), height),
      lock,
    );
    const presets = mkc("div", "composer-image-presets");
    for (const p of PRESETS) {
      const b = mk("button", { type: "button", className: "btn", textContent: p === 100 ? tr("Original") : `${p}%` });
      b.addEventListener("click", () => {
        width.value = String(Math.max(16, Math.round((natural[0] * p) / 100)));
        height.value = String(Math.max(16, Math.round((natural[1] * p) / 100)));
      });
      presets.append(b);
    }
    pop.append(row, presets);
  }

  const actions = mkc("div", "composer-image-actions");
  const remove = mk("button", { type: "button", className: "btn", textContent: tr("Remove") });
  const apply = mk("button", { type: "submit", className: "btn primary", textContent: tr("Apply") });
  actions.append(remove, apply);
  pop.append(actions);

  const close = () => {
    pop.remove();
    document.removeEventListener("mousedown", onAway);
    document.removeEventListener("keydown", onKey);
    removeEventListener("resize", close);
  };
  const onAway = (e) => !pop.contains(e.target) && close();
  const onKey = (e) => {
    if (e.key !== "Escape") return;
    close();
    view.focus();
  };
  remove.addEventListener("click", () => {
    const at = find();
    close();
    if (at) removeImage(view, at.from, at.to);
    view.focus();
  });
  pop.addEventListener("submit", (e) => {
    e.preventDefault();
    const at = find();
    close();
    if (!at) return view.focus();
    const w = width ? Math.round(Number(width.value)) : 0;
    const h = height ? Math.round(Number(height.value)) : 0;
    const url = w >= 16 && h >= 16 ? `${src}#${w}x${h}` : at.url;
    view.dispatch({
      changes: { from: at.from, to: at.to, insert: imageMarkdown({ alt: alt.value, url, caption: caption.value }) },
    });
    view.focus();
  });

  document.body.append(pop);
  // Under the picture, or above it when there is no room; always inside the window.
  const box = anchor.getBoundingClientRect();
  const below = box.bottom + 8;
  const top = below + pop.offsetHeight <= innerHeight - 8 ? below : Math.max(8, box.top - pop.offsetHeight - 8);
  pop.style.insetBlockStart = `${Math.max(8, Math.min(top, innerHeight - pop.offsetHeight - 8))}px`;
  pop.style.insetInlineStart = `${Math.max(8, Math.min(box.left, innerWidth - pop.offsetWidth - 8))}px`;
  setTimeout(() => document.addEventListener("mousedown", onAway), 0);
  document.addEventListener("keydown", onKey);
  addEventListener("resize", close);
  alt.focus();
  alt.select();
}
