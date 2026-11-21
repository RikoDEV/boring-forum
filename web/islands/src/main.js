// Guest islands (counted against the guest JS budget). Plain JS on purpose: the
// Svelte runtime alone is most of the budget, and these are small. Member-only islands
// (composer, passkeys) are Svelte and live in members.js.
//
// The page's inline loader calls mount(name, el) for each [data-island] element; props
// come from data-props JSON. Returning false hands the element to members.js.
import { groupColours } from "./ink.js";
import {
  announce,
  csrf,
  formParams,
  jsonHeaders,
  icon,
  toast,
  tn,
  tr,
  when,
  fullDate,
  timeText,
  LOCALE,
} from "./dom.js";
// The one modal dialog; importing it also opens the server-rendered ones (`data-dialog`).
import { dialog } from "./dialog.js";
// The hover bubble for [data-tip] and icon buttons (one element in the top layer).
import { tooltips } from "./tooltip.js";
tooltips();

// Larger features load as separate chunks, only on pages that use them, so the bundle every
// guest downloads stays within the budget.
const topic = () => import("./topic.js");
const liveUpdates = () => import("./live.js");
const tags = () => import("./tags.js");
const media = () => import("./media.js");
const lazy = (load, name) => (t, p) => load().then((m) => m[name](t, p));

const el = (tag, attrs = {}, text) => {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, v);
  if (text !== undefined) e.textContent = text;
  return e;
};

// Theme: system → light → dark, stored per browser, applied before paint by layout.html.
function themeToggle(target) {
  const order = ["system", "light", "dark"];
  const icon = { system: "◐", light: "☀", dark: "☾" };
  const read = () => {
    try {
      return localStorage.getItem("bf-theme") || "system";
    } catch {
      return "system";
    }
  };
  let theme = read();
  const b = el("button", { type: "button", class: "bf-icon-btn" });
  const paint = () => {
    b.textContent = icon[theme];
    const name = { system: tr("System"), light: tr("Light"), dark: tr("Dark") }[theme] ?? theme;
    b.setAttribute("aria-label", tr("Theme: {name}", { name }));
  };
  b.addEventListener("click", () => {
    theme = order[(order.indexOf(theme) + 1) % order.length];
    try {
      if (theme === "system") localStorage.removeItem("bf-theme");
      else localStorage.setItem("bf-theme", theme);
    } catch {
      // Storage blocked: the change still applies to this page view.
    }
    if (theme === "system") delete document.documentElement.dataset.theme;
    else document.documentElement.dataset.theme = theme;
    // The browser's own chrome (canvas, scrollbars, form controls) follows the meta tag too.
    document
      .querySelector('meta[name="color-scheme"]')
      ?.setAttribute("content", theme === "system" ? "light dark" : theme);
    groupColours(); // backgrounds changed: text colours are measured again
    paint();
  });
  paint();
  target.append(b);
}

// One SSE connection for the whole header, shared by the notification, message and chat badges.
let userEvents = null;
function liveUserEvents() {
  if (typeof EventSource === "undefined") return null;
  return (userEvents ??= new EventSource("/api/v1/live?channels=user"));
}

/**
 * Shows a count in a header badge, and in the name of the button it sits on: a button named
 * only by its aria-label would otherwise hide the visible number from screen readers (and
 * WCAG 2.5.3, Label in Name, wants the visible text inside the name).
 * @param {HTMLElement} badge
 */
function badgePainter(badge) {
  const named = badge.closest("[aria-label]");
  const label = named?.getAttribute("aria-label") ?? "";
  return (/** @type {number} */ count) => {
    badge.textContent = String(count);
    badge.hidden = count === 0;
    named?.setAttribute("aria-label", count ? `${label} ${count}` : label);
  };
}

// Members' unread-notification badge, kept live over SSE; the tab title shows the count.
function notifyCount(target, { count = 0 }) {
  document.addEventListener("bf:notify-clear", () => {
    count = 0;
    paint();
  });
  const base = document.title.replace(/^\(\d+\) /, "");
  const show = badgePainter(target);
  const paint = () => {
    show(count);
    document.title = count ? `(${count}) ${base}` : base;
  };
  paint();
  const es = liveUserEvents();
  if (!es) return;
  es.addEventListener("notification", (e) => {
    // A private message is also a notification; it bumps the mail badge, not this one.
    const d = JSON.parse(e.data);
    if (d?.url?.startsWith("/messages/")) return;
    // A direct message in chat is counted by the chat badge.
    if (typeof d?.group === "string" && d.group.startsWith("chat:")) return;
    count += 1;
    paint();
    announce(tn("js.count.unread_notifications", count));
  });
}

// Unread private messages: the header's mail icon.
function messagesCount(target, { count = 0 }) {
  const show = badgePainter(target);
  const paint = () => show(count);
  paint();
  const es = liveUserEvents();
  if (!es) return;
  es.addEventListener("notification", (e) => {
    if (!JSON.parse(e.data)?.url?.startsWith("/messages/")) return;
    count += 1;
    paint();
    announce(tn("js.count.unread_messages", count));
  });
}

// Unread chat messages, across every channel the member has not muted.
function chatCount(target, { count = 0 }) {
  const show = badgePainter(target);
  const paint = () => show(count);
  paint();
  const es = liveUserEvents();
  if (!es) return;
  es.addEventListener("chat", () => {
    count += 1;
    paint();
    announce(tn("js.count.unread_messages", count));
  });
}

// Member cards on profile links: their own chunk, loaded once the page is idle.
function hovercards() {
  const start = () => import("./hovercard.js").then((m) => m.hovercards());
  if ("requestIdleCallback" in window) requestIdleCallback(start, { timeout: 2000 });
  else setTimeout(start, 500);
}

// A select marked `data-autosubmit` sends its form as soon as it changes (a list's sort order). Without
// the script the form's own button does the same.
document.addEventListener("change", (e) => {
  const select = e.target instanceof HTMLSelectElement && e.target.hasAttribute("data-autosubmit");
  if (select) e.target.form?.requestSubmit();
});

// Copy buttons on code blocks.
function codeCopy(root = document) {
  if (!navigator.clipboard) return;
  for (const pre of root.querySelectorAll(".cooked pre")) {
    const b = el("button", { type: "button", class: "btn copy" }, tr("Copy"));
    b.addEventListener("click", async () => {
      await navigator.clipboard.writeText(pre.querySelector("code")?.textContent ?? "");
      b.textContent = tr("Copied");
    });
    pre.prepend(b);
  }
}

// TeX in posts: the KaTeX chunk loads only on pages that have some.
function math(root = document) {
  if (root.querySelector?.(".cooked [data-math-style]")) import("./math.js").then((m) => m.typeset(root));
}

// Rows appended by infinite scroll get the same enhancements as the first page.
// Link click counts: shown next to links, counted by a beacon that never delays the click.
function linkCounts(target, { id }) {
  const post = (a) =>
    a.closest(".post")?.querySelector("[data-quote]")?.dataset.quote ?? a.closest(".post")?.dataset.post;
  document.addEventListener("click", (e) => {
    const a = e.target.closest?.(".cooked a[href^='http']");
    const p = a && post(a);
    if (p) navigator.sendBeacon?.("/api/v1/links/click", JSON.stringify({ post: +p, url: a.getAttribute("href") }));
  });
  fetch(`/api/v1/topics/${id}/links`)
    .then((r) => (r.ok ? r.json() : []))
    .then((rows) => {
      for (const a of document.querySelectorAll(".cooked a[href^='http']")) {
        const hit = rows.find((r) => r.url === a.getAttribute("href") && String(r.post) === post(a));
        if (hit) a.after(el("small", { class: "muted" }, ` ${hit.clicks}`));
      }
    })
    .catch(() => {});
}

// The system switching light and dark changes every background: measure group colours again.
matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => groupColours());

// Dates in the member's chosen format, in their own time zone.
function localTimes(root = document) {
  // Group colours go through CSSOM, which the strict CSP allows and style attributes are not.
  groupColours(root);
  // Every date shows the full date on hover, in the reader's own time zone (also a <time> the
  // page gave no title), and its words in the page's language: the server's are English.
  // Dates inside posts are the author's own words and keep them.
  for (const t of root.querySelectorAll("time[datetime]")) {
    const iso = t.getAttribute("datetime");
    const full = fullDate(iso);
    if (full) t.title = full;
    if (full && !t.closest(".cooked")) t.textContent = timeText(iso, t.dataset.format);
  }
  const mode = document.documentElement.dataset.date;
  if (!mode) return;
  // A date in a fixed format ("Oct 2026") keeps it.
  for (const t of root.querySelectorAll("time[datetime]:not([data-format])")) {
    const d = new Date(t.getAttribute("datetime"));
    if (Number.isNaN(+d)) continue;
    const pad = (n) => String(n).padStart(2, "0");
    t.textContent =
      mode === "iso"
        ? `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`
        : d.toLocaleString(LOCALE());
  }
}

// The only HTML this bundle inserts: posts already sanitized by the server's cooker.
const cookedPolicy = window.trustedTypes?.createPolicy("bf-cooked", { createHTML: (s) => s }) ?? {
  createHTML: (s) => s,
};

// "in reply to" links expand the quoted post in place; the link still works without JS.
function replyContext() {
  document.addEventListener("click", async (e) => {
    const a = /** @type {HTMLAnchorElement | null} */ (e.target.closest?.("[data-context]"));
    if (!a || e.ctrlKey || e.metaKey) return;
    e.preventDefault();
    const open = a.parentElement.nextElementSibling;
    if (open?.classList.contains("reply-context")) return open.remove();
    const res = await fetch(`/api/v1/topics/${a.dataset.topic}/posts/${a.dataset.context}`).catch(() => null);
    if (!res?.ok) {
      location.href = a.href;
      return;
    }
    const box = el("blockquote", { class: "reply-context cooked" });
    box.innerHTML = cookedPolicy.createHTML((await res.json()).html);
    a.parentElement.after(box);
  });
}

// "Back to where you were": remembers the last post seen per topic, for this tab.
function backToPlace({ id }) {
  const KEY = `bf-place:${id}`;
  let saved;
  try {
    saved = sessionStorage.getItem(KEY);
  } catch {
    return;
  }
  const number = () => {
    let n = null;
    for (const p of document.querySelectorAll('#posts > [id^="post-"]')) {
      if (p.getBoundingClientRect().top > innerHeight / 3) break;
      n = p.id.slice(5);
    }
    return n;
  };
  if (saved && !location.hash && +saved > 1) {
    const b = el(
      "a",
      { href: `/t/x/${id}/${saved}`, class: "btn back-place" },
      tr("Back to where you were (#{n})", { n: saved }),
    );
    document.querySelector("#posts")?.before(b);
  }
  addEventListener("pagehide", () => {
    const n = number();
    if (n)
      try {
        sessionStorage.setItem(KEY, n);
      } catch {}
  });
}

// Attachment cards show how often they were downloaded.
async function fileCards() {
  const links = [...document.querySelectorAll('.cooked a[href^="/uploads/"]')].filter((a) => !a.querySelector("img"));
  if (!links.length) return;
  const key = (a) => a.getAttribute("href").split("/").pop();
  const res = await fetch(`/api/v1/uploads/counts?keys=${links.map(key).join(",")}`).catch(() => null);
  if (!res?.ok) return;
  const rows = await res.json();
  for (const a of links) {
    const r = rows.find((x) => x.key === key(a));
    if (r) a.append(el("small", { class: "muted" }, ` · ${tn("count.downloads", r.downloads)}`));
  }
}

// Quotes of posts deleted since keep no text from them.
async function deletedQuotes() {
  const links = [...document.querySelectorAll('.cooked blockquote a[href*="#post-"]')];
  const ref = (a) => {
    const m = a.getAttribute("href").match(/\/(?:t\/[^/#]+|topic)\/(\d+)[^#]*#post-(\d+)/);
    return m && `${m[1]}:${m[2]}`;
  };
  const refs = [...new Set(links.map(ref).filter(Boolean))];
  if (!refs.length) return;
  const res = await fetch(`/api/v1/quotes/deleted?refs=${refs.join(",")}`).catch(() => null);
  if (!res?.ok) return;
  const gone = await res.json();
  for (const a of links) {
    if (gone.includes(ref(a))) a.closest("blockquote").replaceChildren(el("em", { class: "muted" }, "[deleted]"));
  }
}

function cookieConsent(target) {
  let seen = false;
  try {
    seen = localStorage.getItem("bf-cookies-ok") === "1";
  } catch {
    // Storage blocked: the banner shows each time.
  }
  if (seen) return;
  const b = el("button", { type: "button", class: "btn" }, "OK");
  b.addEventListener("click", () => {
    try {
      localStorage.setItem("bf-cookies-ok", "1");
    } catch {}
    document.dispatchEvent(new CustomEvent("bf:consent"));
    target.hidden = true;
  });
  target.append(" ", b);
  target.hidden = false;
}

const cookedDecorators = [];
const postButtons = [];
const decorateCookedIn = (root) => {
  for (const [selector, fn] of cookedDecorators) {
    for (const e of root.querySelectorAll?.(`.cooked ${selector}`) ?? []) guarded(fn, e);
  }
};
const addPostButtonsIn = (root) => {
  for (const { label, onClick } of postButtons) {
    for (const footer of root.querySelectorAll?.(".post[data-post] .post-actions") ?? [])
      addPostButton(footer, label, onClick);
  }
};
// A plugin's callback failing must not take the rest of the page with it.
function guarded(fn, ...args) {
  try {
    return fn(...args);
  } catch (e) {
    console.warn("plugin failed", e);
  }
}
function addPostButton(footer, label, onClick) {
  const b = el("button", { type: "button", class: "link" }, label);
  const post = footer.closest("[data-post]");
  b.addEventListener("click", () => guarded(onClick, { id: +post.dataset.post, element: post }));
  footer.append(" ", b);
}

document.addEventListener("bf:post-replaced", (e) => onAppend([e.detail]));
const onAppend = (added) =>
  added.forEach((n) => {
    linkPreviews(n);
    codeCopy(n);
    math(n);
    localTimes(n);
    decorateCookedIn(n);
    addPostButtonsIn(n);
    document.dispatchEvent(new CustomEvent("bf:posts-added", { detail: { element: n } }));
  });

// A dismissed announcement stays dismissed in this browser.
function announcement(target, { id }) {
  const key = `bf-announcement-${id}`;
  try {
    if (localStorage.getItem(key)) {
      target.hidden = true;
      return;
    }
  } catch {}
  // A cross in the top corner.
  const b = el("button", { type: "button", class: "bf-icon-btn announcement-close", "aria-label": tr("Dismiss") });
  b.append(icon("x"));
  b.addEventListener("click", () => {
    try {
      localStorage.setItem(key, "1");
    } catch {}
    target.hidden = true;
  });
  target.append(b);
}

// Staff: one composer button per saved reply.
async function savedReplies() {
  const res = await fetch("/api/v1/saved-replies");
  if (!res.ok) return;
  for (const r of await res.json()) sdk.addComposerToolbarButton(r.title, ({ insert }) => insert(r.body));
}

// "Someone is typing" in a conversation: tells others at most every 3 s, and shows theirs.
function pmTyping(target, { id }) {
  const box = document.getElementById("body");
  let last = 0;
  box?.addEventListener("input", () => {
    if (Date.now() - last < 3000) return;
    last = Date.now();
    fetch(`/messages/${id}/typing`, {
      method: "POST",
      headers: { "Content-Type": "application/x-www-form-urlencoded" },
      body: `csrf=${encodeURIComponent(csrf())}`,
    }).catch(() => {});
  });
  let timer;
  const source = new EventSource(`/api/v1/live?channels=conversation:${id}`);
  source.addEventListener("typing", (e) => {
    target.textContent = tr("{name} is typing…", { name: JSON.parse(e.data).by });
    clearTimeout(timer);
    timer = setTimeout(() => (target.textContent = ""), 4000);
  });
  // A reply from someone else: fetch the page again and show the messages that are new.
  source.addEventListener("message", async () => {
    const res = await fetch(location.pathname).catch(() => null);
    if (!res?.ok) return;
    const doc = new DOMParser().parseFromString(await res.text(), "text/html");
    const shown = document.querySelectorAll("article.bf-msg");
    const fresh = [...doc.querySelectorAll("article.bf-msg")].slice(shown.length);
    if (!fresh.length) return;
    shown[shown.length - 1]?.after(...fresh.map((n) => document.adoptNode(n)));
    target.textContent = "";
    announce(tr("New message"));
  });
  // The page is going away (or entering the back/forward cache): free the connection.
  window.addEventListener("pagehide", () => {
    clearTimeout(timer);
    source.close();
  });
}

// Turns browser notifications on or off for this device, from a button in settings.
async function pushToggle(target) {
  if (!("PushManager" in window) || !("serviceWorker" in navigator)) return;
  const keyRes = await fetch("/api/v1/push/key");
  if (!keyRes.ok) return;
  const { key } = await keyRes.json();
  const reg = await navigator.serviceWorker.ready.catch(() => null);
  if (!reg) return;
  const b = el("button", { type: "button", class: "btn" });
  const note = el("span", { class: "muted small", role: "status" });
  const paint = (on) => {
    b.textContent = on ? tr("Turn off browser notifications") : tr("Turn on browser notifications");
    b.dataset.on = on ? "1" : "";
    note.textContent = on ? tr("Browser notifications are on for this device.") : "";
  };
  paint(Boolean(await reg.pushManager.getSubscription().catch(() => null)));
  b.addEventListener("click", async () => {
    b.disabled = true;
    try {
      if (b.dataset.on) {
        const sub = await reg.pushManager.getSubscription();
        if (sub) {
          await fetch("/api/v1/push/subscribe", {
            method: "DELETE",
            headers: jsonHeaders(),
            body: JSON.stringify({ endpoint: sub.endpoint }),
          });
          await sub.unsubscribe();
        }
        paint(false);
      } else {
        const raw = Uint8Array.from(atob(key.replace(/-/g, "+").replace(/_/g, "/")), (c) => c.charCodeAt(0));
        const sub = await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: raw });
        const j = sub.toJSON();
        const res = await fetch("/api/v1/push/subscribe", {
          method: "POST",
          headers: jsonHeaders(),
          body: JSON.stringify({ endpoint: j.endpoint, keys: j.keys }),
        });
        if (res.ok) paint(true);
        else note.textContent = tr("Could not turn them on");
      }
    } catch {
      // Denied, or the push service refused.
      note.textContent = tr("Could not turn them on. Check this site's notification permission.");
    }
    b.disabled = false;
  });
  target.append(b, " ", note);
  target.hidden = false;
}

// Closes open <details> menus on an outside click or Esc; they work without this too.
// Header dropdowns: filled from the API each time one opens. Without JS the panel still has
// its link to the full page.
function dropdowns() {
  const row = (n) => {
    const li = el("li", n.read ? {} : { class: "is-unread" });
    const a = el("a", { href: n.url });
    if (n.avatar)
      a.append(
        el("img", {
          class: "avatar",
          src: `/avatar/${encodeURIComponent(n.avatar)}.svg`,
          width: 32,
          height: 32,
          alt: "",
        }),
      );
    else a.append(icon(n.icon ?? "bell"));
    const body = el("div");
    if (n.title) body.append(el("strong", {}, n.title));
    const at = el("small", { class: "muted" });
    at.append(when(n.when, n.when_at));
    body.append(el("span", { class: "bf-drop-text" }, n.text), at);
    a.append(body);
    li.append(a);
    return li;
  };
  for (const d of document.querySelectorAll("details[data-drop]")) {
    const list = d.querySelector("[data-drop-list]");
    d.addEventListener("toggle", async () => {
      if (!d.open) return;
      if (!list.children.length) list.append(el("li", { class: "muted bf-drop-empty" }, tr("Loading…")));
      const res = await fetch(d.dataset.drop).catch(() => null);
      const items = res?.ok ? await res.json() : null;
      if (!items) list.replaceChildren(el("li", { class: "muted bf-drop-empty" }, tr("Could not load this list.")));
      else if (!items.length) list.replaceChildren(el("li", { class: "muted bf-drop-empty" }, tr("Nothing here yet.")));
      else list.replaceChildren(...items.map(row));
    });
    d.querySelector("[data-drop-read]")?.addEventListener("submit", markAllRead(list));
  }
  userMenu();
}

// "Mark all as read" in a header menu: posts the form, then clears the unread marks.
const markAllRead = (list) => async (e) => {
  e.preventDefault();
  const form = /** @type {HTMLFormElement} */ (e.target);
  const res = await fetch(form.action, { method: "POST", body: formParams(form) }).catch(() => null);
  if (!res?.ok) return form.submit();
  for (const li of list.querySelectorAll(".is-unread")) li.classList.remove("is-unread");
  document.dispatchEvent(new Event("bf:notify-clear"));
};

// The avatar menu: notifications by tab, with the account links on the last tab.
function userMenu() {
  const menu = document.querySelector("details[data-user-menu]");
  if (!menu) return;
  const tabs = [...menu.querySelectorAll("[data-um-tab]")];
  const items = menu.querySelector("[data-um-items]");
  const list = menu.querySelector("[data-um-list]");
  const profile = menu.querySelector("[data-um-profile]");
  const all = /** @type {HTMLAnchorElement} */ (menu.querySelector("[data-um-all]"));
  const panel = menu.querySelector("#bf-um-panel");
  const t = { empty: menu.dataset.empty, failed: menu.dataset.failed };
  const avatar = (name) =>
    el("img", { class: "avatar", src: `/avatar/${encodeURIComponent(name)}.svg`, width: 36, height: 36, alt: "" });
  const row = (n, fallback) => {
    const li = el("li", n.read === false ? { class: "is-unread" } : {});
    const a = el("a", { href: n.url, title: [n.text, fullDate(n.when_at) || n.when].filter(Boolean).join(" · ") });
    const pic = el("span", { class: "bf-um-pic" }, "");
    pic.append(avatar(n.by || n.avatar || menu.dataset.me));
    const badge = el("span", { class: "bf-um-kind" }, "");
    badge.append(icon(n.icon ?? fallback));
    pic.append(badge);
    const body = el("div", { class: "bf-um-text" });
    const label = n.label ?? n.title;
    if (label) body.append(el("span", { class: "bf-um-label" }, label));
    body.append(el("span", { class: "bf-um-desc" }, n.description ?? n.text ?? ""));
    a.append(pic, body);
    li.append(a);
    return li;
  };
  let asked = 0;
  const select = async (tab, focus) => {
    for (const x of tabs) {
      const on = x === tab;
      x.classList.toggle("is-active", on);
      x.setAttribute("aria-selected", String(on));
      x.tabIndex = on ? 0 : -1;
    }
    panel.setAttribute("aria-labelledby", tab.id);
    if (focus) tab.focus();
    const url = tab.dataset.umApi;
    items.hidden = !url;
    profile.hidden = Boolean(url);
    if (!url) return;
    all.href = tab.getAttribute("href");
    const mine = ++asked;
    list.replaceChildren(el("li", { class: "muted bf-um-empty" }, "…"));
    const res = await fetch(url).catch(() => null);
    const rows = res?.ok ? await res.json().catch(() => null) : null;
    if (mine !== asked) return; // another tab was chosen meanwhile
    if (!rows) list.replaceChildren(el("li", { class: "muted bf-um-empty" }, t.failed));
    else if (!rows.length) list.replaceChildren(el("li", { class: "muted bf-um-empty" }, t.empty));
    else list.replaceChildren(...rows.map((n) => row(n, tab.dataset.umTab === "messages" ? "mail" : "bell")));
  };
  items.hidden = false;
  profile.hidden = true;
  tabs[0].tabIndex = 0;
  menu.addEventListener("toggle", () => {
    if (menu.open) select(tabs.find((x) => x.classList.contains("is-active")) ?? tabs[0]);
  });
  menu.querySelector("[role=tablist]").addEventListener("click", (e) => {
    const tab = e.target.closest("[data-um-tab]");
    // A modified click still opens the full page in a new tab.
    if (!tab || e.ctrlKey || e.metaKey || e.shiftKey || e.button) return;
    e.preventDefault();
    select(tab);
  });
  menu.querySelector("[role=tablist]").addEventListener("keydown", (e) => {
    const i = tabs.indexOf(/** @type {HTMLElement} */ (document.activeElement));
    if (i < 0) return;
    const next = { ArrowDown: i + 1, ArrowUp: i - 1, Home: 0, End: tabs.length - 1 }[e.key];
    if (next === undefined) return;
    e.preventDefault();
    select(tabs[(next + tabs.length) % tabs.length], true);
  });
  menu.querySelector("[data-um-read]").addEventListener("submit", markAllRead(list));
}

// The header's chat icon opens a drawer docked in the corner, which follows you between pages.
function chatDrawer() {
  const link = document.querySelector("[data-chat-link]");
  if (!link || location.pathname.startsWith("/chat")) return;
  const KEY = "bf-chat-drawer";
  const remember = (v) => {
    try {
      if (v === null) localStorage.removeItem(KEY);
      else localStorage.setItem(KEY, String(v));
    } catch {
      // Storage blocked: the drawer just won't follow to the next page.
    }
  };
  let box = null;
  let app = null;
  const close = () => {
    app?.close();
    box?.remove();
    box = app = null;
    remember(null);
  };
  const open = async (channel) => {
    box = el("div", { class: "chat-drawer", role: "complementary", "aria-label": tr("Chat") });
    document.body.append(box);
    const m = await import(/* @vite-ignore */ link.dataset.editor);
    // Pictures in messages open large, as they do in posts.
    topic().then((t) => t.lightbox());
    app = m.chatApp(box, {
      drawer: true,
      channel,
      me: link.dataset.me,
      staff: link.dataset.staff === "true",
      onClose: close,
      onChannel: remember,
    });
    remember(channel);
  };
  link.addEventListener("click", (e) => {
    // Phones and new-tab clicks get the full page.
    if (e.ctrlKey || e.metaKey || e.shiftKey || matchMedia("(max-width: 760px)").matches) return;
    e.preventDefault();
    if (box) close();
    else open(0);
  });
  try {
    const saved = localStorage.getItem(KEY);
    if (saved !== null && !matchMedia("(max-width: 760px)").matches) open(+saved);
  } catch {
    // As above.
  }
}

// Share: the device's own sheet where there is one, otherwise a menu of ours.
function shareLinks() {
  document.addEventListener("click", async (e) => {
    const a = e.target.closest?.("a.share");
    if (!a || e.ctrlKey || e.metaKey || e.shiftKey || e.button) return;
    e.preventDefault();
    const title = document.querySelector("main h1")?.textContent.trim() || document.title;
    if (navigator.share) {
      try {
        await navigator.share({ title, url: a.href });
        return;
      } catch (err) {
        if (err.name === "AbortError") return; // the person changed their mind
      }
    }
    shareMenu(a.href, title);
  });
}

function shareMenu(url, title) {
  const row = el("div", { class: "bf-share-link" });
  const input = el("input", { readonly: "", value: url, placeholder: " " });
  const linkField = el("label", { class: "bf-field" });
  linkField.append(input, el("span", { class: "bf-field-label" }, tr("Link")));
  const copy = el("button", { type: "button", class: "btn" }, tr("Copy"));
  copy.addEventListener("click", async () => {
    input.select();
    await navigator.clipboard?.writeText(url);
    copy.textContent = tr("Copied");
  });
  row.append(linkField, copy);
  const [u, t] = [encodeURIComponent(url), encodeURIComponent(title)];
  const list = el("div", { class: "bf-share-sites" });
  for (const [name, href] of [
    ["X", `https://x.com/intent/post?url=${u}&text=${t}`],
    ["Facebook", `https://www.facebook.com/sharer/sharer.php?u=${u}`],
    ["Reddit", `https://www.reddit.com/submit?url=${u}&title=${t}`],
    ["WhatsApp", `https://wa.me/?text=${t}%20${u}`],
    ["Telegram", `https://t.me/share/url?url=${u}&text=${t}`],
    [tr("Email"), `mailto:?subject=${t}&body=${u}`],
  ])
    list.append(el("a", { class: "btn", href, target: "_blank", rel: "noopener noreferrer" }, name));
  dialog({ title: tr("Share"), body: [row, list] }).show();
  input.select();
}

// A wiki topic's first post gets a table of contents built from its headings: in the rail on wide
// screens, folded above the text on narrow ones.
function tableOfContents() {
  const cooked = document.querySelector("#post-1[data-wiki] .cooked");
  if (!cooked) return;
  const heads = [...cooked.querySelectorAll("h1, h2, h3, h4, h5, h6")].filter((h) => h.textContent.trim());
  if (heads.length < 2) return;
  const top = Math.min(...heads.map((h) => +h.tagName[1]));
  const used = new Set();
  for (const h of heads) {
    if (h.id) continue;
    const base = `toc-${h.textContent
      .trim()
      .toLowerCase()
      .replace(/[^\p{L}\p{N}]+/gu, "-")
      .replace(/^-|-$/g, "")}`;
    let id = base;
    for (let n = 2; used.has(id) || document.getElementById(id); n++) id = `${base}-${n}`;
    used.add(id);
    h.id = id;
  }
  const links = new Map();
  const list = () => {
    const ul = el("ul", { class: "bf-toc-list" });
    for (const h of heads) {
      const a = el("a", { href: `#${h.id}` }, h.textContent.trim());
      const li = el("li", { class: `bf-toc-depth-${+h.tagName[1] - top}` });
      li.append(a);
      ul.append(li);
      links.set(h, [...(links.get(h) ?? []), a]);
    }
    return ul;
  };
  const rail = document.querySelector(".bf-rail");
  const nav = el("nav", { class: "bf-toc", "aria-label": tr("Contents") });
  nav.append(list());
  rail?.prepend(nav);
  const fold = el("details", { class: "bf-toc-fold" });
  fold.append(el("summary", {}, tr("Contents")), list());
  cooked.before(fold);
  // The current section is the last heading that has passed the top of the screen.
  const mark = () => {
    const limit = (document.querySelector(".bf-header-bar")?.offsetHeight ?? 0) + 24;
    let now = heads[0];
    for (const h of heads) if (h.getBoundingClientRect().top <= limit) now = h;
    for (const [h, as] of links) for (const a of as) a.classList.toggle("is-current", h === now);
  };
  mark();
  addEventListener("scroll", mark, { passive: true });
}

// The page's title moves into the sticky header once its heading has scrolled away.
function headerTitle() {
  const h1 = document.querySelector("main h1:not(.visually-hidden)");
  const brand = document.querySelector(".site-header .brand");
  // The admin panel changes pages without reloading, so a title taken once would go stale.
  if (location.pathname.startsWith("/admin")) return;
  if (!h1 || !brand || typeof IntersectionObserver === "undefined") return;
  const copy = /** @type {HTMLElement} */ (h1.cloneNode(true));
  for (const tag of copy.querySelectorAll(".tag, button, form")) tag.remove();
  const text = copy.textContent.trim();
  if (!text) return;
  // Where the topic lives (forum and parent categories, from the breadcrumb) sits above its title.
  const path = [...document.querySelectorAll("nav.crumbs a")].slice(1);
  const title = el("div", { class: "bf-header-title", hidden: "" });
  if (path.length) {
    title.append(
      el(
        "a",
        { class: "bf-ht-forum", href: path.at(-1).getAttribute("href") },
        path.map((a) => a.textContent.trim()).join(" › "),
      ),
    );
  }
  const name = el("button", { type: "button", class: "bf-ht-title", title: text }, text);
  name.addEventListener("click", () => scrollTo({ top: 0, behavior: "smooth" }));
  // An ideas topic keeps its vote box in the header, small, still clickable.
  const box = document.querySelector(".bf-topic-titlerow > .bf-vote-box");
  if (box) {
    const copy = /** @type {HTMLElement} */ (box.cloneNode(true));
    copy.classList.add("is-small");
    title.append(copy);
  }
  const words = el("div", { class: "bf-ht-text" });
  words.append(...title.querySelectorAll(".bf-ht-forum"), name);
  title.append(words);
  brand.after(title);
  // Shown once the heading has gone under the header. Read from the scroll position, not from an
  // observer's transitions, and with a margin before it hides again, so a change in the header's
  // own height cannot flip it back and forth.
  const update = () => {
    const bar = document.querySelector(".bf-header-bar")?.offsetHeight ?? 0;
    const bottom = h1.getBoundingClientRect().bottom;
    if (title.hidden && bottom <= bar) title.hidden = false;
    else if (!title.hidden && bottom > bar + 24) title.hidden = true;
  };
  let queued = false;
  addEventListener(
    "scroll",
    () => {
      if (queued) return;
      queued = true;
      requestAnimationFrame(() => {
        queued = false;
        update();
      });
    },
    { passive: true },
  );
  addEventListener("resize", update);
  update();
}

// A bare link alone on its line becomes a card, once it comes near the screen.
function linkPreviews(root = document) {
  const seen = new IntersectionObserver(
    (entries) => {
      for (const e of entries) {
        if (!e.isIntersecting) continue;
        seen.unobserve(e.target);
        card(e.target);
      }
    },
    { rootMargin: "300px" },
  );
  const card = async (a) => {
    const res = await fetch(`/api/v1/onebox?url=${encodeURIComponent(a.href)}`).catch(() => null);
    if (!res?.ok) return;
    const d = await res.json();
    const box = el("a", { class: "bf-onebox", href: a.href, rel: a.rel || "nofollow ugc noopener" });
    if (d.image) box.append(el("img", { src: d.image, alt: "", loading: "lazy", referrerpolicy: "no-referrer" }));
    const text = el("span", { class: "bf-onebox-text" });
    text.append(el("small", {}, d.site), el("strong", {}, d.title));
    if (d.description) text.append(el("span", {}, d.description));
    box.append(text);
    box.querySelector("img")?.addEventListener("error", (ev) => ev.target.remove());
    a.replaceWith(box);
  };
  for (const a of /** @type {NodeListOf<HTMLAnchorElement> | never[]} */ (
    root.querySelectorAll?.(".cooked > p > a:only-child") ?? []
  )) {
    if (a.closest(".composer-preview") || a.querySelector("img")) continue;
    if (
      a.parentElement.textContent.trim() === a.textContent.trim() &&
      a.textContent.trim() === a.href.replace(/\/$/, a.textContent.trim().endsWith("/") ? "/" : "")
    )
      seen.observe(a);
  }
}

// Moderators: a floating bar over ticked replies, with what can be done to them. On a phone it is
// a small pill of icon buttons; the title for a new topic only opens when "Move" is tapped.
function postSelection() {
  const boxes = () => [...document.querySelectorAll('input[form="split-form"][name="post"]')];
  if (!boxes().length) return;
  const narrow = matchMedia("(width <= 639px)");
  const bar = el("div", { class: "bf-select-bar", role: "region", "aria-label": tr("Selected replies"), hidden: "" });
  const count = el("strong");
  // An icon with its words: the words are hidden on a phone, but stay as the button's name.
  const tool = (cls, name, text) => {
    const b = el("button", { type: "button", class: `btn ${cls}`, "aria-label": text });
    b.append(icon(name), el("span", { class: "bf-select-label" }, text));
    return b;
  };
  const relabel = (b, text) => {
    b.setAttribute("aria-label", text);
    b.querySelector(".bf-select-label").textContent = text;
  };
  const all = tool("", "circle-check", tr("Select all"));
  const clear = tool("", "x", tr("Clear"));
  const title = el("input", { type: "text", placeholder: " " });
  const titleField = el("label", { class: "bf-field" });
  titleField.append(title, el("span", { class: "bf-field-label" }, tr("New topic title")));
  const move = tool("primary", "corner-down-right", tr("Move to new topic"));
  const remove = tool("bf-btn-danger", "trash-2", tr("Delete"));
  const head = el("div", { class: "bf-select-head" });
  head.append(count, all, clear);
  const actions = el("div", { class: "bf-select-actions" });
  actions.append(move, remove);
  bar.append(head, actions, titleField);
  document.body.append(bar);
  const selected = () => boxes().filter((b) => b.checked);
  let confirming = false;
  const reset = () => {
    confirming = false;
    bar.classList.remove("is-confirm");
    relabel(remove, tr("Delete"));
  };
  const paint = () => {
    const n = selected().length;
    count.textContent = tr("{n} selected", { n });
    bar.hidden = n === 0;
    if (!n) {
      reset();
      bar.classList.remove("is-moving");
    }
    for (const b of boxes()) b.closest(".post")?.classList.toggle("is-selected", b.checked);
  };
  document.addEventListener("change", (e) => {
    if (e.target.matches?.('input[form="split-form"]')) paint();
  });
  document.addEventListener("click", (e) => {
    const below = e.target.closest?.("[data-select-below]");
    if (!below) return;
    const list = boxes();
    const from = list.indexOf(below.closest(".post").querySelector('input[form="split-form"]'));
    list.forEach((b, i) => {
      if (i >= from) b.checked = true;
    });
    paint();
  });
  for (const [btn, on] of [
    [all, true],
    [clear, false],
  ])
    btn.addEventListener("click", () => {
      for (const b of boxes()) b.checked = on;
      paint();
    });
  move.addEventListener("click", () => {
    // A phone shows the title only now; the first tap opens it.
    if (narrow.matches && !bar.classList.contains("is-moving")) {
      reset();
      bar.classList.add("is-moving");
      return title.focus();
    }
    const name = title.value.trim();
    if (!name) return title.focus();
    const form = /** @type {HTMLFormElement} */ (document.getElementById("split-form"));
    form.querySelector('input[name="title"]').value = name;
    form.submit();
  });
  remove.addEventListener("click", async () => {
    const list = selected();
    // Two steps, like the editor's discard button.
    if (!confirming) {
      confirming = true;
      bar.classList.remove("is-moving");
      bar.classList.add("is-confirm");
      relabel(remove, tr("Delete {n}? Click again", { n: list.length }));
      return;
    }
    remove.disabled = true;
    for (const b of list) {
      const res = await fetch(`/post/${b.value}/delete`, {
        method: "POST",
        body: new URLSearchParams({ csrf: csrf() }),
      }).catch(() => null);
      if (!res?.ok) {
        toast(tr("A reply could not be deleted."), true);
        break;
      }
    }
    location.reload();
  });
}

// A green dot on the avatars of members who are online.
function presence() {
  const nameOf = (img) => {
    const m = img.getAttribute("src")?.match(/^\/avatar\/(.+)\.svg$/);
    return m ? decodeURIComponent(m[1]) : null;
  };
  const refresh = async () => {
    const imgs = [...document.querySelectorAll("img.avatar")].filter((i) => !i.closest(".bf-crop"));
    const names = [...new Set(imgs.map(nameOf).filter(Boolean))].slice(0, 100);
    if (!names.length) return;
    const res = await fetch(`/api/v1/online?names=${encodeURIComponent(names.join(","))}`).catch(() => null);
    if (!res?.ok) return;
    const online = new Set(await res.json());
    for (const img of imgs) {
      // The profile picture sits in a positioned box of its own; others get a small wrapper.
      const box = img.closest(".bf-avatar-wrap") ?? img.parentElement;
      const on = online.has(nameOf(img));
      // A trust level badge on the avatar shows presence itself, instead of a dot.
      const lvl = box.querySelector(":scope > .bf-lvl");
      if (lvl) {
        lvl.dataset.online = on ? "1" : "0";
        lvl.dataset.tipBase ??= lvl.dataset.tip ?? "";
        lvl.dataset.tip = `${lvl.dataset.tipBase} · ${on ? tr("Online") : tr("Offline")}`;
        continue;
      }
      let dot = box.querySelector(":scope > .bf-online");
      if (on && !dot) {
        let host = box;
        if (!img.closest(".bf-avatar-wrap") && !box.classList.contains("bf-presence")) {
          host = el("span", { class: "bf-presence" });
          img.replaceWith(host);
          host.append(img);
        }
        dot = el("span", { class: "bf-online", role: "img", "aria-label": tr("Online") });
        dot.style.setProperty("--dot", `${Math.max(8, Math.round((+img.getAttribute("width") || 32) * 0.2))}px`);
        host.append(dot);
      } else if (!on) {
        dot?.remove();
      }
    }
  };
  refresh();
  setInterval(() => document.visibilityState === "visible" && refresh(), 60000);
}

function menus() {
  dropdowns();
  presence();
  postSelection();
  linkPreviews();
  document.querySelector("[data-copy-link]")?.addEventListener("click", async () => {
    await navigator.clipboard?.writeText(location.href.split("#")[0]);
    toast(tr("Link copied"));
  });
  // Copies a fixed string (data-copy), or every item of a list it points to (data-copy-target).
  document.addEventListener("click", async (e) => {
    const b = e.target.closest?.("[data-copy], [data-copy-target]");
    if (!b) return;
    const text =
      b.dataset.copy ??
      [...document.querySelectorAll(`#${CSS.escape(b.dataset.copyTarget)} li`)]
        .map((li) => li.textContent.trim())
        .join("\n");
    await navigator.clipboard?.writeText(text);
    toast(tr("Copied"));
  });
  chatDrawer();
  shareLinks();
  headerTitle();
  tableOfContents();
  if (document.querySelector("input[data-tag-input], input[data-people-input]")) tags().then((m) => m.tagInputs());
  if (
    document.querySelector(
      "img.cover[data-y], form.bf-avatar-edit, form.bf-cover-upload, .bf-cover-wrap, form.bf-avatar-remove",
    )
  ) {
    media().then((m) => m.profileMedia());
  }
  // Links that do not fit the wide nav row move into a "More" menu.
  const nav = document.querySelector(".main-nav");
  if (nav && !matchMedia("(max-width: 640px)").matches && nav.scrollWidth > nav.clientWidth) {
    const body = el("div", { class: "bf-menu-body" });
    const more = el("details", { class: "bf-menu" });
    const label = el("summary", {});
    label.textContent = tr("More");
    more.append(label, body);
    nav.append(more);
    const links = () => nav.querySelectorAll(":scope > a");
    while (nav.scrollWidth > nav.clientWidth && links().length > 1) body.prepend(links()[links().length - 1]);
    nav.classList.add("has-more");
  }
  // Menus close on a click outside them. (Modals are <dialog class="bf-dialog">: dialog.js.)
  const open = () => document.querySelectorAll("details.bf-menu[open]");
  document.addEventListener("click", (e) => {
    for (const d of open()) {
      if (!d.contains(/** @type {Node} */ (e.target))) d.open = false;
    }
  });
  document.addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return;
    for (const d of open()) {
      d.open = false;
      d.querySelector("summary")?.focus();
    }
  });
}

// Self-hosted registration check: finds the number behind a signed hash, then sends it with the form.
function altcha(target) {
  const box = el("input", { type: "checkbox", required: "" });
  const text = el("span", {}, tr("I am not a robot"));
  const label = el("label", { class: "check bf-altcha" });
  label.append(box, text);
  const field = el("input", { type: "hidden", name: "altcha" });
  target.append(label, field);
  const hash = async (s) =>
    [...new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(s)))]
      .map((b) => b.toString(16).padStart(2, "0"))
      .join("");
  const solve = async () => {
    const c = await (await fetch("/api/v1/captcha/altcha")).json();
    for (let n = 0; n <= c.maxnumber; n++) {
      if ((await hash(c.salt + n)) === c.challenge)
        return btoa(
          JSON.stringify({
            algorithm: c.algorithm,
            challenge: c.challenge,
            number: n,
            salt: c.salt,
            signature: c.signature,
          }),
        );
    }
    throw new Error("unsolved");
  };
  box.addEventListener("click", async (e) => {
    if (field.value) return e.preventDefault();
    e.preventDefault();
    box.disabled = true;
    text.textContent = tr("Checking…");
    try {
      field.value = await solve();
      box.checked = true;
      text.textContent = tr("Verified");
    } catch {
      text.textContent = tr("That did not work. Try again.");
    }
    box.disabled = false;
  });
}

// The header's real height, for what must fill the screen below it (chat). The stylesheet's guess
// is wrong whenever a logo, a font or a wrapped row changes it.
function headerSize() {
  const bar = document.querySelector(".bf-header-bar");
  if (!bar || !("ResizeObserver" in window)) return;
  new ResizeObserver(() => {
    document.documentElement.style.setProperty("--bf-header-height", `${bar.getBoundingClientRect().height}px`);
  }).observe(bar);
}

const islands = {
  "header-size": headerSize,
  menus: () => menus(),
  altcha,
  announcement,
  "push-toggle": pushToggle,
  "pm-typing": pmTyping,
  "saved-replies": () => savedReplies(),
  infinite: (t, p) => topic().then((m) => m.infinite(t, { ...p, onAppend })),
  "composer-resume": lazy(topic, "composerResume"),
  "link-counts": (t, p) => {
    linkCounts(t, p);
    replyContext();
    backToPlace(p);
    fileCards();
    deletedQuotes();
  },
  "local-times": () => localTimes(),
  // The page that asks for a click before an outside login: when the site sends guests straight
  // there (`auth_immediately`), the page presses its own button.
  autosubmit: (form) => form.requestSubmit(),
  "edit-layout": editLayout,
  "cookie-consent": cookieConsent,
  lightbox: lazy(topic, "lightbox"),
  scrubber: lazy(topic, "scrubber"),
  "code-copy": () => codeCopy(),
  analytics,
  hovercards: () => hovercards(),
  "theme-toggle": themeToggle,
  live: (t, p) => liveUpdates().then((m) => m.live(t, { ...p, onAppend })),
  "terms-gate": lazy(() => import("./terms.js"), "termsGate"),
  "live-list": (t, p) => liveUpdates().then((m) => m.liveList(t, { ...p, onAppend })),
  "live-search": lazy(() => import("./search.js"), "liveSearch"),
  "ai-topic": lazy(() => import("./ai.js"), "aiTopic"),
  "ai-similar": () =>
    import("./ai.js").then(
      (m) =>
        document.getElementById("title") &&
        m.similarTopics(/** @type {HTMLInputElement} */ (document.getElementById("title"))),
    ),
  shortcuts: lazy(() => import("./shortcuts.js"), "shortcuts"),
  "notify-count": notifyCount,
  "messages-count": messagesCount,
  "chat-count": chatCount,
  autosave,
  chat: (t, p) => import(/* @vite-ignore */ p.editorUrl).then((m) => m.chatApp(t, p)),
};

// Submits a form as soon as one of its options changes. The save reloads the page, so focus is
// put back on the option, or keyboard users would start again from the top on every arrow key.
function autosave(form) {
  const key = `bf-autosave:${form.getAttribute("action")}`;
  try {
    const name = sessionStorage.getItem(key);
    sessionStorage.removeItem(key);
    if (name) form.querySelector(`input[name="${CSS.escape(name)}"]:checked`)?.focus();
  } catch {
    // Storage can be blocked; the save still works, focus just is not restored.
  }
  form.addEventListener("change", (e) => {
    try {
      sessionStorage.setItem(key, e.target.name);
    } catch {
      // As above.
    }
    form.submit();
  });
}

math();

// Analytics: `bfTrack(name, props)` sends an event to whichever provider the site set up;
// elements with `data-track` send one when clicked or submitted.
window.bfTrack = (name, props = {}) => {
  try {
    if (window.plausible) window.plausible(name, { props });
    else if (window.umami) window.umami.track(name, props);
    else if (window.gtag) window.gtag("event", name, props);
    else if (window._paq) window._paq.push(["trackEvent", "forum", name]);
  } catch {
    // A blocked or failing tracker never breaks the page.
  }
};
document.addEventListener("submit", (e) => {
  const name = e.target.closest?.("form[data-track]")?.dataset.track;
  if (name) window.bfTrack(name);
});
document.addEventListener("click", (e) => {
  const t = e.target.closest?.("a[data-track], button[data-track]:not([type=submit])");
  if (t) window.bfTrack(t.dataset.track);
});

// GA4 and Matomo set cookies: they load only once the visitor accepts them, when the site asks.
function analytics(_target, { provider, id, origin }) {
  const asked = !!document.querySelector('[data-island="cookie-consent"]');
  const allowed = () => {
    if (!asked) return true;
    try {
      return localStorage.getItem("bf-cookies-ok") === "1";
    } catch {
      return false;
    }
  };
  const script = (src) => {
    const s = document.createElement("script");
    s.async = true;
    s.src = src;
    document.head.append(s);
  };
  const load = () => {
    if (provider === "ga4") {
      window.dataLayer = window.dataLayer || [];
      window.gtag = function () {
        window.dataLayer.push(arguments);
      };
      window.gtag("js", new Date());
      window.gtag("config", id);
      script(`https://www.googletagmanager.com/gtag/js?id=${encodeURIComponent(id)}`);
    } else if (provider === "matomo") {
      const paq = (window._paq = window._paq || []);
      paq.push(["setTrackerUrl", `${origin}/matomo.php`], ["setSiteId", id], ["trackPageView"], ["enableLinkTracking"]);
      script(`${origin}/matomo.js`);
    }
  };
  if (allowed()) load();
  else document.addEventListener("bf:consent", load, { once: true });
}

// Forms that ask first (`data-confirm`) and print buttons.
document.addEventListener("submit", (e) => {
  const message = e.target.closest?.("form[data-confirm]")?.dataset.confirm;
  if (message && !confirm(message)) e.preventDefault();
});
document.addEventListener("click", (e) => {
  if (e.target.closest?.("[data-print]")) print();
  // Error pages: back to the form, with what was typed, when there is a page to go back to.
  if (e.target.closest?.("a[data-back]") && history.length > 1) {
    e.preventDefault();
    history.back();
  }
});

// Ideas forums: a vote changes the count where it is, on the topic and in the list, with the
// votes the member has left and the topic's voters. Without scripts the form posts and returns.
document.addEventListener("submit", async (e) => {
  const form = e.target.closest?.("form[data-vote]");
  if (!form || e.defaultPrevented) return;
  e.preventDefault();
  const button = /** @type {HTMLButtonElement} */ (form.querySelector("button"));
  button.disabled = true;
  const res = await fetch(form.action, {
    method: "POST",
    headers: { Accept: "application/json" },
    body: formParams(form),
  }).catch(() => null);
  button.disabled = false;
  if (!res?.ok) {
    const problem = await res?.json().catch(() => null);
    toast(problem?.detail ?? tr("Your vote could not be saved."), true);
    return;
  }
  const { votes, voted, left, limit } = await res.json();
  // The topic page's vote box and the sticky header's copy of it, or the one pill in a list.
  const own = /** @type {HTMLElement | null} */ (form.closest("[data-vote-group]"));
  const groups = own?.dataset.topic
    ? [...document.querySelectorAll(`[data-vote-group][data-topic="${CSS.escape(own.dataset.topic)}"]`)]
    : [own ?? form];
  for (const group of groups) {
    for (const c of group.querySelectorAll("[data-vote-count]")) c.textContent = votes;
    for (const input of group.querySelectorAll('input[name="on"]')) input.value = voted ? "0" : "1";
    group.classList.toggle("is-voted", voted);
    for (const b of group.querySelectorAll("button")) {
      b.setAttribute("aria-pressed", String(voted));
      b.classList.toggle("is-voted", voted);
      b.setAttribute("aria-label", voted ? tr("Take your vote back") : tr("Vote"));
    }
  }
  if (left != null) {
    for (const n of document.querySelectorAll("[data-votes-left]"))
      n.textContent = n.textContent.replace(/^\d+ of \d+/, `${left} of ${limit}`);
  }
  // The topic's voters: the member's own picture comes and goes with their vote.
  const voters = document.querySelector(".bf-topic-votes .bf-voters");
  const me = document.querySelector("[data-user-menu]")?.dataset.me;
  const list = voters?.querySelector(".bf-voter-list");
  if (list && me) {
    const mine = list.querySelector(`[data-voter="${CSS.escape(me)}"]`);
    if (voted && !mine) {
      const a = el("a", { href: `/u/${me.toLowerCase()}`, title: me, "data-voter": me });
      a.append(
        el("img", { class: "avatar", src: `/avatar/${encodeURIComponent(me)}.svg`, width: 24, height: 24, alt: me }),
      );
      list.prepend(a);
    } else if (!voted) mine?.remove();
    voters.hidden = votes === 0;
  }
  announce(`${voted ? tr("Voted") : tr("Vote taken back")}. ${tn("count.votes", votes)}.`);
});

// Administrators: the pencil beside a forum or category edits it in place (quick-edit.js).
document.addEventListener("click", (e) => {
  const pencil = e.target.closest?.("[data-node-edit]");
  if (!pencil) return;
  e.preventDefault();
  import("./quick-edit.js").then((m) => m.quickEdit(pencil.dataset.nodeEdit, pencil));
});

// Plugin SDK: a plugin module runs `(window.bfPlugins ??= []).push((api) => { ... })`,
// which works whether it loads before or after this bundle.
const sdk = {
  registerIsland(name, fn) {
    islands[name] = fn;
    for (const t of document.querySelectorAll(`[data-island="${CSS.escape(name)}"]`)) mount(name, t);
  },
  decorateCooked(selector, fn) {
    cookedDecorators.push([selector, fn]);
    for (const e of document.querySelectorAll(`.cooked ${selector}`)) guarded(fn, e);
  },
  // `onClick({ id, element })` for the post the button sits on; also covers posts loaded later.
  addPostMenuButton(label, onClick) {
    postButtons.push({ label, onClick });
    for (const footer of document.querySelectorAll(".post[data-post] .post-actions"))
      addPostButton(footer, label, onClick);
  },
  // `onClick({ insert })`, where `insert(text)` puts text at the cursor. The editor bundle loads
  // later and reads this list; composers already open are told through the event.
  addComposerToolbarButton(label, onClick, icon = "") {
    const button = { label, icon, onClick: (ctx) => guarded(onClick, ctx) };
    (window.bfComposerButtons ??= []).push(button);
    document.dispatchEvent(new CustomEvent("bf:composer-button", { detail: button }));
  },
  // Events: "posts-added", "composer-button", or anything a plugin dispatches as `bf:<name>`.
  on(name, fn) {
    const listener = (e) => guarded(fn, e.detail);
    document.addEventListener(`bf:${name}`, listener);
    return () => document.removeEventListener(`bf:${name}`, listener);
  },
  fetch: (url, opts = {}) => fetch(url, { ...opts, headers: { ...opts.headers, "X-CSRF-Token": csrf() } }),
};
function runPlugin(register) {
  try {
    register(sdk);
  } catch (e) {
    console.warn("plugin failed", e); // the page keeps its server HTML
  }
}
(window.bfPlugins ?? []).forEach(runPlugin);
window.bfPlugins = { push: runPlugin };

// Buttons a plugin declares in its manifest (`[[post_actions]]`, `[[composer_buttons]]`) are drawn
// here without any client code of the plugin: pressing one asks the plugin's `action.run` hook.
async function runAction(a, body, insert) {
  const res = await fetch(`/plugin/${encodeURIComponent(a.plugin)}/action/${encodeURIComponent(a.key)}`, {
    method: "POST",
    headers: jsonHeaders(),
    body: JSON.stringify(body),
  });
  const out = await res.json().catch(() => ({}));
  if (!res.ok) return toast(out?.detail ?? tr("That did not work."), true);
  if (out.toast) toast(out.toast);
  if (out.insert && insert) insert(out.insert);
  if (out.html) {
    // The site's one dialog; the HTML is the plugin's, sanitized by the server.
    const holder = el("div", { class: "cooked" });
    holder.innerHTML = cookedPolicy.createHTML(out.html);
    dialog({ title: a.label, body: [holder] }).show();
  }
}
try {
  for (const a of JSON.parse(document.getElementById("bf-plugin-actions")?.textContent || "[]")) {
    if (a.kind === "post") {
      sdk.addPostMenuButton(a.label, ({ id }) => runAction(a, { post_id: id }));
    } else if (a.kind === "composer") {
      sdk.addComposerToolbarButton(
        a.label,
        (ctx) => runAction(a, { selection: ctx.selection?.() ?? "" }, ctx.insert),
        a.icon,
      );
    }
  }
} catch (e) {
  console.warn("plugin buttons unreadable", e);
}

// Slots a plugin draws per viewer (`slot.render`): the page holds a placeholder, and the
// answer is the plugin's HTML, sanitized by the server.
function fillPluginSlots(root = document) {
  for (const holder of root.querySelectorAll("[data-plugin-slot]:not([data-filled])")) {
    holder.dataset.filled = "1";
    const q = new URLSearchParams({ page: location.pathname });
    const post = /** @type {HTMLElement | null} */ (holder.closest("[data-post]"))?.dataset.post;
    if (post) q.set("post", post);
    fetch(`/plugin-slot/${encodeURIComponent(holder.dataset.pluginSlot)}?${q}`)
      .then((r) => (r.ok ? r.text() : ""))
      .then((html) => {
        if (html) holder.innerHTML = cookedPolicy.createHTML(html);
      })
      .catch(() => {});
  }
}
fillPluginSlots();
document.addEventListener("bf:posts-added", () => fillPluginSlots());

// Plugin client code loads only where it is needed: on pages with one of its islands.
try {
  const modules = JSON.parse(document.getElementById("bf-plugin-modules")?.textContent || "[]");
  for (const { src, islands: names } of modules) {
    const wanted = !names.length || names.some((n) => document.querySelector(`[data-island="${CSS.escape(n)}"]`));
    if (wanted) import(/* @vite-ignore */ src).catch((e) => console.warn("plugin failed to load", src, e));
  }
} catch (e) {
  console.warn("plugin list unreadable", e);
}

// Registering a worker counts as loading a script, so Trusted Types wants a policy; this one
// allows that one address and nothing else.
const workerPolicy = window.trustedTypes?.createPolicy("bf-worker", {
  createScriptURL: (u) => {
    if (u !== "/sw.js") throw new TypeError("unexpected worker address");
    return u;
  },
}) ?? { createScriptURL: (u) => u };
if ("serviceWorker" in navigator)
  navigator.serviceWorker.register(workerPolicy.createScriptURL("/sw.js")).catch(() => {});

// "Edit layout" (P2, the page builder): admins drag widgets between a page's areas. The
// button keeps the page's own query (a search, a page number); the editor itself is in the
// bundle that also holds the post editor, loaded once it is open.
function editLayout(target, { editorUrl }) {
  const url = new URL(location.href);
  if (url.searchParams.get("edit_layout") !== "1") {
    url.searchParams.set("edit_layout", "1");
    url.hash = "";
    target.href = url.pathname + url.search;
    return;
  }
  import(/* @vite-ignore */ editorUrl).then((m) => m.layoutEditor());
}

/**
 * Starts the island `name` on a `[data-island]` element; its props are the JSON in `data-props`.
 * @param {string} name
 * @param {HTMLElement} target
 */
export function mount(name, target) {
  const fn = islands[name];
  if (!fn) return false;
  let props = {};
  try {
    props = JSON.parse(target.dataset.props || "{}");
  } catch {
    // Bad props mount the island with defaults instead of breaking the page.
  }
  fn(target, props);
  return true;
}
