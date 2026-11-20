// Quick edit for administrators: a forum's or category's name, description and type, from the
// pencil beside it on the forum pages, without opening the admin panel. It saves through the
// admin API (which asks for the password again when the session needs it) and then swaps the
// changed parts of the page in place.
import { dialog } from "./dialog.js";
import { announce, csrf, el, toast, tr } from "./dom.js";
import { sitePicker } from "./icon-picker.js";

// Parsing only, into an inert document: this site's own page, fetched again after saving.
const policy = window.trustedTypes?.createPolicy("bf-quick-edit", { createHTML: (s) => s }) ?? { createHTML: (s) => s };
const json = { Accept: "application/json", "Content-Type": "application/json" };

const field = (label, control) => {
  const wrap = el("label", { class: "bf-field" });
  if (control.tagName !== "SELECT") control.setAttribute("placeholder", " ");
  wrap.append(control, el("span", { class: "bf-field-label" }, label));
  return wrap;
};

const MODES = [
  ["discussion", tr("Discussion")],
  ["qa", tr("Questions and answers")],
  ["ideas", tr("Ideas (members vote)")],
];

// The page as it is now: every block for this forum (its row, heading or page header) and, on
// its own page, the breadcrumb and the tab title.
async function refresh(id) {
  const res = await fetch(location.pathname + location.search).catch(() => null);
  if (!res?.ok) return;
  // A new name is a new address: the page's own follows it.
  if (res.redirected) {
    const to = new URL(res.url);
    history.replaceState(history.state, "", to.pathname + to.search + location.hash);
  }
  const doc = new DOMParser().parseFromString(policy.createHTML(await res.text()), "text/html");
  for (const now of document.querySelectorAll(`[data-node="${id}"]`)) {
    const fresh = doc.querySelector(`[data-node="${id}"]${now.tagName === "HEADER" ? "" : ":not(header)"}`);
    if (fresh) now.replaceChildren(...[...fresh.childNodes].map((n) => document.adoptNode(n)));
  }
  if (document.querySelector(`header[data-node="${id}"]`)) {
    const crumb = doc.querySelector("nav.crumbs > span");
    if (crumb) document.querySelector("nav.crumbs > span")?.replaceChildren(crumb.textContent);
    if (doc.title) document.title = document.title.replace(/^(\(\d+\) )?.*$/, (_, count = "") => count + doc.title);
  }
}

export async function quickEdit(id, opener) {
  const res = await fetch(`/api/v1/admin/nodes/${id}`, { headers: { Accept: "application/json" } }).catch(() => null);
  if (!res?.ok) {
    toast(tr("This forum could not be loaded for editing."), true);
    return;
  }
  const node = await res.json();
  const category = node.kind === "category";

  const form = el("form");
  const title = el("input", { name: "title", required: "", maxlength: "120", autocomplete: "off" });
  title.value = node.title;
  const description = el("textarea", { name: "description", rows: "4" });
  description.value = node.description;
  const mode = el("select", { name: "mode" });
  for (const [value, label] of MODES) {
    const o = el("option", { value }, label);
    if (value === node.mode) o.selected = true;
    mode.append(o);
  }
  // Shown when the server asks for the password again before the change.
  const password = el("input", { type: "password", name: "password", autocomplete: "current-password" });
  const sudo = field(tr("Confirm your password to save"), password);
  sudo.hidden = true;
  const error = el("p", { class: "bf-dialog-error", role: "alert", hidden: "" });

  const more = el("a", { class: "bf-dialog-aside small", href: `/admin/forums/#node=${id}` }, tr("All settings"));
  const cancel = el("button", { type: "button", class: "btn", "data-dialog-close": "" }, tr("Cancel"));
  const save = el("button", { type: "submit", class: "btn primary" }, tr("Save"));
  const actions = el("div", { class: "bf-dialog-actions" });
  actions.append(more, cancel, save);

  form.append(field(tr("Name"), title), field(tr("Description (Markdown)"), description));
  if (!category) form.append(field(tr("Type"), mode));
  // The same icon picker as the admin panel. An uploaded picture takes priority over the icon,
  // so it is said here rather than letting a change appear to do nothing.
  let icon = node.icon;
  const iconBox = el("fieldset", { class: "bf-quick-icon" });
  iconBox.append(el("legend", {}, tr("Icon")));
  if (node.icon_image)
    iconBox.append(
      el(
        "p",
        { class: "small muted" },
        tr("This forum shows an uploaded picture instead; remove it in All settings to use an icon."),
      ),
    );
  form.append(iconBox);
  sitePicker({
    value: /^[a-z0-9-]+$/.test(node.icon) ? node.icon : "",
    name: `icon-${id}`,
    onChange: (v) => (icon = v),
  }).then((p) => {
    iconBox.append(p.element);
    p.reveal();
  });
  form.append(sudo, error, actions);
  const box = dialog({ title: category ? tr("Edit category") : tr("Edit forum"), body: [form] });
  const close = () => box.close();

  const fail = (text) => {
    error.textContent = text;
    error.hidden = false;
  };
  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    error.hidden = true;
    save.disabled = true;
    try {
      if (!sudo.hidden) {
        const ok = await fetch("/api/v1/sudo", {
          method: "POST",
          headers: { ...json, "X-CSRF-Token": csrf() },
          body: JSON.stringify({ [password.name]: password.value }),
        }).catch(() => null);
        if (!ok?.ok) {
          const p = await ok?.json().catch(() => null);
          fail(p?.detail ?? tr("That password did not work."));
          password.value = "";
          password.focus();
          return;
        }
      }
      // Only what was changed, so a field left alone keeps exactly what it had.
      const body = {};
      if (title.value.trim() !== node.title) body.title = title.value.trim();
      if (description.value !== node.description) body.description = description.value;
      if (!category && mode.value !== node.mode) body.mode = mode.value;
      const iconChanged = icon !== node.icon;
      if (!Object.keys(body).length && !iconChanged) return close();
      // Text first, then the icon (its own endpoint, keeping any uploaded picture).
      const send = async () => {
        if (Object.keys(body).length) {
          const r = await fetch(`/api/v1/admin/nodes/${id}`, {
            method: "PATCH",
            headers: { ...json, "X-CSRF-Token": csrf() },
            body: JSON.stringify(body),
          }).catch(() => null);
          if (!r?.ok) return r;
          for (const k of Object.keys(body)) node[k] = body[k];
          for (const k of Object.keys(body)) delete body[k];
        }
        if (!iconChanged) return { ok: true };
        const r = await fetch(`/api/v1/admin/nodes/${id}/icon`, {
          method: "PUT",
          headers: { ...json, "X-CSRF-Token": csrf() },
          body: JSON.stringify({ icon, image: node.icon_image }),
        }).catch(() => null);
        if (r?.ok) node.icon = icon;
        return r;
      };
      const r = await send();
      if (!r) return fail(tr("Could not reach the server. Try again."));
      if (!r.ok) {
        const p = await r.json().catch(() => null);
        if (p?.type === "sudo-required") {
          const how = await fetch("/api/v1/sudo", { headers: { Accept: "application/json" } })
            .then((x) => x.json())
            .catch(() => null);
          if (how?.method === "none") {
            return fail(tr("This account signs in without a password: sign out and in again, then save."));
          }
          if (how?.method === "code") {
            Object.assign(password, {
              type: "text",
              name: "code",
              inputMode: "numeric",
              autocomplete: "one-time-code",
            });
            sudo.querySelector(".bf-field-label").textContent = tr("Authenticator code to save");
          }
          sudo.hidden = false;
          password.focus();
          return;
        }
        return fail(p?.detail ?? tr("The change was not saved."));
      }
      close();
      await refresh(id);
      announce(tr("Saved."));
      toast(tr("Saved."));
    } finally {
      save.disabled = false;
    }
  });

  box.show(opener);
  title.select();
}
