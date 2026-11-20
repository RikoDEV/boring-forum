// Live updates for topic pages and topic lists, over SSE. Nothing here reloads the page:
// new posts and rows come from the server already rendered, and are put in place, so the
// reader keeps their place, their draft and whatever menu or dialog they have open.
import { announce, tn, tr } from "./dom.js";

const el = (tag, attrs = {}, text) => {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, v);
  if (text !== undefined) e.textContent = text;
  return e;
};

// Parsing only, into an inert document: our own pages and fragments, nothing is run from them.
const policy = window.trustedTypes?.createPolicy("bf-live", { createHTML: (s) => s }) ?? { createHTML: (s) => s };
const parse = (html) => new DOMParser().parseFromString(policy.createHTML(html), "text/html");
const dataOf = (e) => {
  try {
    return JSON.parse(e.data)?.data ?? {};
  } catch {
    return {};
  }
};

function bar(target) {
  const notice = el("div", { class: "live-notice", hidden: "", role: "status", "aria-live": "polite" });
  const button = el("button", { type: "button", class: "btn primary" });
  notice.append(button);
  target.append(notice);
  return { notice, button };
}

// A topic: replies are added at the end of the last page, edited and deleted posts are swapped
// where they stand, and the title follows the topic's state (locked, solved, retitled).
/**
 * Live updates for a topic page over SSE: new posts are appended as they arrive.
 * @param {HTMLElement} target
 * @param {{channel: string, onAppend?: (posts: any[]) => void}} props
 */
export function live(target, { channel, onAppend }) {
  const topicId = channel?.startsWith("item:") ? channel.slice(5) : null;
  const list = document.getElementById("posts");
  if (!topicId || !list) return;
  const isPost = (n) => /^post-\d+$/.test(n.id);
  const numberOf = (n) => +n.id.slice(5);
  const lastShown = () => Math.max(0, ...[...list.children].filter(isPost).map(numberOf));
  const onLastPage = () => !document.querySelector('nav.pager a[rel="next"]');
  const motion = () => (matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth");

  // Replies added below the fold, until the reader scrolls to them; or, away from the last
  // page, replies waiting there.
  const fresh = new Set();
  let elsewhere = 0;
  let newest = 0;
  const { notice, button } = bar(target);
  const paint = () => {
    const n = fresh.size || elsewhere;
    notice.hidden = n === 0;
    const what = tn("js.count.new_replies", n);
    button.textContent = fresh.size ? `${what} ↓` : tr("{what}: go to the last page", { what });
  };
  const seen =
    "IntersectionObserver" in window
      ? new IntersectionObserver((entries) => {
          for (const e of entries) {
            if (!e.isIntersecting) continue;
            fresh.delete(e.target);
            seen.unobserve(e.target);
          }
          paint();
        })
      : null;
  button.addEventListener("click", () => {
    const first = fresh.values().next().value;
    if (!first) {
      location.href = `/t/x/${topicId}/${newest}`;
      return;
    }
    first.setAttribute("tabindex", "-1");
    first.scrollIntoView({ block: "start", behavior: motion() });
    first.focus({ preventScroll: true });
  });

  // Posts as the topic page renders them for this viewer: one they may not see is left out.
  const part = async (from, count) => {
    const read = document.visibilityState === "visible" ? "&seen=true" : "";
    const res = await fetch(`/api/v1/topics/${topicId}/posts?from=${from}&count=${count}${read}`).catch(() => null);
    return res?.ok ? parse(await res.text()).body : null;
  };
  // A post comes with the notes before it ("3 weeks later", "first post"): they stay together.
  const groups = (body) => {
    const out = [];
    let lead = [];
    for (const n of [...body.children]) {
      lead.push(n);
      if (isPost(n)) {
        out.push({ number: numberOf(n), nodes: lead });
        lead = [];
      }
    }
    return out;
  };

  let busy = false;
  let again = false;
  const addNew = async () => {
    if (!onLastPage()) {
      elsewhere += 1;
      paint();
      return;
    }
    if (busy) {
      again = true;
      return;
    }
    busy = true;
    try {
      do {
        again = false;
        const body = await part(lastShown() + 1, 50);
        if (!body) break;
        const added = [];
        const authors = [];
        for (const g of groups(body)) {
          if (document.getElementById(`post-${g.number}`)) continue; // already here
          const nodes = g.nodes.map((n) => document.adoptNode(n));
          list.append(...nodes);
          added.push(...nodes);
          const post = nodes.at(-1);
          post.classList.add("is-live-new");
          authors.push(post.querySelector(".bf-author-name")?.textContent.trim() ?? "");
          const r = post.getBoundingClientRect();
          if (seen && !(r.top < innerHeight && r.bottom > 0)) {
            fresh.add(post);
            seen.observe(post);
          }
        }
        if (added.length) {
          onAppend?.(added);
          announce(
            authors.length === 1
              ? tr("New reply by {name}", { name: authors[0] })
              : tn("js.count.new_replies", authors.length),
          );
        }
      } while (again);
    } finally {
      busy = false;
    }
    paint();
  };

  // Not a post the reader is editing: their unsaved text wins until they save or cancel.
  const editing = (n) => !n || n.querySelector("form.bf-inline-edit");
  const replace = async (number) => {
    if (!number || editing(document.getElementById(`post-${number}`))) return;
    const body = await part(number, 1);
    if (!body) return;
    const now = document.getElementById(`post-${number}`);
    if (editing(now)) return;
    const g = groups(body).find((x) => x.number === number);
    if (!g) {
      // Deleted, or hidden from this viewer.
      fresh.delete(now);
      now.remove();
      paint();
      return;
    }
    // Only the post: the notes before it are as they were.
    const node = document.adoptNode(g.nodes.at(-1));
    if (now.classList.contains("is-live-new")) node.classList.add("is-live-new");
    now.replaceWith(node);
    document.dispatchEvent(new CustomEvent("bf:post-replaced", { detail: node }));
  };

  // The title with its badges, the crumb and the tags, from the page as it is now. The rest of
  // the header has menus bound to their elements, and a moderator's own change reloads anyway.
  const restate = async () => {
    const res = await fetch(location.pathname + location.search).catch(() => null);
    if (!res?.ok) return;
    const doc = parse(await res.text());
    // Contents, not the elements: the sticky header's title watches this h1.
    for (const sel of ["header.bf-topic-head h1", "nav.crumbs > span"]) {
      const from = doc.querySelector(sel);
      const to = document.querySelector(sel);
      if (from && to) to.replaceChildren(...[...from.childNodes].map((n) => document.adoptNode(n)));
    }
    const name = doc.querySelector("nav.crumbs > span")?.textContent.trim();
    const copy = document.querySelector(".bf-header-title .bf-ht-title");
    if (name && copy) {
      copy.textContent = name;
      copy.title = name;
    }
    // The tab keeps its unread count in front: "(2) Title - Forum".
    if (doc.title) document.title = document.title.replace(/^(\(\d+\) )?.*$/, (_, count = "") => count + doc.title);
    const tags = doc.querySelector(".bf-topic-tags");
    const here = document.querySelector(".bf-topic-tags");
    if (tags && here) here.replaceWith(document.adoptNode(tags));
    else if (tags) document.querySelector("header.bf-topic-head h1")?.after(document.adoptNode(tags));
    else here?.remove();
    // The live Q&A panel, unless the reader is using it.
    const panel = doc.getElementById("live");
    const mine = document.getElementById("live");
    if (panel && mine && !mine.contains(document.activeElement)) mine.replaceWith(document.adoptNode(panel));
  };

  const onPost = (n) => {
    newest = Math.max(newest, n || 0);
    addNew();
  };
  // Without SSE, or when it keeps failing: the newest post number, every 30 s.
  let polling = false;
  const poll = () => {
    if (polling) return;
    polling = true;
    let known = null;
    const check = async () => {
      const res = await fetch(`/api/v1/topics/${topicId}/state`).catch(() => null);
      if (!res?.ok) return;
      const { highest_post_number: n } = await res.json();
      if (known !== null && n > known) onPost(n);
      known = n;
    };
    check();
    setInterval(check, 30000);
  };
  if (typeof EventSource === "undefined") {
    poll();
    return;
  }
  let failures = 0;
  const es = new EventSource(`/api/v1/live?channels=${encodeURIComponent(channel)}`);
  es.onopen = () => (failures = 0);
  es.onerror = () => {
    if (++failures >= 3) {
      es.close();
      poll();
    }
  };
  es.addEventListener("post", (e) => onPost(dataOf(e).post_number));
  es.addEventListener("edit", (e) => replace(dataOf(e).post_number));
  es.addEventListener("state", () => restate());
  // Messages were dropped for this slow connection: catch up from the page's own state.
  es.addEventListener("resync", async () => {
    const res = await fetch(`/api/v1/topics/${topicId}/state`).catch(() => null);
    if (res?.ok) onPost((await res.json()).highest_post_number);
    restate();
  });
}

// A topic list: rows never move under the reader. A bar counts new topics and replies, and
// showing them swaps the list in place, with what changed marked.
/**
 * Live updates for a topic list: the rows are replaced when a newer list is available.
 * @param {HTMLElement} target
 * @param {{channel: string, onAppend?: (posts: any[]) => void}} props
 */
export function liveList(target, { channel, onAppend }) {
  const tbody = () => /** @type {HTMLTableSectionElement | null} */ (document.querySelector("table.topics tbody"));
  if (!tbody() || typeof EventSource === "undefined") return;
  let topics = 0;
  let replies = 0;
  const { notice, button } = bar(target);
  const counted = () => {
    const parts = [];
    if (topics) parts.push(tn("count.topics", topics));
    if (replies) parts.push(tn("count.replies", replies));
    return parts.length ? tr("New: {what}", { what: parts.join(", ") }) : "";
  };
  const paint = () => {
    const text = counted();
    notice.hidden = !text;
    button.textContent = tr("{what}: show", { what: text });
  };
  const es = new EventSource(`/api/v1/live?channels=${encodeURIComponent(channel)}`);
  es.addEventListener("topic", () => {
    topics += 1;
    paint();
    announce(counted());
  });
  es.addEventListener("reply", () => {
    replies += 1;
    paint();
    announce(counted());
  });
  button.addEventListener("click", async () => {
    button.disabled = true;
    const res = await fetch(location.pathname + location.search).catch(() => null);
    button.disabled = false;
    if (!res?.ok) return;
    const from = /** @type {HTMLTableSectionElement | null} */ (
      parse(await res.text()).querySelector("table.topics tbody")
    );
    const to = tbody();
    if (!from || !to) return;
    // A row is new or changed when its text is: those are marked, and the first gets focus.
    const before = new Map([...to.rows].map((r) => [r.dataset.topicId, r.textContent]));
    const rows = [...from.rows].map((r) => document.adoptNode(r));
    to.replaceChildren(...rows);
    const changed = rows.filter((r) => r.dataset.topicId && before.get(r.dataset.topicId) !== r.textContent);
    for (const r of changed) r.classList.add("is-live-new");
    onAppend?.(rows);
    topics = 0;
    replies = 0;
    paint();
    (changed[0] ?? rows[0])?.querySelector(".bf-topic-title a")?.focus();
  });
}
