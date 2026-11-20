// Header search: scopes, grouped results, recent searches and keyboard control. The form stays a
// plain GET to /search, so Enter works the same without JavaScript.
import { composing, el, icon, tr } from "./dom.js";

const KEY = "bf-recent-searches";
const GROUPS = [
  ["member", tr("Members"), "user"],
  ["forum", tr("Forums"), "folder"],
  ["tag", tr("Tags"), "flag"],
  ["topic", tr("Topics and posts"), "message-square"],
];
const TIPS = [
  ["in:mine", tr("Only my posts")],
  ["in:bookmarks", tr("Only my bookmarks")],
  ["status:closed", tr("Closed topics")],
  ["@", tr("By a member, like @name")],
];

const recent = () => {
  try {
    return JSON.parse(localStorage.getItem(KEY) || "[]");
  } catch {
    return [];
  }
};
const saveRecent = (list) => {
  try {
    localStorage.setItem(KEY, JSON.stringify(list));
  } catch {
    // Storage blocked: nothing to remember.
  }
};

// The match in bold; text nodes only, so a result cannot add markup.
function marked(text, q) {
  const span = el("span");
  const at = q ? text.toLowerCase().indexOf(q.toLowerCase()) : -1;
  if (at < 0) {
    span.textContent = text;
    return span;
  }
  span.append(text.slice(0, at), el("mark", {}, text.slice(at, at + q.length)), text.slice(at + q.length));
  return span;
}

// Where else this page lets us search: the forum or topic being read.
function pageScopes() {
  const scopes = [
    ["all", tr("Everything"), {}],
    ["topics", tr("Topics"), { in: "topics" }],
  ];
  const forum = location.pathname.match(/^\/c\/(?:[^/]+\/)?(\d+)/);
  const topic = location.pathname.match(/^\/t\/(?:[^/]+\/)?(\d+)/);
  if (forum) scopes.push(["forum", tr("This forum"), { forum: forum[1] }]);
  if (topic) scopes.push(["topic", tr("This topic"), { topic: topic[1] }]);
  return scopes;
}

/** The header search box: suggestions as you type, scopes, recent searches. @param {HTMLElement} target */
export function liveSearch(target) {
  const form = target.closest("form");
  const input = form?.querySelector('input[name="q"]');
  if (!input) return;
  const scopes = pageScopes();
  let scope = scopes[0];
  let timer;
  let seq = 0;
  let active = -1;

  const panel = el("div", { class: "bf-search-panel", hidden: "" });
  const chips = el("div", { class: "bf-search-scopes", role: "group", "aria-label": tr("Search in") });
  const body = el("div", { class: "bf-search-body" });
  const foot = el("div", { class: "bf-search-foot" });
  panel.append(chips, body, foot);
  target.append(panel);
  const hidden = el("span", { hidden: "" });
  form.append(hidden);

  const links = () => [...body.querySelectorAll("a"), ...foot.querySelectorAll("a")];
  const setActive = (i) => {
    const all = links();
    active = all.length ? (i + all.length) % all.length : -1;
    all.forEach((a, n) => a.classList.toggle("is-active", n === active));
    if (active >= 0) all[active].scrollIntoView({ block: "nearest" });
  };
  const close = () => {
    panel.hidden = true;
    active = -1;
    form.classList.remove("is-open");
  };
  const open = () => {
    panel.hidden = false;
    form.classList.add("is-open");
  };
  const params = (q) => new URLSearchParams({ q, .../** @type {Record<string, string>} */ (scope[2]) }).toString();
  const remember = (q) => q && saveRecent([q, ...recent().filter((r) => r !== q)].slice(0, 5));

  const paintFoot = (q) => {
    const parts = [];
    if (q) parts.push(el("a", { href: `/search?${params(q)}` }, tr("See all results for “{q}”", { q })));
    parts.push(el("a", { href: "/search", class: "bf-search-advanced" }, tr("Advanced search")));
    foot.replaceChildren(...parts);
  };

  const paintEmpty = () => {
    const nodes = [];
    const list = recent();
    if (list.length) {
      const head = el("div", { class: "bf-search-head" });
      const clear = el("button", { type: "button", class: "link small" }, tr("Clear"));
      clear.addEventListener("click", () => {
        saveRecent([]);
        paintEmpty();
      });
      head.append(el("span", {}, tr("Recent searches")), clear);
      const ul = el("ul");
      for (const r of list) {
        const li = el("li");
        const a = el("a", { href: `/search?q=${encodeURIComponent(r)}` });
        a.append(icon("history"), el("span", {}, r));
        const x = el("button", { type: "button", class: "bf-search-x", "aria-label": tr("Forget {q}", { q: r }) });
        x.append(icon("x"));
        x.addEventListener("click", () => {
          saveRecent(recent().filter((v) => v !== r));
          paintEmpty();
        });
        li.append(a, x);
        ul.append(li);
      }
      nodes.push(head, ul);
    }
    const tips = el("div", { class: "bf-search-tips" });
    for (const [op, label] of TIPS) {
      const b = el("button", { type: "button", "data-tip": label }, op);
      b.addEventListener("click", () => {
        input.value = `${op}${op.endsWith(":") || op === "@" ? "" : " "}`;
        input.focus();
      });
      tips.append(b);
    }
    nodes.push(el("div", { class: "bf-search-head" }, tr("Try")), tips);
    body.replaceChildren(...nodes);
    paintFoot("");
  };

  const paintResults = (q, rows) => {
    const nodes = [];
    for (const [kind, title, ic] of GROUPS) {
      const group = rows.filter((r) => (r.kind ?? "topic") === kind);
      if (!group.length) continue;
      const ul = el("ul");
      for (const r of group) {
        const a = el("a", { href: r.url });
        if (kind === "member")
          a.append(
            el("img", {
              class: "avatar",
              src: `/avatar/${encodeURIComponent(r.title)}.svg`,
              width: 24,
              height: 24,
              alt: "",
            }),
          );
        else a.append(icon(ic));
        const text = el("div");
        text.append(marked(r.title, q));
        if (kind === "topic" && r.snippet) text.append(el("small", {}, r.snippet));
        a.append(text);
        a.addEventListener("click", () => remember(q));
        const li = el("li");
        li.append(a);
        ul.append(li);
      }
      nodes.push(el("div", { class: "bf-search-head" }, title), ul);
    }
    if (!nodes.length) nodes.push(el("p", { class: "bf-search-none" }, tr("Nothing quick for “{q}”.", { q })));
    body.replaceChildren(...nodes);
    active = -1;
    paintFoot(q);
  };

  const run = () => {
    clearTimeout(timer);
    const q = input.value.trim();
    if (q.length < 2) {
      seq++;
      paintEmpty();
      return;
    }
    timer = setTimeout(async () => {
      const mine = ++seq;
      const res = await fetch(`/api/v1/search?${params(q)}`).catch(() => null);
      if (mine !== seq) return;
      paintResults(q, res?.ok ? await res.json() : []);
    }, 200);
  };

  const paintScopes = () => {
    chips.replaceChildren(
      ...scopes.map((s) => {
        const b = el("button", { type: "button", "aria-pressed": String(s === scope) }, String(s[1]));
        b.addEventListener("click", () => {
          scope = s;
          hidden.replaceChildren(
            ...Object.entries(s[2]).map(([name, value]) => {
              const i = el("input", { type: "hidden", name });
              i.value = value;
              return i;
            }),
          );
          paintScopes();
          input.focus();
          run();
        });
        return b;
      }),
    );
  };

  paintScopes();
  input.addEventListener("focus", () => {
    open();
    run();
  });
  input.addEventListener("input", () => {
    open();
    run();
  });
  input.addEventListener("keydown", (e) => {
    if (composing(e)) return; // an IME is still choosing its text
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      setActive(active + (e.key === "ArrowDown" ? 1 : -1));
    } else if (e.key === "Enter" && active >= 0) {
      e.preventDefault();
      links()[active].click();
    } else if (e.key === "Escape") {
      close();
      input.blur();
    }
  });
  form.addEventListener("submit", () => remember(input.value.trim()));
  document.addEventListener("click", (e) => {
    // A button may have just removed itself from the panel, so ask where it was.
    if (!e.composedPath().includes(form)) close();
  });
  document.addEventListener("keydown", (e) => {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
      e.preventDefault();
      input.focus();
    }
  });
}
