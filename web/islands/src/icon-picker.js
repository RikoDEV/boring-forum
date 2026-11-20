// The icon picker, the one used everywhere: in the admin panel (wrapped by
// web/admin/src/lib/IconPicker.svelte) and on the site (the forum quick edit). Choose one icon,
// or none: general icons, then brand logos, with a search over both. The grid scrolls, so the
// name of the icon under the pointer shows in a line below it rather than as a tooltip the
// scroll would clip. Styles: themes/default/css/components/icon-picker.css.

import { tr } from "./dom.js";

// Browser and system logos and editor controls make poor labels.
const NOT_LABELS =
  /^(b-|os-)|^(x|menu|minus|plus|ellipsis|chevron-.*|grip-vertical|corner-down-right|panel-bottom|maximize-2|bold|italic|log-out|trash-2)$/;

/** The general icons in a sprite's source, in sprite order. */
export const iconList = (spriteText) =>
  [...spriteText.matchAll(/id="([a-z0-9-]+)"/g)].map((m) => m[1]).filter((n) => !NOT_LABELS.test(n));

const SVG = "http://www.w3.org/2000/svg";
const el = (tag, attrs = {}, text) => {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, v);
  if (text !== undefined) e.textContent = text;
  return e;
};

/**
 * @param {object} o
 * @param {string} [o.value] the chosen icon, "" for none
 * @param {string} [o.name] the radio group's name
 * @param {string[]} o.icons general icon names
 * @param {string[]} [o.brands] brand icon names (`brand-*`)
 * @param {Record<string,string>} [o.brandNames] readable brand names
 * @param {(name: string) => string} o.href where an icon is drawn from
 * @param {boolean} [o.none] offer "no icon"
 * @param {(value: string) => void} [o.onChange]
 */
export function iconPicker({
  value = "",
  name = "icon",
  icons,
  brands = [],
  brandNames = {},
  href,
  none = true,
  onChange,
}) {
  const label = (n) => brandNames[n] ?? n.replaceAll("-", " ");
  const root = el("div", { class: "bf-iconpicker" });
  const find = el("input", {
    type: "search",
    placeholder: brands.length ? tr("Find an icon or brand") : tr("Find an icon"),
    "aria-label": tr("Find an icon"),
  });
  const grid = el("div", { class: "bf-iconpicker-grid" });
  const caption = el("p", { class: "bf-iconpicker-name muted", "aria-live": "polite" });
  root.append(find, grid, caption);
  let hovered = "";

  const paintCaption = () => {
    const shown = hovered || value;
    caption.textContent = !shown
      ? tr("No icon chosen")
      : hovered && hovered !== value
        ? label(hovered)
        : tr("Chosen: {name}", {
            name: value.startsWith("brand-") || value.match(/^[a-z0-9-]+$/) ? label(value) : value,
          });
  };
  const choice = (n) => {
    const box = el("label", { class: `bf-icon-choice${n === value ? " is-on" : ""}`, "data-icon": n });
    const radio = el("input", { type: "radio", name, value: n, "aria-label": n ? label(n) : tr("No icon") });
    radio.checked = n === value;
    box.append(radio);
    if (n) {
      const svg = document.createElementNS(SVG, "svg");
      svg.setAttribute("class", "bf-icon");
      svg.setAttribute("aria-hidden", "true");
      const use = document.createElementNS(SVG, "use");
      use.setAttribute("href", href(n));
      svg.append(use);
      box.append(svg);
    } else box.append(el("span", { "aria-hidden": "true" }, "—"));
    box.addEventListener("mouseenter", () => {
      hovered = n;
      paintCaption();
    });
    radio.addEventListener("change", () => set(n, true));
    return box;
  };
  const section = (title, names) => {
    if (!names.length) return;
    if (title) grid.append(el("p", { class: "bf-iconpicker-head" }, title));
    const row = el("div", { class: "bf-iconpick" });
    row.append(...names.map(choice));
    grid.append(row);
  };
  const render = () => {
    const q = find.value.trim().toLowerCase();
    const match = (n) => label(n).toLowerCase().includes(q);
    grid.replaceChildren();
    const general = icons.filter(match);
    const brand = brands.filter(match);
    // "No icon" is the first icon of the list, not a row of its own above the heading.
    section(tr("Icons"), none && !q ? ["", ...general] : general);
    section(tr("Brands"), brand);
    if (!general.length && !brand.length)
      grid.append(el("p", { class: "muted" }, tr("No icons match “{q}”.", { q: find.value })));
  };
  function set(next, fromUser = false) {
    value = next ?? "";
    for (const b of grid.querySelectorAll(".bf-icon-choice")) {
      const on = b.dataset.icon === value;
      b.classList.toggle("is-on", on);
      b.querySelector("input").checked = on;
    }
    paintCaption();
    if (fromUser) onChange?.(value);
  }
  find.addEventListener("input", render);
  grid.addEventListener("mouseleave", () => {
    hovered = "";
    paintCaption();
  });
  render();
  paintCaption();

  return {
    element: root,
    set: (v) => v !== value && set(v),
    /** Bring the chosen icon into view inside the grid, once it has a layout. */
    reveal() {
      requestAnimationFrame(() => {
        const on = grid.querySelector(".is-on");
        if (on) grid.scrollTop = on.offsetTop - grid.clientHeight / 2 + on.offsetHeight / 2;
      });
    },
  };
}

/**
 * For site pages: the icons come from the page's own sprite, brands from the brand sprite the
 * layout names in `<meta name="bf-brands">`.
 */
export async function sitePicker(options) {
  const sprite = document.querySelector(".bf-icon use")?.getAttribute("href")?.split("#")[0] ?? "";
  const brandSprite = document.querySelector('meta[name="bf-brands"]')?.content ?? "";
  const [text, brandNames] = await Promise.all([
    sprite
      ? fetch(sprite)
          .then((r) => (r.ok ? r.text() : ""))
          .catch(() => "")
      : "",
    import("../../../themes/default/brand-names.json").then((m) => m.default).catch(() => ({})),
  ]);
  const brands = brandSprite ? Object.keys(brandNames).sort((a, b) => brandNames[a].localeCompare(brandNames[b])) : [];
  return iconPicker({
    ...options,
    icons: iconList(text),
    brands,
    brandNames,
    href: (n) => `${n.startsWith("brand-") ? brandSprite : sprite}#${n}`,
  });
}
