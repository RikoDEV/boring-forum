// AI features on topic pages and in the composer: the summary panel, related topics, machine
// translation of a post, and similar topics while a title is typed. Everything here asks the
// server, which checks who may and what it costs; nothing changes on the page unless it answers.
import { announce, el, jsonHeaders, icon, toast, tr } from "./dom.js";

// Posts translated by the server are already sanitized by the cooker; this is the only HTML here.
const policy = window.trustedTypes?.createPolicy("bf-ai", { createHTML: (s) => s }) ?? { createHTML: (s) => s };

/** POSTs JSON; the answer, or the server's own message. @param {string} url @param {object} [body] */
async function ask(url, body = {}) {
  const res = await fetch(url, {
    method: "POST",
    headers: jsonHeaders({ Accept: "application/json" }),
    body: JSON.stringify(body),
  }).catch(() => null);
  if (!res) throw new Error(tr("That did not work. Try again."));
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(data.detail || tr("That did not work. Try again."));
  return data;
}

/** A summary as a sentence and a list of points. @param {string} text */
function paragraphs(text) {
  const box = el("div", { class: "bf-ai-text" });
  let list = null;
  for (const line of text.split("\n").map((l) => l.trim()).filter(Boolean)) {
    if (/^[-*•]\s+/.test(line)) {
      list ??= box.appendChild(el("ul"));
      list.append(el("li", {}, line.replace(/^[-*•]\s+/, "")));
    } else {
      list = null;
      box.append(el("p", {}, line));
    }
  }
  return box;
}

function summaryPanel(panel, id, hasUnread, member, button) {
  /** @param {boolean} since @param {{summary: string, stale?: boolean} | null} [known] */
  const run = async (since, known = null) => {
    panel.hidden = false;
    if (button) button.hidden = true;
    panel.replaceChildren(el("p", { class: "muted" }, tr("Summarizing…")));
    panel.setAttribute("aria-busy", "true");
    try {
      const r = known ?? (await ask(`/api/v1/topics/${id}/ai-summary`, { since }));
      const head = el("div", { class: "bf-box-head" });
      const title = el("h2");
      title.append(icon("sparkles"), " ", since ? tr("What you have not read") : tr("Summary"));
      const close = el("button", { type: "button", class: "bf-icon-btn", "aria-label": tr("Close") });
      close.append(icon("x"));
      close.addEventListener("click", () => {
        panel.hidden = true;
        if (button) button.hidden = false;
      });
      head.append(title, close);
      const foot = el("p", { class: "muted small" }, tr("Made by AI from the posts. It can be wrong; the posts have the details."));
      const more = [];
      if (!since && member && r.stale) {
        const b = el("button", { type: "button", class: "btn" }, tr("Update the summary"));
        b.addEventListener("click", () => run(false));
        more.push(b);
      }
      if (!since && member && hasUnread) {
        const b = el("button", { type: "button", class: "btn" }, tr("Only what I have not read"));
        b.addEventListener("click", () => run(true));
        more.push(b);
      }
      panel.replaceChildren(head, paragraphs(r.summary), foot);
      if (more.length) {
        const row = el("div", { class: "bf-ai-actions" });
        row.append(...more);
        panel.append(row);
      }
      announce(tr("The summary is ready."));
    } catch (e) {
      panel.replaceChildren(el("p", {}, e.message));
    } finally {
      panel.removeAttribute("aria-busy");
    }
  };
  return run;
}

async function related(box, id) {
  const res = await fetch(`/api/v1/topics/${id}/related`).catch(() => null);
  const items = res?.ok ? await res.json().catch(() => []) : [];
  if (!items.length) return;
  const list = box.querySelector("ul");
  for (const t of items) {
    const li = el("li");
    li.append(el("a", { href: t.url }, t.title));
    if (t.forum) li.append(" ", el("span", { class: "muted small" }, t.forum));
    list.append(li);
  }
  box.hidden = false;
}

/** Replaces a post's text with its translation, the original kept in a fold. @param {Element} post @param {string} html */
function showTranslation(post, html) {
  const cooked = post.querySelector(".cooked");
  if (!cooked || cooked.querySelector(".bf-translation")) return;
  const original = el("div");
  original.append(...cooked.childNodes);
  const translated = el("div", { class: "bf-translation" });
  translated.innerHTML = policy.createHTML(html);
  const fold = el("details", { class: "bf-original small" });
  fold.append(el("summary", {}, tr("Translated automatically. Show the original")), original);
  cooked.append(translated, fold);
}

/** @param {Element} _t @param {{id: number, summary?: boolean, member?: boolean, related?: boolean, translate?: boolean, unread?: boolean}} p */
export function aiTopic(_t, { id, summary, member, related: wantRelated, translate, unread }) {
  const panel = document.querySelector("[data-ai-panel]");
  if (summary && panel) {
    const button = document.querySelector("[data-ai-summary]");
    const run = summaryPanel(panel, id, unread, member, button);
    button?.addEventListener("click", () => run(false));
    // A summary made earlier is simply there, under the first post.
    fetch(`/api/v1/topics/${id}/ai-summary`)
      .then((res) => (res.status === 200 ? res.json() : null))
      .then((known) => known && run(false, known))
      .catch(() => {});
  }
  const box = document.querySelector("[data-ai-related]");
  if (wantRelated && box) related(box, id);
  if (translate) {
    document.addEventListener("click", async (e) => {
      const b = /** @type {HTMLElement | null} */ (e.target.closest?.("[data-ai-translate]"));
      if (!b || b.hasAttribute("aria-busy")) return;
      b.setAttribute("aria-busy", "true");
      try {
        const r = await ask(`/api/v1/posts/${b.dataset.aiTranslate}/ai-translate`);
        showTranslation(b.closest("article, .bf-post, [id^='post-']") ?? document, r.cooked);
        b.closest("details")?.removeAttribute("open");
      } catch (err) {
        toast(err.message, true);
      } finally {
        b.removeAttribute("aria-busy");
      }
    });
  }
}

/** Similar topics under the title field of the new-topic form. @param {HTMLInputElement} input */
export function similarTopics(input) {
  const box = el("div", { class: "bf-ai-similar muted small", hidden: true, "aria-live": "polite" });
  input.closest(".bf-field")?.after(box);
  let timer = 0;
  let last = "";
  input.addEventListener("input", () => {
    clearTimeout(timer);
    const q = input.value.trim();
    if (q.length < 12 || q === last) return;
    timer = setTimeout(async () => {
      last = q;
      const res = await fetch(`/api/v1/ai/similar?q=${encodeURIComponent(q)}`).catch(() => null);
      const items = res?.ok ? await res.json().catch(() => []) : [];
      box.replaceChildren();
      if (!items.length) {
        box.hidden = true;
        return;
      }
      box.append(el("strong", {}, tr("Already asked?")), " ");
      const list = el("ul");
      for (const t of items) {
        const li = el("li");
        li.append(el("a", { href: t.url, target: "_blank", rel: "noopener" }, t.title));
        list.append(li);
      }
      box.append(list);
      box.hidden = false;
    }, 700);
  });
}
