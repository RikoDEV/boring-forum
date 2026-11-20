// The site's one modal dialog. Every dialog, whether a script builds it or the server renders it,
// has the same parts and behaviour:
//
//   <dialog class="bf-dialog">
//     <header class="bf-dialog-head"><h2>Title</h2><button data-dialog-close>×</button></header>
//     <div class="bf-dialog-body">…</div>
//     <div class="bf-dialog-actions">…buttons…</div>      (or inside a form in the body)
//   </dialog>
//
// It opens as a true modal (the page behind is inert and, by CSS, does not scroll), focuses its
// first field, closes on ×, Escape or a click beside it (each can be turned off), and gives focus
// back to whatever opened it. Styles: themes/default/css/components/dialog.css.
//
// Server-rendered dialogs open from a button with `data-dialog="<dialog id>"`; any element with
// `data-dialog-close` inside one closes it.
import { el, icon, tr } from "./dom.js";

let ids = 0;

/** Gives a dialog element the shared behaviour; safe to call twice. */
function wire(d, { escape = true, backdrop = true } = {}) {
  if (d.dataset.wired) return d;
  d.dataset.wired = "1";
  d.addEventListener("cancel", (e) => {
    if (!escape) e.preventDefault();
  });
  // Escape and a click beside the dialog are the browser's own `closedby` where it has one. Only
  // "Escape off, backdrop on" has no value for it, and older browsers have none at all.
  const native = "closedBy" in HTMLDialogElement.prototype && escape;
  if (native) d.closedBy = backdrop ? "any" : "closerequest";
  const byClick = backdrop && !native;
  d.addEventListener("click", (e) => {
    // The dialog has no padding of its own, so a click on it is a click on the backdrop.
    if ((byClick && e.target === d) || e.target.closest?.("[data-dialog-close]")) d.close();
  });
  d.addEventListener("close", () => {
    const back = d._opener;
    d._opener = null;
    if (back?.isConnected) back.focus();
  });
  return d;
}

/** Opens a wired dialog, remembering what to give focus back to. */
function show(d, opener = document.activeElement) {
  d._opener = opener;
  if (!d.isConnected) document.body.append(d);
  d.showModal();
  firstField(d)?.focus();
}

/** The field to start in: one marked autofocus, or the first visible text field or list. */
export function firstField(d) {
  const fields = d.querySelectorAll(
    "[autofocus], .bf-dialog-body :is(textarea, input:not([type=hidden], [type=search], [type=radio], [type=checkbox]), select)",
  );
  return [...fields].find((f) => f.checkVisibility?.() ?? f.offsetParent !== null);
}

/**
 * A dialog built by a script. It is removed from the page when it closes.
 * @param {object} o
 * @param {string} o.title
 * @param {string} [o.className] extra classes, e.g. "is-wide"
 * @param {Node[]} [o.body]
 * @param {Node[]} [o.actions] buttons for the row at the bottom
 * @param {boolean} [o.closeButton] the × in the header
 * @param {boolean} [o.escape] Escape closes it
 * @param {boolean} [o.backdrop] a click beside it closes it
 * @param {boolean} [o.keep] stay in the page after closing, to be shown again
 * @param {(value: string) => void} [o.onClose] with the value `close(value)` was given
 */
export function dialog({
  title,
  className = "",
  body = [],
  actions = [],
  closeButton = true,
  escape = true,
  backdrop = true,
  keep = false,
  onClose,
}) {
  const id = `bf-dialog-${++ids}`;
  const d = el("dialog", { class: `bf-dialog ${className}`.trim(), "aria-labelledby": id });
  const head = el("header", { class: "bf-dialog-head" });
  head.append(el("h2", { id }, title));
  if (closeButton) {
    const x = el("button", {
      type: "button",
      class: "bf-icon-btn",
      "data-dialog-close": "",
      "aria-label": tr("Close"),
    });
    x.append(icon("x"));
    head.append(x);
  }
  const main = el("div", { class: "bf-dialog-body" });
  main.append(...body);
  d.append(head, main);
  if (actions.length) {
    const row = el("div", { class: "bf-dialog-actions" });
    row.append(...actions);
    d.append(row);
  }
  wire(d, { escape, backdrop });
  d.addEventListener("close", () => {
    if (!keep) d.remove();
    onClose?.(d.returnValue);
  });
  return {
    element: d,
    head,
    body: main,
    show: (opener) => show(d, opener),
    close: (value = "") => d.close(value),
  };
}

// Server-rendered dialogs: the button names the dialog it opens. Once per page, though both the
// islands and the editor bundle include this module.
if (!window.bfDialogs) {
  window.bfDialogs = true;
  document.addEventListener("click", (e) => {
    const opener = e.target.closest?.("[data-dialog]");
    const d = opener && document.getElementById(opener.dataset.dialog);
    if (!(d instanceof HTMLDialogElement) || d.open) return;
    e.preventDefault();
    // `data-fill-<name>="value"` on the button sets the dialog's field called <name> (what a
    // shared report dialog is about).
    for (const [key, value] of Object.entries(opener.dataset)) {
      if (!key.startsWith("fill") || key.length < 5) continue;
      const name = key[4].toLowerCase() + key.slice(5);
      const field = d.querySelector(`[name="${CSS.escape(name)}"]`);
      if (field instanceof HTMLInputElement || field instanceof HTMLTextAreaElement) field.value = value ?? "";
    }
    wire(d);
    show(d, opener);
  });
}
