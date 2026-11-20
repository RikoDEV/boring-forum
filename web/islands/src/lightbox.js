import { el, icon, tr } from "./dom.js";

// The image viewer, one for the whole site: pictures in posts and chat (topic.js) open here. It is a full-screen modal dialog with a
// counter, the way to the original, zoom, a strip of thumbnails to jump along, arrows at the sides,
// and the keyboard and a swipe to move. Pictures next to the one opened are loaded ahead.
//
//   openLightbox([{ src, thumb, alt, page }], startIndex)
//
// `src` is the full picture, `thumb` the small one for the strip (the picture itself when
// missing), `alt` its caption, and `page` where its details are, when it has a page of its own.
// Styles: themes/default/css/components/lightbox.css.

/** @typedef {{ src: string, thumb?: string, alt?: string, page?: string }} Shot */

/** @type {{ dialog: HTMLDialogElement, show: (items: Shot[], at: number) => void } | null} */
let viewer = null;

const SWIPE = 50;

/** What editors put in a picture's description when nobody wrote one ("Image", "Obraz", "Screenshot 2"). */
const GENERIC = /^(image|img|picture|photo|screenshot|obraz|obrazek|zdj[ęe]cie|zrzut ekranu|صورة)[\s_-]*\d*$/i;

/** A caption worth showing: not empty, not just the file's name, not a stand-in word. */
const caption = (alt = "") => (/\.(jpe?g|png|gif|webp|avif)$/i.test(alt.trim()) || GENERIC.test(alt.trim()) ? "" : alt.trim());

function build() {
  const dialog = /** @type {HTMLDialogElement} */ (el("dialog", { class: "lb", "aria-label": tr("Image viewer") }));
  // The tools along the top say what they do on hover, below them; the arrows are plain.
  const button = (label, glyph, run, extra = "") => {
    const b = el("button", { type: "button", class: `lb-btn ${extra}`.trim(), "aria-label": label });
    if (!extra.includes("lb-nav")) {
      b.dataset.tip = label;
      b.dataset.tipSide = "below";
    }
    b.append(icon(glyph));
    b.addEventListener("click", run);
    return b;
  };
  const link = (label, glyph) => {
    const a = el("a", { class: "lb-btn", "aria-label": label, "data-tip": label, "data-tip-side": "below", target: "_blank", rel: "noopener" });
    a.append(icon(glyph));
    return a;
  };

  const count = el("span", { class: "lb-count", "aria-live": "polite" });
  const zoom = button(tr("Zoom"), "maximize-2", () => toggleZoom(), "lb-zoom");
  const original = link(tr("Open the original"), "link");
  const details = link(tr("Details"), "image");
  details.removeAttribute("target");
  const close = button(tr("Close"), "x", () => dialog.close(), "lb-close");
  const tools = el("div", { class: "lb-tools" });
  tools.append(zoom, original, details, close);
  const top = el("div", { class: "lb-top" });
  top.append(count, tools);

  const prev = button(tr("Previous image"), "chevron-right", () => go(-1), "lb-nav lb-prev");
  const next = button(tr("Next image"), "chevron-right", () => go(1), "lb-nav lb-next");
  const big = el("img", { class: "lb-img", alt: "", draggable: "false" });
  const note = el("figcaption", { class: "lb-caption" });
  const figure = el("figure", { class: "lb-figure" });
  figure.append(big, note);
  const spinner = el("span", { class: "lb-spinner", "aria-hidden": "true" });
  // The stage takes the focus when the viewer opens, so no button shows a ring nobody asked for; the
  // keys still reach the dialog, and Tab goes on to the first tool.
  const stage = el("div", { class: "lb-stage", tabindex: "-1", autofocus: "" });
  stage.append(figure, spinner, prev, next);
  const strip = el("div", { class: "lb-strip", role: "group", "aria-label": tr("Images") });
  dialog.append(top, stage, strip);

  /** @type {Shot[]} */
  let items = [];
  let at = 0;
  let zoomed = false;

  const rtl = () => getComputedStyle(dialog).direction === "rtl";
  const reduced = () => matchMedia("(prefers-reduced-motion: reduce)").matches;

  function toggleZoom(on = !zoomed) {
    zoomed = on;
    figure.classList.toggle("is-zoomed", zoomed);
    zoom.setAttribute("aria-pressed", String(zoomed));
    stage.scrollTo({ left: 0, top: 0 });
  }

  function preload(i) {
    if (items.length < 2) return;
    const n = items.length;
    for (const j of [i + 1, i - 1]) new Image().src = items[(j + n) % n].src;
  }

  function show(list, start) {
    if (list !== items) {
      items = list;
      strip.replaceChildren();
      strip.hidden = items.length < 2;
      items.forEach((it, i) => {
        const t = el("button", { type: "button", class: "lb-thumb", "aria-label": tr("Image {n} of {total}", { n: i + 1, total: items.length }) });
        const pic = el("img", { src: it.thumb || it.src, alt: "", loading: "lazy", draggable: "false" });
        t.append(pic);
        t.addEventListener("click", () => show(items, i));
        strip.append(t);
      });
      dialog.classList.toggle("is-single", items.length < 2);
    }
    at = ((start % items.length) + items.length) % items.length;
    const it = items[at];
    toggleZoom(false);
    figure.classList.remove("is-loaded");
    spinner.hidden = false;
    const loaded = () => {
      figure.classList.add("is-loaded");
      spinner.hidden = true;
    };
    big.onload = big.onerror = loaded;
    big.src = it.src;
    big.alt = it.alt || "";
    if (big.complete && big.naturalWidth) loaded();
    // Captions that are the same on every picture ("Screenshot") say nothing.
    const generic = items.length > 1 && items.every((x) => x.alt === it.alt);
    note.textContent = generic ? "" : caption(it.alt);
    count.textContent = `${at + 1} / ${items.length}`;
    count.hidden = items.length < 2;
    original.href = it.src;
    details.hidden = !it.page;
    if (it.page) details.href = it.page;
    [...strip.children].forEach((t, i) => {
      const on = i === at;
      if (on) t.setAttribute("aria-current", "true");
      else t.removeAttribute("aria-current");
      if (on) t.scrollIntoView({ inline: "center", block: "nearest", behavior: reduced() ? "auto" : "smooth" });
    });
    preload(at);
  }

  function go(by) {
    if (items.length > 1) show(items, at + by);
  }

  dialog.addEventListener("keydown", (e) => {
    const side = rtl() ? -1 : 1;
    if (e.key === "ArrowRight") go(side);
    else if (e.key === "ArrowLeft") go(-side);
    else if (e.key === "Home" && items.length > 1) show(items, 0);
    else if (e.key === "End" && items.length > 1) show(items, items.length - 1);
    else if (e.key === "+" || e.key === "-" || e.key === "z") toggleZoom();
    else return;
    e.preventDefault();
  });

  // A click on the picture zooms; one beside it closes, like a click on the backdrop would.
  big.addEventListener("click", () => toggleZoom());
  stage.addEventListener("click", (e) => {
    if (e.target === stage || e.target === figure) dialog.close();
  });
  top.addEventListener("click", (e) => {
    if (e.target === top) dialog.close();
  });

  // A swipe along the picture moves to the next one.
  let from = null;
  stage.addEventListener("pointerdown", (e) => {
    if (e.pointerType === "mouse" || zoomed) return;
    from = { x: e.clientX, y: e.clientY };
  });
  stage.addEventListener("pointerup", (e) => {
    if (!from) return;
    const dx = e.clientX - from.x;
    const dy = e.clientY - from.y;
    from = null;
    if (Math.abs(dx) > SWIPE && Math.abs(dy) < Math.abs(dx) * 0.6) go(dx < 0 ? (rtl() ? -1 : 1) : rtl() ? 1 : -1);
  });
  stage.addEventListener("pointercancel", () => (from = null));

  dialog.addEventListener("close", () => {
    toggleZoom(false);
    big.removeAttribute("src");
  });
  document.body.append(dialog);
  return { dialog, show };
}

/**
 * Opens the viewer on `items`, at `start`.
 * @param {Shot[]} items
 * @param {number} [start]
 */
export function openLightbox(items, start = 0) {
  if (!items.length || typeof HTMLDialogElement === "undefined") return;
  viewer ??= build();
  viewer.show(items, start);
  if (!viewer.dialog.open) viewer.dialog.showModal();
}
