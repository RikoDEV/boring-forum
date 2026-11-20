// The composer's AI helper: proofread, tone, shorten, explain, and title and tag ideas for the
// draft. Nothing is applied by itself: the answer is shown, and the member chooses to use it.
import { jsonHeaders, tr } from "../dom.js";

/** @param {object} body */
async function ask(body) {
  const res = await fetch("/api/v1/ai/helper", {
    method: "POST",
    headers: jsonHeaders({ Accept: "application/json" }),
    body: JSON.stringify(body),
  }).catch(() => null);
  if (!res) throw new Error(tr("That did not work. Try again."));
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(data.detail || tr("That did not work. Try again."));
  return data;
}

const button = (label, cls = "btn") => {
  const b = document.createElement("button");
  b.type = "button";
  b.className = cls;
  b.textContent = label;
  return b;
};

/**
 * The AI button and its panel, or null when the site has the helper off.
 * @param {{view: any, iconButton: (name: string, title: string) => HTMLButtonElement, topic: string | null, form: HTMLFormElement | null}} p
 */
export function aiHelper({ view, iconButton, topic, form }) {
  if (!document.querySelector('meta[name="bf-ai-helper"]')) return null;
  const titleInput = form?.querySelector('input[name="title"]') ?? null;
  const tagsInput = /** @type {HTMLInputElement | null} */ (form?.querySelector("#tags") ?? null);
  const forumPicker = /** @type {HTMLInputElement | null} */ (form?.querySelector("[data-forum-picker]") ?? null);

  const root = document.createElement("div");
  root.className = "composer-ai";
  const open = iconButton("sparkles", tr("AI help with this draft"));
  open.setAttribute("aria-haspopup", "true");
  open.setAttribute("aria-expanded", "false");
  const panel = document.createElement("div");
  panel.className = "composer-ai-panel";
  panel.hidden = true;
  const result = document.createElement("div");
  result.className = "composer-ai-result";
  result.setAttribute("aria-live", "polite");
  const menu = document.createElement("div");
  menu.className = "composer-ai-menu";
  panel.append(menu, result);

  const show = (on) => {
    panel.hidden = !on;
    open.setAttribute("aria-expanded", String(on));
  };
  const range = () => {
    const { from, to } = view.state.selection.main;
    return from === to ? { from: 0, to: view.state.doc.length } : { from, to };
  };
  const context = () => ({
    topic: topic ? Number(topic) : undefined,
    forum: forumPicker?.value ? Number(forumPicker.value) : undefined,
  });

  /** @param {string} action */
  const run = async (action) => {
    const { from, to } = range();
    const text = view.state.sliceDoc(from, to).trim();
    if (!text) {
      result.replaceChildren(tr("Write something first."));
      return;
    }
    result.replaceChildren(tr("Working…"));
    panel.setAttribute("aria-busy", "true");
    try {
      const r = await ask({ action, text, ...context() });
      result.replaceChildren();
      if (r.text !== undefined) {
        const box = document.createElement("div");
        box.className = "composer-ai-text";
        box.textContent = r.text;
        const use = button(action === "explain" ? tr("Insert below") : tr("Use this"), "btn primary");
        use.addEventListener("click", () => {
          const insert = action === "explain" ? { from: to, insert: `\n\n${r.text}` } : { from, to, insert: r.text };
          view.dispatch({ changes: insert, userEvent: "input" });
          view.focus();
          show(false);
        });
        const cancel = button(tr("Dismiss"));
        cancel.addEventListener("click", () => show(false));
        const row = document.createElement("div");
        row.className = "composer-ai-actions";
        row.append(use, cancel);
        result.append(box, row);
      } else if (r.titles) {
        for (const t of r.titles) {
          const b = button(t, "composer-ai-choice");
          b.addEventListener("click", () => {
            if (titleInput) titleInput.value = t;
            titleInput?.dispatchEvent(new Event("input", { bubbles: true }));
            show(false);
          });
          result.append(b);
        }
        if (!r.titles.length) result.append(tr("No ideas this time."));
      } else if (r.tags) {
        const b = button(r.tags.join(", ") || tr("No ideas this time."), "composer-ai-choice");
        b.addEventListener("click", () => {
          if (tagsInput && r.tags.length) tagsInput.value = r.tags.join(", ");
          show(false);
        });
        result.append(b);
      }
    } catch (e) {
      result.replaceChildren(e.message);
    } finally {
      panel.removeAttribute("aria-busy");
    }
  };

  /** @type {Array<[string, string]>} */
  const actions = [
    ["proofread", tr("Proofread")],
    ["friendly", tr("Friendlier")],
    ["formal", tr("More formal")],
    ["shorten", tr("Shorten")],
    ["explain", tr("Explain")],
  ];
  if (titleInput) actions.push(["titles", tr("Suggest a title")]);
  if (tagsInput) actions.push(["tags", tr("Suggest tags")]);
  for (const [action, label] of actions) {
    const b = button(label, "composer-ai-item");
    b.addEventListener("click", () => run(action));
    menu.append(b);
  }
  open.addEventListener("click", () => {
    show(panel.hidden);
    if (!panel.hidden) result.replaceChildren();
  });
  document.addEventListener("click", (e) => {
    if (!root.contains(/** @type {Node} */ (e.target))) show(false);
  });
  root.append(open, panel);
  return root;
}
