// A context menu for the editor: a list of actions at the pointer, with submenus. It follows the
// usual menu keys (arrows, Enter, Escape, Left and Right for submenus) and closes when anything else
// is clicked or the window changes.
import { icon as svgIcon, mk } from "../dom.js";

/**
 * @typedef {{ label: string, icon?: string, keys?: string, run?: () => void, disabled?: boolean, children?: Item[] } | "-"} Item
 */

let closeCurrent = null;

/** Closes the menu that is open, if any. */
export const closeMenu = () => closeCurrent?.();

/**
 * Opens a menu with its corner at (x, y), moved in if it would run off the window.
 * @param {Item[]} items
 * @param {number} x
 * @param {number} y
 * @param {() => void} [onClose]
 */
export function openMenu(items, x, y, onClose) {
  closeMenu();
  const menus = [];

  const items_of = (menu) =>
    [...menu.children].flatMap((r) => (r.matches(".composer-ctx-row") ? [r.firstElementChild] : []));
  const build = (list, depth) => {
    const menu = mk("div", { className: depth ? "composer-ctx is-sub" : "composer-ctx" });
    menu.setAttribute("role", "menu");
    if (depth) menu.hidden = true;
    for (const it of list) {
      if (it === "-") {
        menu.append(mk("div", { className: "composer-ctx-sep" }));
        menu.lastElementChild?.setAttribute("role", "separator");
        continue;
      }
      const row = mk("div", { className: "composer-ctx-row" });
      const b = mk("button", { type: "button", className: "composer-ctx-item", disabled: !!it.disabled });
      b.setAttribute("role", "menuitem");
      b.tabIndex = -1;
      b.append(it.icon ? svgIcon(it.icon) : mk("span", { className: "composer-ctx-gap" }));
      b.append(mk("span", { className: "composer-ctx-label", textContent: it.label }));
      if (it.keys) b.append(mk("kbd", { textContent: it.keys }));
      row.append(b);
      if (it.children) {
        b.setAttribute("aria-haspopup", "menu");
        b.setAttribute("aria-expanded", "false");
        b.append(svgIcon("chevron-right"));
        const sub = build(it.children, depth + 1);
        row.append(sub);
        const show = () => {
          for (const other of row.parentElement.querySelectorAll(":scope > .composer-ctx-row > .composer-ctx")) {
            if (other !== sub) other.hidden = true;
          }
          sub.hidden = false;
          b.setAttribute("aria-expanded", "true");
          // Beside the item; on the other side or higher up when there is no room.
          sub.classList.remove("is-left");
          const r = sub.getBoundingClientRect();
          if (r.right > innerWidth - 8) sub.classList.add("is-left");
          const over = r.bottom - (innerHeight - 8);
          sub.style.insetBlockStart = over > 0 ? `${-over - 4}px` : "";
        };
        const hide = () => {
          sub.hidden = true;
          b.setAttribute("aria-expanded", "false");
        };
        row.addEventListener("mouseenter", show);
        row.addEventListener("mouseleave", hide);
        b.addEventListener("click", show);
        b.addEventListener("keydown", (e) => {
          if (e.key === "ArrowRight") {
            e.preventDefault();
            e.stopPropagation();
            show();
            items_of(sub)[0]?.focus();
          }
        });
        sub.addEventListener("keydown", (e) => {
          if (e.key === "ArrowLeft") {
            e.preventDefault();
            e.stopPropagation();
            hide();
            b.focus();
          }
        });
      } else {
        b.addEventListener("click", () => {
          close();
          it.run?.();
        });
        row.addEventListener("mouseenter", () => b.focus({ preventScroll: true }));
      }
      menu.append(row);
    }
    menus.push(menu);
    menu.addEventListener("keydown", (e) => {
      const enabled = items_of(menu).filter((i) => !i.disabled);
      const at = enabled.indexOf(/** @type {HTMLButtonElement} */ (document.activeElement));
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        e.stopPropagation();
        const next = e.key === "ArrowDown" ? at + 1 : at - 1;
        enabled[(next + enabled.length) % enabled.length]?.focus();
      }
    });
    return menu;
  };

  const root = build(items, 0);
  const onAway = (e) => !root.contains(e.target) && close();
  const onKey = (e) => {
    if (e.key === "Escape" || e.key === "Tab") {
      e.preventDefault();
      close();
    }
  };
  function close() {
    closeCurrent = null;
    root.remove();
    document.removeEventListener("mousedown", onAway, true);
    document.removeEventListener("keydown", onKey, true);
    removeEventListener("resize", close);
    removeEventListener("blur", close);
    onClose?.();
  }
  closeCurrent = close;
  document.body.append(root);
  root.style.insetInlineStart = `${Math.max(8, Math.min(x, innerWidth - root.offsetWidth - 8))}px`;
  root.style.insetBlockStart = `${Math.max(8, Math.min(y, innerHeight - root.offsetHeight - 8))}px`;
  document.addEventListener("mousedown", onAway, true);
  document.addEventListener("keydown", onKey, true);
  addEventListener("resize", close);
  addEventListener("blur", close);
  // The keyboard starts on the first item that can be used.
  items_of(root)
    .find((i) => !i.disabled)
    ?.focus();
  return close;
}
