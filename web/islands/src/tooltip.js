// The site's one hover bubble (also used by the admin panel). It is a single element in the
// browser's top layer (a manual popover), so no ancestor can clip it: not a box with
// `overflow: hidden`, a dialog, a scroller. It is placed from the trigger's position on screen,
// above it when there is room and below when there is not, and kept inside the window.
//
//   data-tip="Text"            any element; `data-tip-side="below"` or `"end"` (beside it, after it
//                              in reading order) picks the side; `data-tip-gate` wraps long text
//   .bf-icon-btn[aria-label]   icon buttons use their label as the text (`extra` adds more selectors)
//
// Shown after a short delay on hover and on keyboard focus; never on touch screens, and not for
// the button of a menu that is open (its panel is where the eyes are).

const GAP = 8;
const EDGE = 6;
const DELAY = 300;
let started = false;

/** @param {string} [extra] more selectors whose aria-label is a tooltip, e.g. ".acp-icon-btn" */
export function tooltips(extra = "") {
  if (started) return;
  started = true;
  const triggers = ["[data-tip]", ".bf-icon-btn[aria-label]", ...(extra ? extra.split(",") : [])].map((s) => s.trim());
  const SEL = triggers.join(",");
  /** @type {HTMLElement | null} */
  let tip = null;
  /** @type {Element | null} */
  let target = null;
  let timer = 0;

  const textOf = (/** @type {Element} */ el) => el.getAttribute("data-tip") ?? el.getAttribute("aria-label") ?? "";

  function hide() {
    clearTimeout(timer);
    target = null;
    if (tip?.matches(":popover-open")) tip.hidePopover();
  }

  function place(/** @type {Element} */ el, /** @type {HTMLElement} */ bubble) {
    const r = el.getBoundingClientRect();
    const { width: w, height: h } = bubble.getBoundingClientRect();
    // The room the page has: a reserved scrollbar gutter is part of the window but not of the page.
    const root = document.documentElement;
    const vw = Math.min(root.clientWidth, root.getBoundingClientRect().width);
    const vh = document.documentElement.clientHeight;
    const rtl = getComputedStyle(el).direction === "rtl";
    const side = el.getAttribute("data-tip-side");
    let x;
    let y;
    if (side === "end") {
      x = rtl ? r.left - GAP - w : r.right + GAP;
      y = r.top + (r.height - h) / 2;
      // No room beside it: the other side.
      if (x < EDGE || x + w > vw - EDGE) x = rtl ? r.right + GAP : r.left - GAP - w;
    } else {
      // A long reason lines up with the end of its control, so it stays on screen; short text centres.
      x = el.hasAttribute("data-tip-gate") ? (rtl ? r.left : r.right - w) : r.left + (r.width - w) / 2;
      const below = side === "below";
      y = below ? r.bottom + GAP : r.top - GAP - h;
      // The side that has no room gives way to the other.
      if (y < EDGE && !below) y = r.bottom + GAP;
      else if (y + h > vh - EDGE && below && r.top - GAP - h >= EDGE) y = r.top - GAP - h;
    }
    x = Math.max(EDGE, Math.min(x, vw - w - EDGE));
    y = Math.max(EDGE, Math.min(y, vh - h - EDGE));
    bubble.style.left = `${Math.round(x)}px`;
    bubble.style.top = `${Math.round(y)}px`;
  }

  function show(/** @type {Element} */ el) {
    const text = textOf(el);
    if (!text || !el.isConnected) return;
    if (!tip) {
      tip = document.createElement("div");
      tip.className = "bf-tip";
      tip.setAttribute("popover", "manual");
      tip.setAttribute("role", "tooltip");
      document.body.append(tip);
    }
    tip.textContent = text;
    tip.classList.toggle("is-gate", el.hasAttribute("data-tip-gate"));
    if (!tip.matches(":popover-open")) tip.showPopover();
    place(el, tip);
  }

  function schedule(/** @type {Element} */ el) {
    // The button of an open menu: its panel is what to look at.
    if (el.matches("details[open] > summary")) return;
    clearTimeout(timer);
    target = el;
    timer = window.setTimeout(() => target === el && show(el), DELAY);
  }

  document.addEventListener("mouseover", (e) => {
    if (matchMedia("(hover: none)").matches) return;
    const el = e.target instanceof Element ? e.target.closest(SEL) : null;
    if (el === target) return;
    hide();
    if (el) schedule(el);
  });
  document.addEventListener("mouseout", (e) => {
    if (target && !(e.relatedTarget instanceof Node && target.contains(e.relatedTarget))) hide();
  });
  document.addEventListener("focusin", (e) => {
    const el = e.target instanceof Element ? e.target.closest(SEL) : null;
    if (el?.matches(":focus-visible")) schedule(el);
  });
  document.addEventListener("focusout", hide);
  document.addEventListener("pointerdown", hide, true);
  document.addEventListener("keydown", hide, true);
  window.addEventListener("scroll", hide, { capture: true, passive: true });
  window.addEventListener("resize", hide);
}
