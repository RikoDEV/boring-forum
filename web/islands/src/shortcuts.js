import { dialog } from "./dialog.js";
import { el, tr } from "./dom.js";

// Keyboard shortcuts: / search, j/k next/previous post, r reply, g h home, ? help.
// Plain JS so it costs a few hundred bytes of the guest budget.
const HELP = tr("Keyboard shortcuts:\n/  search\nj / k  next / previous post\nr  reply\ng h  home\n?  this help");

// The help text's first line is its title, the rest a list of keys; the dialog closes on Escape or ×.
function showHelp() {
  if (document.querySelector("dialog.bf-shortcuts[open]")) return;
  const [title, ...lines] = HELP.split("\n");
  const list = el("pre", { class: "bf-shortcuts-list" }, lines.join("\n"));
  dialog({ title: title.replace(/[:：]$/, ""), className: "bf-shortcuts", body: [list] }).show();
}

export function shortcuts() {
  let g = false;
  document.addEventListener("keydown", (e) => {
    const t = /** @type {HTMLElement} */ (e.composedPath()[0]); // the real target, also inside the editor's shadow root
    if (e.ctrlKey || e.metaKey || e.altKey || t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName))
      return;
    const posts = [...document.querySelectorAll(".post")];
    const current = posts.findIndex((p) => p.getBoundingClientRect().top > 10);
    const go = (el) => el && (el.scrollIntoView({ block: "start" }), el.focus?.({ preventScroll: true }));
    if (g && e.key === "h") location.href = "/";
    g = e.key === "g";
    if (e.key === "/") {
      e.preventDefault();
      document.querySelector('.header-search input[name="q"]')?.focus();
    } else if (e.key === "j") {
      go(
        posts[current === -1 ? posts.length - 1 : current + (posts[current]?.getBoundingClientRect().top < 40 ? 1 : 0)],
      );
    } else if (e.key === "k") {
      go(posts[Math.max(0, (current === -1 ? posts.length : current) - 1)]);
    } else if (e.key === "r") {
      const body = /** @type {HTMLTextAreaElement | null} */ (document.querySelector("#body"));
      if (body) {
        e.preventDefault();
        // Once the editor is up the textarea is hidden; the form hands focus on to the editor.
        if (body.hidden) body.form?.dispatchEvent(new Event("bf:focus"));
        else body.focus();
        (body.hidden ? body.form : body)?.scrollIntoView({ block: "center" });
      }
    } else if (e.key === "?") {
      showHelp();
    }
  });
}
