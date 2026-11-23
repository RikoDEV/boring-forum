// The dialog under a post's reaction pills: who reacted, a tab for each reaction and one for all of
// them, the newest first. It opens from `[data-reactions="<post id>"]` (topic.js loads this file the
// first time one is clicked). Styles: themes/default/css/components/reactions.css.
import { dialog } from "./dialog.js";
import { el, tr, userLink, when } from "./dom.js";

/**
 * @typedef {{ name: string, at: string }} Reactor
 * @typedef {{ id: number, icon: string, name: string, users: Reactor[] }} Group
 */

/** One row: the member's picture, their name, when they reacted, and the reaction they chose. */
function row(user, group) {
  const li = el("li", { class: "bf-reactor" });
  const img = el("img", {
    class: "avatar",
    src: `/avatar/${encodeURIComponent(user.name)}.svg`,
    width: 32,
    height: 32,
    alt: "",
    loading: "lazy",
  });
  const who = el("div", { class: "bf-reactor-who" });
  const name = el("strong");
  name.append(userLink(user.name));
  const small = el("small", { class: "muted" });
  small.append(when(null, user.at));
  who.append(name, small);
  li.append(img, who);
  // The reaction on every row, in each tab too: a list with one reaction still says which.
  const mark = el("span", { class: "bf-reactor-icon", "aria-label": group.name }, group.icon);
  mark.dataset.tip = group.name;
  li.append(mark);
  return li;
}

/** @param {HTMLElement} body @param {Group[]} groups */
function render(body, groups) {
  const all = groups
    .flatMap((g) => g.users.map((u) => ({ user: u, group: g })))
    .sort((a, b) => b.user.at.localeCompare(a.user.at));
  if (!all.length) {
    body.replaceChildren(el("p", { class: "muted" }, tr("No reactions yet.")));
    return;
  }
  const list = el("ul", { class: "bf-reactors" });
  const show = (items) => list.replaceChildren(...items.map((x) => row(x.user, x.group)));
  show(all);
  if (groups.length < 2) {
    body.replaceChildren(list);
    return;
  }
  // One tab for everything and one for each reaction, with how many members chose it.
  const tabs = el("div", { class: "bf-reactions-tabs", role: "tablist", "aria-label": tr("Reactions") });
  const make = (label, count, items, selected) => {
    const tab = el("button", {
      type: "button",
      class: "bf-reactions-tab",
      role: "tab",
      "aria-selected": String(selected),
    });
    tab.tabIndex = selected ? 0 : -1;
    tab.append(el("span", {}, label), el("span", { class: "bf-reactions-count" }, String(count)));
    tab.addEventListener("click", () => {
      for (const t of tabs.children) {
        t.setAttribute("aria-selected", String(t === tab));
        /** @type {HTMLElement} */ (t).tabIndex = t === tab ? 0 : -1;
      }
      show(items);
    });
    return tab;
  };
  tabs.append(make(tr("All"), all.length, all, true));
  for (const g of groups) {
    const tab = make(
      g.icon,
      g.users.length,
      g.users.map((u) => ({ user: u, group: g })),
      false,
    );
    tab.setAttribute("aria-label", `${g.name}: ${g.users.length}`);
    tabs.append(tab);
  }
  // Left and Right move along the tabs, as in any tab list.
  tabs.addEventListener("keydown", (e) => {
    if (e.key !== "ArrowRight" && e.key !== "ArrowLeft") return;
    const all = [...tabs.children];
    const at = all.indexOf(/** @type {Element} */ (document.activeElement));
    const next = all[(at + (e.key === "ArrowRight" ? 1 : -1) + all.length) % all.length];
    if (!(next instanceof HTMLElement)) return;
    e.preventDefault();
    next.focus();
    next.click();
  });
  body.replaceChildren(tabs, list);
}

/**
 * Opens the dialog for one post.
 * @param {string} postId
 * @param {Element | null} opener what gets focus back when it closes
 */
export async function openReactions(postId, opener) {
  const body = el("div", { class: "bf-reactions-body" });
  body.append(el("p", { class: "muted" }, tr("Loading…")));
  const d = dialog({ title: tr("Reactions"), className: "bf-reactions is-narrow", body: [body] });
  d.show(opener ?? undefined);
  try {
    const res = await fetch(`/api/v1/posts/${encodeURIComponent(postId)}/reactions`, {
      headers: { accept: "application/json" },
    });
    if (!res.ok) throw new Error(String(res.status));
    render(body, (await res.json()).groups ?? []);
  } catch {
    body.replaceChildren(el("p", { class: "muted" }, tr("This could not be loaded.")));
  }
}
