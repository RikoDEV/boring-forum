// Sudo mode for admin writes made from a script: when a write needs the password confirmed
// again (`sudo-required`), a dialog asks for it over the page, and the write is retried. Used by
// the admin panel and by the forum's layout editor, each with its own text lookup and CSRF token.
//
// One dialog at a time: requests that need confirming while it is open wait for the same answer.

import { dialog } from "./dialog.js";
import { tr } from "./dom.js";

/**
 * @param {{ text: () => Record<"code" | "password" | "confirm" | "cancel" | "none" | "lead" | "title" | "checking" | "failed" | "offline", string>, csrf: () => string }} deps
 * @returns {{ confirmSudo: () => Promise<boolean> }}
 */
export function createSudo({ text: getText, csrf }) {
  let pending = null;

  const el = (tag, attrs = {}, ...kids) => {
    const e = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs)) {
      if (v === true) e.setAttribute(k, "");
      else if (v !== false && v != null) e.setAttribute(k, v);
    }
    e.append(...kids);
    return e;
  };

  /** Resolves true once the password is confirmed, false if the dialog was dismissed. */
  function confirmSudo() {
    pending ??= ask().finally(() => (pending = null));
    return pending;
  }

  async function ask() {
    // Looked up now, not when the module loads: the language is known by then.
    const text = getText();
    const info = await fetch("/api/v1/sudo", { headers: { Accept: "application/json" }, credentials: "same-origin" })
      .then((r) => (r.ok ? r.json() : null))
      .catch(() => null);
    if (info?.confirmed) return true;
    const method = info?.method ?? "password";

    return new Promise((resolve) => {
      const error = el("p", { class: "err", role: "alert", hidden: true });
      const input =
        method === "code"
          ? el("input", {
              id: "sudo-secret",
              name: "code",
              type: "text",
              inputmode: "numeric",
              autocomplete: "one-time-code",
              maxlength: "10",
              required: true,
              placeholder: " ",
            })
          : el("input", {
              id: "sudo-secret",
              name: "password",
              type: "password",
              autocomplete: "current-password",
              required: true,
              placeholder: " ",
            });
      const field = el(
        "div",
        { class: "bf-field" },
        input,
        el("label", { class: "bf-field-label", for: "sudo-secret" }, method === "code" ? text.code : text.password),
      );
      const confirm = el("button", { type: "submit", class: "btn primary" }, text.confirm);
      const cancel = el("button", { type: "button", class: "btn" }, text.cancel);
      const lead = method === "none" ? text.none : text.lead;
      const form = el(
        "form",
        {},
        el(
          "div",
          { class: "bf-dialog-body" },
          el("p", { class: "muted" }, lead),
          ...(method === "none" ? [] : [field]),
          error,
        ),
        el("div", { class: "bf-dialog-actions" }, cancel, ...(method === "none" ? [] : [confirm])),
      );
      let done = false;
      const finish = (ok) => {
        if (done) return;
        done = true;
        box.close();
        resolve(ok);
      };
      // The site's own dialog. Closing it any way (×, Escape, beside it, Cancel) is a "no".
      const box = dialog({
        title: text.title,
        className: "is-narrow",
        body: [],
        onClose: () => finish(false),
      });
      box.body.replaceWith(form);
      cancel.addEventListener("click", () => finish(false));
      form.addEventListener("submit", async (e) => {
        e.preventDefault();
        error.hidden = true;
        confirm.disabled = true;
        confirm.textContent = text.checking;
        try {
          const res = await fetch("/api/v1/sudo", {
            method: "POST",
            headers: { "Content-Type": "application/json", Accept: "application/json", "X-CSRF-Token": csrf() },
            credentials: "same-origin",
            body: JSON.stringify({ [input.name]: input.value }),
          });
          if (res.ok) return finish(true);
          const p = await res.json().catch(() => ({}));
          error.textContent = p.detail || p.title || text.failed;
        } catch {
          error.textContent = text.offline;
        }
        error.hidden = false;
        confirm.disabled = false;
        confirm.textContent = text.confirm;
        input.value = "";
        input.focus();
      });
      box.show();
      (method === "none" ? cancel : input).focus();
    });
  }

  return { confirmSudo };
}

// The forum's lookups: the site catalog (the admin panel passes its own).
export const siteSudo = (/** @type {() => string} */ csrf) =>
  createSudo({
    csrf,
    text: () => ({
      code: tr("Authenticator code"),
      password: tr("Password"),
      confirm: tr("Confirm"),
      cancel: tr("Cancel"),
      none: tr("This account signs in without a password. Sign out and sign in again, then try once more."),
      lead: tr("For your security, confirm it's you before changing this. You won't be asked again for a while."),
      title: tr("Confirm it's you"),
      checking: tr("Checking…"),
      failed: tr("That did not work. Try again."),
      offline: tr("Could not reach the server. Try again."),
    }),
  });
