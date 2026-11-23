// Reading aids for topic and forum pages. All are progressive: without
// JavaScript, images are plain images and every page keeps its real URL.

import { csrf, el, formParams, fullDate, toast, tr, when as dated } from "./dom.js";

// Lightbox: click an image in a post or a chat message to view it large. The pictures that belong
// together open in one viewer, with a strip of the others to jump to: those of the same post, or of
// the same chat log; anything marked `data-lightbox-group` is its own set. Linked images follow
// their link. Pages that can show images in more than one place (a topic with the chat drawer open)
// ask for this more than once, so it starts only the first time.
let lightboxStarted = false;
export function lightbox() {
  if (lightboxStarted) return;
  lightboxStarted = true;
  const groupOf = (img) => img.closest("[data-lightbox-group]") ?? img.closest(".chat-log") ?? img.closest(".cooked");
  document.addEventListener("click", async (e) => {
    const img = e.target.closest?.(".cooked img");
    if (!img || img.closest("a") || e.defaultPrevented || typeof HTMLDialogElement === "undefined") return;
    const set = [...groupOf(img).querySelectorAll("img")].filter((i) => !i.closest("a") && i.closest(".cooked"));
    const { openLightbox } = await import("./lightbox.js");
    openLightbox(
      // The picture itself, not the thumbnail from srcset; the thumbnail goes in the strip.
      set.map((i) => ({ src: i.src, thumb: i.currentSrc || i.src, alt: i.title || i.alt })),
      Math.max(0, set.indexOf(img)),
    );
  });
}

// The pills under a post open a dialog with who reacted. It loads when first asked for, once per page.
if (!(/** @type {any} */ (window).bfReactionsDialog)) {
  /** @type {any} */ (window).bfReactionsDialog = true;
  document.addEventListener("click", async (e) => {
    const pills = e.target.closest?.("[data-reactions]");
    if (!pills || e.defaultPrevented) return;
    e.preventDefault();
    const { openReactions } = await import("./reactions.js");
    openReactions(pills.dataset.reactions, pills);
  });
}

// Reacting to a post changes the page in place: the buttons under it send their form in the
// background and the pills and pressed states are drawn again from the answer, so the page neither
// reloads nor jumps to the top.
/** @param {Element} footer @param {string} postId @param {{ mine: number | null, pills: { icon: string, count: number, mine: boolean }[] }} data */
function redrawReactions(footer, postId, data) {
  const chosen = data.mine ?? null;
  const like = footer.querySelector(".bf-react-main");
  like?.classList.toggle("liked", chosen !== null);
  like?.setAttribute("aria-pressed", String(chosen !== null));
  for (const form of footer.querySelectorAll('form[action$="/react"]')) {
    const button = form.querySelector("button");
    const field = form.querySelector('input[name="reaction"]');
    if (!button || !(field instanceof HTMLInputElement)) continue;
    const id = Number(button.dataset.reaction);
    button.setAttribute("aria-pressed", String(id === chosen));
    // Pressing the one that is chosen takes it back.
    field.value = id === chosen ? "0" : String(id);
  }
  let pills = footer.querySelector(".bf-react-pills");
  if (!data.pills.length) {
    pills?.remove();
    return;
  }
  if (!pills) {
    pills = el("button", {
      type: "button",
      class: "bf-react-pills",
      "data-reactions": postId,
      "aria-haspopup": "dialog",
    });
    /** @type {HTMLElement} */ (pills).dataset.tip = tr("See who reacted");
    footer.prepend(pills);
  }
  pills.replaceChildren(
    ...data.pills.map((p) =>
      el("span", { class: p.mine ? "bf-react-pill is-mine" : "bf-react-pill" }, `${p.icon} ${p.count}`),
    ),
  );
}

if (!(/** @type {any} */ (window).bfReacting)) {
  /** @type {any} */ (window).bfReacting = true;
  document.addEventListener("submit", async (e) => {
    const form = e.target;
    if (!(form instanceof HTMLFormElement) || e.defaultPrevented) return;
    const match = /^\/post\/(\d+)\/(?:like|react)$/.exec(new URL(form.action, location.href).pathname);
    const footer = form.closest(".post-actions");
    if (!match || !footer) return;
    e.preventDefault();
    const buttons = [...form.querySelectorAll("button")];
    for (const b of buttons) b.disabled = true;
    try {
      const res = await fetch(form.action, {
        method: "POST",
        headers: { Accept: "application/json" },
        body: new URLSearchParams(/** @type {any} */ (new FormData(form))),
      });
      const data = await res.json().catch(() => ({}));
      if (!res.ok) throw new Error(data.detail || data.title || String(res.status));
      redrawReactions(footer, match[1], data);
    } catch (err) {
      toast(err instanceof Error ? err.message : tr("Something went wrong."), true);
    } finally {
      for (const b of buttons) b.disabled = false;
    }
  });
}

// Timeline scrubber: drag to a post number, release to go there.
/**
 * The timeline rail of a topic: a slider over the posts, with first and last dates.
 * @param {HTMLElement} target
 * @param {{highest?: number, base?: string, id: number, first?: string, last?: string, first_at?: string, last_at?: string}} props
 */
export function scrubber(target, { highest = 0, base = "", id, first = "", last = "", first_at = "", last_at = "" }) {
  if (highest < 2) return;
  const count = el("strong");
  const when = el("small");
  const handle = el("output", { class: "scrub-handle" });
  handle.append(count, when);
  const input = el("input", { type: "range", min: 1, max: highest, value: 1, "aria-label": tr("Jump to post number") });
  // Dates are fetched on first drag; the label shows the nearest known one.
  let dates = null;
  const dateOf = (n) => {
    let found = "";
    for (const [number, day] of dates ?? []) {
      if (number > n) break;
      found = day;
    }
    return found;
  };
  const paint = () => {
    const n = +input.value;
    // Posts on this page carry their own date; further ones use the fetched list.
    const shown = document.querySelector(`#post-${n} time`);
    count.textContent = `${n} / ${highest}`;
    when.textContent = shown?.textContent ?? dateOf(n);
    // The full date on hover, as everywhere: the post's own, or the day from the list.
    when.title = shown?.title || fullDate(shown?.getAttribute("datetime")) || dateOf(n);
    handle.style.setProperty("--p", String(highest > 1 ? (n - 1) / (highest - 1) : 0));
  };
  const loadDates = async () => {
    dates = [];
    const res = await fetch(`/api/v1/topics/${id}/dates`).catch(() => null);
    if (res?.ok) dates = await res.json();
    paint();
  };
  // Replies added live (live.js) move the end of the rail.
  document.addEventListener("bf:posts-added", (e) => {
    const n = +(/^post-(\d+)$/.exec(e.detail?.element?.id ?? "")?.[1] ?? 0);
    if (n <= highest) return;
    highest = n;
    input.max = String(n);
    paint();
  });
  const current = () => {
    const posts = document.querySelectorAll('#posts > [id^="post-"]');
    let n = 1;
    for (const p of posts) {
      if (p.getBoundingClientRect().top > window.innerHeight / 3) break;
      n = +p.id.slice(5);
    }
    return n;
  };
  let dragging = false;
  input.addEventListener("input", () => {
    dragging = true;
    if (!dates) loadDates();
    paint();
  });
  input.addEventListener("change", () => {
    const here = document.getElementById(`post-${input.value}`);
    dragging = false;
    if (here) here.scrollIntoView();
    else location.href = `${base}/${input.value}`;
  });
  // The address follows the post being read (/t/title/id/number), so a copied link or a reload
  // returns here. Written once scrolling pauses, and only when the post changes.
  let urlTimer = 0;
  let shown = "";
  const reflect = (n) => {
    clearTimeout(urlTimer);
    urlTimer = setTimeout(() => {
      const path = n > 1 ? `${base}/${n}` : base;
      if (!base || path === shown || path === location.pathname) return;
      shown = path;
      history.replaceState(history.state, "", path);
    }, 200);
  };
  let queued = false;
  const sync = (fromScroll = false) => {
    queued = false;
    if (dragging) return;
    const n = current();
    input.value = String(n);
    paint();
    if (fromScroll === true) reflect(n);
  };
  document.addEventListener(
    "scroll",
    () => {
      if (!queued) requestAnimationFrame(() => sync(true));
      queued = true;
    },
    { passive: true },
  );
  target.classList.add("scrubber");
  const track = el("div", { class: "scrub-track" });
  track.append(input, handle);
  // The first post's month ("Oct 2026"), and how long ago the last one was.
  const end = (text, iso, format) => {
    const e = el("div", { class: "scrub-end" });
    e.append(dated(text, iso, format ? { "data-format": format } : {}));
    return e;
  };
  target.append(end(first, first_at, "month"), track, end(last, last_at));
  sync();
}

import { tagInputs } from "./tags.js";

// Parsing only, into an inert document: our own next page, nothing is run from it.
const pagePolicy = window.trustedTypes?.createPolicy("bf-page", { createHTML: (s) => s }) ?? { createHTML: (s) => s };

// Infinite scroll, a per-member choice: fetches the next real page and appends its rows.
/**
 * Infinite scroll, a per-member choice: appends the next page of posts or topics.
 * @param {HTMLElement} target
 * @param {{on?: boolean, onAppend?: (added: Element[]) => void}} props
 */
export function infinite(target, { on = false, onAppend }) {
  const pager = document.querySelector("nav.pager");
  const list = () => document.querySelector("#posts") ?? document.querySelector("table.topics tbody");
  if (!pager || !list()) return;
  let observer = null;
  let loading = false;
  const toggle = el("button", { type: "button", class: "bf-switch small", role: "switch" }, tr("Infinite scroll"));
  const load = async () => {
    const next = /** @type {HTMLAnchorElement | null} */ (document.querySelector('nav.pager a[rel="next"]'));
    if (loading || !next) return;
    loading = true;
    const res = await fetch(next.href).catch(() => null);
    loading = false;
    if (!res?.ok) return;
    const doc = new DOMParser().parseFromString(pagePolicy.createHTML(await res.text()), "text/html");
    const from = doc.querySelector("#posts") ?? doc.querySelector("table.topics tbody");
    const to = list();
    if (!from || !to) return;
    const added = [...from.children];
    to.append(...added);
    onAppend?.(added);
    history.replaceState(null, "", next.href);
    const pagerNew = doc.querySelector("nav.pager");
    if (pagerNew) pager.replaceChildren(...pagerNew.childNodes, toggle);
    // Still in view (short pages): the observer will not fire again by itself.
    observer?.unobserve(pager);
    observer?.observe(pager);
  };
  const set = (value) => {
    toggle.setAttribute("aria-checked", String(value));
    observer?.disconnect();
    observer = null;
    if (value && "IntersectionObserver" in window) {
      observer = new IntersectionObserver((es) => es.some((e) => e.isIntersecting) && load(), { rootMargin: "600px" });
      observer.observe(pager);
    }
  };
  toggle.addEventListener("click", async () => {
    const value = toggle.getAttribute("aria-checked") !== "true";
    set(value);
    const res = await fetch("/api/v1/me/infinite-scroll", {
      method: "PUT",
      headers: {
        "Content-Type": "application/json",
        "X-CSRF-Token": csrf(),
      },
      body: JSON.stringify({ on: value }),
    }).catch(() => null);
    if (!res?.ok) set(!value);
  });
  // Only worth offering when there is another page.
  if (pager.querySelector('a[rel="next"]') || on) pager.append(toggle);
  set(on);
}

// New topic as a bottom sheet. The form is part of the forum page and opens by itself through
// :target without JS; here it gets the editor, chips and errors shown in place.
const problem = async (res) => {
  const text = await res.text();
  try {
    const data = JSON.parse(text);
    return data.detail || data.title;
  } catch {
    const page = new DOMParser().parseFromString(pagePolicy.createHTML(text), "text/html");
    return page.querySelector("main")?.textContent.trim() || text.trim() || tr("Could not create the topic.");
  }
};

function prepareSheet(form, saved, editorUrl) {
  const title = form.querySelector('input[name="title"]');
  const text = form.querySelector("textarea");
  const error = form.querySelector("[data-sheet-error]");
  if (saved.titleText) title.value = saved.titleText;
  if (saved.body) text.value = saved.body;
  if (saved.action) form.setAttribute("action", saved.action);
  form.querySelector("[data-sheet-close]")?.remove();
  const forum = form.getAttribute("action").match(/\d+/)[0];
  const picker = form.querySelector("[data-forum-picker]");
  if (picker?.options.length > 1) {
    picker.value = forum;
    picker.hidden = false;
    picker.addEventListener("change", () => {
      form.setAttribute("action", `/c/${picker.value}/new`);
      form.querySelector("#prefix")?.remove(); // prefixes belong to the previous forum
    });
  }
  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    const send = form.querySelector(".composer-foot > .btn.primary");
    send.disabled = true;
    error.hidden = true;
    const res = await fetch(form.action, { method: "POST", body: formParams(form) }).catch(() => null);
    if (res?.redirected) {
      form.dispatchEvent(new Event("bf:sent"));
      location.href = res.url;
      return;
    }
    error.textContent = res ? await problem(res) : tr("The server could not be reached. Your text is still here.");
    error.hidden = false;
    send.disabled = false;
  });
  tagInputs(form);
  return import(/* @vite-ignore */ editorUrl).then((m) => m.mount(text, { draft: `new-topic:${forum}`, sheet: true }));
}

async function openSheet(editorUrl, saved = {}) {
  let form = document.querySelector("form.bf-sheet");
  if (!form && saved.action) {
    // Resuming on a page without the form: it comes from that forum's page.
    const res = await fetch(`/c/x/${saved.action.match(/\d+/)[0]}`).catch(() => null);
    const page = res?.ok ? new DOMParser().parseFromString(pagePolicy.createHTML(await res.text()), "text/html") : null;
    form = page?.querySelector("form.bf-sheet");
    if (!form) return;
    document.body.append(document.adoptNode(form));
  }
  if (!form) return;
  form.classList.add("is-open");
  if (!form.dataset.ready) {
    form.dataset.ready = "1";
    await prepareSheet(form, saved, editorUrl);
  }
  form.querySelector('input[name="title"]').focus();
}

// Editing happens inside the post; there is no page for it.
const parsePage = (html) => new DOMParser().parseFromString(pagePolicy.createHTML(html), "text/html");

async function editInPlace(trigger, editorUrl) {
  const post = trigger.closest("article[data-post]");
  const cooked = post?.querySelector(".cooked:not(.signature)");
  if (!cooked) return;
  const open = post.querySelector("form.bf-inline-edit");
  if (open) {
    open.scrollIntoView({ block: "center" });
    return;
  }
  const id = post.dataset.post;
  const error = el("p", { class: "error", role: "alert", hidden: "" });
  const say = (message) => {
    error.textContent = message;
    error.hidden = false;
    error.scrollIntoView({ block: "center" });
  };
  const res = await fetch(`/api/v1/posts/${id}/source`).catch(() => null);
  if (!res?.ok) {
    cooked.before(error);
    say(
      res?.status === 403 ? tr("This post can no longer be edited.") : tr("The post could not be opened for editing."),
    );
    return;
  }
  const source = await res.json();
  const form = el("form", { method: "post", action: `/post/${id}/edit`, class: "composer bf-inline-edit" });
  const hidden = (name, value) => {
    const input = el("input", { type: "hidden", name });
    input.value = value;
    return input;
  };
  const base = hidden("base", source.base);
  const label = el("label", { for: `edit-body-${id}` }, tr("Message"));
  const text = el("textarea", { id: `edit-body-${id}`, name: "body", rows: "10" });
  text.value = source.raw;
  const reason = el("input", { id: `edit-reason-${id}`, name: "reason", placeholder: " " });
  reason.required = source.reason_required;
  const reasonBox = el("div", { class: "bf-field" });
  reasonBox.append(
    reason,
    el(
      "label",
      { class: "bf-field-label", for: reason.id },
      source.reason_required ? tr("Reason") : tr("Reason (optional)"),
    ),
  );
  const conflict = el("section", { class: "panel block conflict", hidden: "" });
  const save = el("button", { class: "btn primary" }, tr("Save"));
  const cancel = el("button", { type: "button", class: "btn" }, tr("Cancel"));
  const row = el("div", { class: "bf-inline-edit-actions" });
  row.append(save, cancel);
  form.append(error, conflict, hidden("csrf", csrf()), base, label, text, reasonBox, row);
  cooked.hidden = true;
  cooked.after(form);
  // The Edit button sits at the foot of the post. Hiding a long post's text shortens the page by
  // its height, and the view, still at the old scroll position, would be left far below the editor:
  // bring the editor into view, now and again once the editor has its full height.
  form.scrollIntoView({ block: "nearest" });
  const editor = await import(/* @vite-ignore */ editorUrl);
  const view = editor.mount(text, {});
  view.focus();
  form.scrollIntoView({ block: "nearest" });
  const close = () => {
    text.value = ""; // nothing left for the leave-page warning to protect
    view.destroy();
    form.remove();
    cooked.hidden = false;
  };
  cancel.addEventListener("click", close);
  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    save.disabled = true;
    error.hidden = true;
    const sent = await fetch(form.action, { method: "POST", body: formParams(form) }).catch(() => null);
    save.disabled = false;
    if (sent?.redirected) {
      // The page being read, not where the redirect points: it holds this post, in its place.
      const page = await fetch(location.pathname + location.search).catch(() => null);
      const fresh = page?.ok ? parsePage(await page.text()).querySelector(`article[data-post="${id}"]`) : null;
      text.value = "";
      view.destroy();
      if (!fresh) return location.reload();
      const node = document.adoptNode(fresh);
      post.replaceWith(node);
      document.dispatchEvent(new CustomEvent("bf:post-replaced", { detail: node }));
      return;
    }
    const answer = sent ? await sent.json().catch(() => null) : null;
    if (sent?.status === 409 && answer) {
      // Saving again goes over their edit, now that this member has seen it.
      base.value = answer.base;
      conflict.replaceChildren();
      for (const [title, diff] of [
        [tr("Their edit"), answer.theirs],
        [tr("Your edit"), answer.yours],
      ]) {
        const box = el("div", { class: "diff" });
        box.innerHTML = pagePolicy.createHTML(diff); // escaped by the server's diff
        conflict.append(el("h2", {}, title), box);
      }
      conflict.hidden = false;
    }
    say(answer?.detail || answer?.title || tr("The post could not be saved. Your text is still here."));
  });
}

function inlineEditing(editorUrl) {
  document.addEventListener("click", (e) => {
    const trigger = e.target.closest?.("article[data-post] [data-edit-post]");
    if (!trigger) return;
    trigger.closest("details")?.removeAttribute("open");
    editInPlace(trigger, editorUrl).catch(() => {});
  });
}

// A docked reply stays open while browsing: pages are full loads, so pages without
// a composer rebuild the panel from what the editor stored, and the draft restores its text.
/**
 * Reopens a reply the member started and left (kept for this tab).
 * @param {HTMLElement} target
 * @param {{editorUrl: string}} props
 */
export function composerResume(target, { editorUrl }) {
  inlineEditing(editorUrl);
  document.addEventListener("click", (e) => {
    if (!e.target.closest?.('a[href="#new-topic"]')) return;
    e.preventDefault();
    openSheet(editorUrl);
  });
  if (location.hash === "#new-topic") openSheet(editorUrl);
  let open = null;
  try {
    open = JSON.parse(localStorage.getItem("bf-composer-open") || "null");
  } catch {
    return;
  }
  if (open && /^\/c\/\d+\/new$/.test(open.action) && /^new-topic:\d+$/.test(open.draft))
    return void openSheet(editorUrl, open);
  if (!open || document.querySelector("form.composer:not(.bf-sheet)")) return;
  // Stored by our own editor, but checked anyway: the form must post to one of our URLs.
  if (!/^\/t\/\d+\/reply$/.test(open.action) || !/^reply:\d+$/.test(open.draft)) return;
  const form = el("form", { method: "post", action: open.action, class: "panel composer" });
  const csrfInput = el("input", { type: "hidden", name: "csrf" });
  csrfInput.value = csrf();
  const reply = el("input", { type: "hidden", name: "reply_to" });
  reply.value = open.replyTo ?? "";
  const label = el(
    "label",
    { for: "body" },
    open.title ? tr("Reply to {title}", { title: open.title }) : tr("Reply to the topic"),
  );
  const text = el("textarea", { id: "body", name: "body", rows: "6" });
  text.value = open.body ?? "";
  const send = el("button", { class: "btn primary" }, tr("Post reply"));
  {
    form.append(csrfInput, label, reply, text, send);
  }
  document.body.append(form);
  import(/* @vite-ignore */ editorUrl).then((m) => m.mount(text, { draft: open.draft }));
}
