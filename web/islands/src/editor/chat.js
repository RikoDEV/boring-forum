// Chat, laid out like Discourse's: a full page with channels and personal chats in a sidebar, or
// a drawer docked at the bottom corner that shows one view at a time.
import { dialog } from "../dialog.js";
import { LOCALE, announce, composing, el, jsonHeaders, fullDate, mk, mkc as make, tn, tr, userLink } from "../dom.js";
const QUICK = ["👍", "❤️", "😂"];
const EMOJI = ["👍", "❤️", "😂", "🎉", "😮", "😢", "🔥", "👏", "🙏", "🤔", "😀", "😍", "😎", "👀", "✅", "🚀"];
const GROUP_MINUTES = 5;

export function startChat(root, props, tools) {
  const { policy, iconButton, uploadFile } = tools;
  const drawer = Boolean(props.drawer);
  // The app's text field: the label sits in the box and floats up when it has a value.
  const field = (control, label) => {
    const box = make("label", "bf-field");
    if (control.tagName !== "SELECT") control.placeholder = " ";
    box.append(control, make("span", "bf-field-label", label));
    return box;
  };
  const button = (cls, text) => Object.assign(make("button", cls, text), { type: "button" });
  const glyphOf = (name) => iconButton(name, "").firstChild;
  const hashGlyph = () => {
    const g = glyphOf("hash");
    g.classList.add("chat-hash");
    return g;
  };
  const notice = () => mk("p", { className: "chat-error", hidden: true });
  const show = (p, text) => {
    p.textContent = text;
    p.hidden = false;
  };
  const icon = (name, title, run, cls = "") => {
    const b = iconButton(name, title);
    b.className = `chat-btn ${cls}`;
    if (run) b.addEventListener("click", run);
    return b;
  };
  // A control the member's trust level does not allow yet: disabled, and its tooltip says why.
  const gated = (b, tip) => {
    if (tip) {
      b.disabled = true;
      b.setAttribute("aria-label", tip);
    }
    return b;
  };
  const api = async (method, path, body) => {
    const res = await fetch(`/api/v1/chat${path}`, {
      method,
      headers: jsonHeaders(),
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    if (!res.ok) throw new Error((await res.json().catch(() => null))?.detail ?? tr("That did not work."));
    return res.status === 204 ? null : res.json();
  };
  const avatar = (name, size) => {
    const img = make("img", "avatar");
    img.src = `/avatar/${encodeURIComponent(name || "deleted")}.svg`;
    img.width = img.height = size;
    img.alt = "";
    img.loading = "lazy";
    return img;
  };

  let ignored = new Set();
  let myId = 0;
  let channels = [];
  let current = null;
  let replyTo = null;
  let editing = null;
  let live = null;
  let order = [];
  const byId = new Map();
  let filter = {};

  // --- Skeleton
  const shell = make("div", `chat-shell${drawer ? " chat-shell-drawer" : ""}`);
  const side = make("aside", "chat-sidebar");
  const main = make("section", "chat-channel");
  const head = make("header", "chat-channel-head");
  const scroller = make("div", "chat-scroll");
  const log = el("div", { class: "chat-log", role: "log", "aria-live": "off" });
  scroller.append(log);
  const down = icon("chevron-down", tr("Jump to the newest message"), () => toBottom(), "chat-to-bottom");
  down.hidden = true;
  const context = mk("div", { className: "chat-composer-context", hidden: true });
  const status = el("p", { class: "chat-error", role: "alert", hidden: "" });
  const joinBar = mk("div", { className: "chat-join-bar", hidden: true });
  const form = make("form", "chat-composer");
  const input = mk("textarea", { className: "chat-input", rows: 1, disabled: Boolean(props.needs?.write) });
  input.setAttribute("aria-label", tr("Message"));
  const picker = mk("input", { type: "file", hidden: true });
  const attach = icon("plus", tr("Upload a file"), () => picker.click());
  const emojiOpen = icon("smile", tr("Insert emoji"), (e) => {
    e.stopPropagation();
    emojiPanel(emojiOpen, (emoji) => {
      input.setRangeText(emoji, input.selectionStart, input.selectionEnd, "end");
      input.focus();
      syncInput();
    });
  });
  const sendButton = icon("arrow-up", tr("Send"), null, "chat-send");
  sendButton.type = "submit";
  sendButton.disabled = true;
  form.append(attach, input, picker, emojiOpen, sendButton);
  main.append(head, scroller, down, status, context, joinBar, form);
  shell.append(side, main);
  root.replaceChildren(shell);

  const say = (e) => {
    status.textContent = e?.message ?? "";
    status.hidden = !e;
  };
  const nameOf = (c) => (c.kind === "dm" ? c.people.join(", ") || props.me : c.name);
  const showList = () => shell.classList.remove("is-channel");

  // --- Popups
  let popup = null;
  const closePopup = () => {
    popup?.remove();
    popup = null;
  };
  document.addEventListener("click", closePopup);
  const placePopup = (el, anchor) => {
    closePopup();
    popup = el;
    el.addEventListener("click", (e) => e.stopPropagation());
    root.append(el);
    const a = anchor.getBoundingClientRect();
    const box = el.getBoundingClientRect();
    el.style.top = `${Math.max(8, a.top - box.height - 4 < 8 ? a.bottom + 4 : a.top - box.height - 4)}px`;
    el.style.left = `${Math.max(8, Math.min(innerWidth - box.width - 8, a.right - box.width))}px`;
  };
  function emojiPanel(anchor, choose) {
    const panel = make("div", "chat-popup chat-emoji");
    for (const e of EMOJI) {
      const b = button("", e);
      b.addEventListener("click", () => {
        closePopup();
        choose(e);
      });
      panel.append(b);
    }
    placePopup(panel, anchor);
  }
  function menu(anchor, items) {
    const panel = make("div", "chat-popup chat-menu");
    for (const [name, label, run, cls] of items) {
      const b = iconButton(name, label);
      b.className = `chat-menu-item ${cls ?? ""}`;
      b.append(make("span", "", label));
      b.addEventListener("click", () => {
        closePopup();
        run();
      });
      panel.append(b);
    }
    placePopup(panel, anchor);
  }
  // The site's shared dialog (islands dialog.js), sized for chat.
  function modal(title, fill) {
    const box = dialog({ title, className: "chat-modal" });
    fill(box.body, () => box.close());
    box.show();
  }

  // --- Sidebar
  function row(c) {
    const a = Object.assign(make("a", `chat-row${c.id === current?.id ? " is-active" : ""}`), {
      href: `/chat/${c.id}`,
    });
    a.append(c.kind === "dm" ? avatar(c.people[0] ?? props.me, 24) : hashGlyph());
    const name = make("span", "chat-row-name", nameOf(c));
    a.append(name);
    if (c.muted) a.classList.add("is-muted");
    if (c.unread && c.id !== current?.id && !c.muted) {
      a.classList.add("is-unread");
      a.append(
        c.kind === "dm"
          ? make("span", "chat-count", String(c.unread))
          : Object.assign(make("span", "chat-dot"), { title: `${c.unread} unread` }),
      );
    }
    a.addEventListener("click", (e) => {
      if (e.ctrlKey || e.metaKey || e.shiftKey) return;
      e.preventDefault();
      open(c.id, true).catch(say);
    });
    return a;
  }
  function section(title, add, rows, empty) {
    const box = make("div", "chat-section");
    box.append(
      mk("div", { className: "chat-section-head" }, make("span", "", title), add),
      ...(rows.length ? rows : [make("p", "chat-empty", empty)]),
    );
    return box;
  }
  async function loadChannels() {
    channels = await api("GET", "/channels");
    const joined = channels.filter((c) => c.kind === "public" && c.joined);
    const dms = channels.filter((c) => c.kind === "dm");
    side.replaceChildren();
    if (drawer) side.append(drawerHead(tr("Chat")));
    const lists = make("div", "chat-sections");
    lists.append(
      section(tr("Channels"), channelTools(), joined.map(row), tr("Browse channels to join one.")),
      section(
        tr("Personal chat"),
        gated(icon("plus", tr("New message"), newMessage), props.needs?.dm),
        dms.map(row),
        tr("No personal chats yet."),
      ),
    );
    side.append(lists);
  }
  function channelTools() {
    return mk(
      "span",
      { className: "chat-section-tools" },
      icon("list", tr("Browse channels"), browse),
      gated(icon("plus", tr("New channel"), newChannel), props.needs?.create),
    );
  }
  function newChannel() {
    modal(tr("New invite-only channel"), (body, close) => {
      const name = make("input");
      const about = make("input");
      const problem = notice();
      const go = button("btn primary", tr("Create channel"));
      go.addEventListener("click", async () => {
        try {
          const { id, held, message } = await api("POST", "/channels", { name: name.value, description: about.value });
          if (held) return show(problem, message);
          close();
          await open(id, true);
          manageMembers();
        } catch (e) {
          show(
            problem,
            e.message === tr("That did not work.")
              ? tr("Your trust level is too low to start a channel yet.")
              : e.message,
          );
        }
      });
      body.append(field(name, tr("Name")), field(about, tr("Description")), problem, go);
      name.focus();
    });
  }
  function manageMembers() {
    const c = current;
    modal(tr("People in {channel}", { channel: c.name }), (body) => {
      const input = mk("input", { autocomplete: "off" });
      const list = make("div", "chat-members");
      const problem = notice();
      const fail = (e) => show(problem, e.message);
      const refresh = async () => {
        const people = await api("GET", `/channels/${c.id}/members`);
        list.replaceChildren(
          ...people.map((p) => {
            const line = make("div", "chat-row");
            line.append(avatar(p.username, 24), mk("span", { className: "chat-row-name" }, userLink(p.username)));
            if (p.username !== props.me)
              line.append(
                icon("x", tr("Remove {name}", { name: p.username }), () =>
                  api("DELETE", `/channels/${c.id}/members/${p.id}`).then(refresh).catch(fail),
                ),
              );
            return line;
          }),
        );
      };
      input.addEventListener("keydown", (e) => {
        if (e.key !== "Enter" || composing(e) || !input.value.trim()) return;
        e.preventDefault();
        problem.hidden = true;
        api("POST", `/channels/${c.id}/members`, { username: input.value })
          .then(() => {
            input.value = "";
            return refresh();
          })
          .catch(fail);
      });
      body.append(field(input, tr("Add someone: type a username and press Enter")), problem, list);
      refresh().catch(fail);
      input.focus();
    });
  }
  function browse() {
    modal(tr("Browse channels"), (body, close) => {
      const list = channels.filter((c) => c.kind === "public");
      if (!list.length) body.append(make("p", "chat-empty", tr("An administrator has not added any channels yet.")));
      for (const c of list) {
        const line = make("div", "chat-browse-row");
        const text = make("div", "chat-browse-text");
        text.append(make("strong", "", `# ${c.name}`));
        if (c.description) text.append(make("span", "", c.description));
        const go = button(c.joined ? "btn" : "btn primary", c.joined ? tr("Open") : tr("Join"));
        go.addEventListener("click", async () => {
          try {
            if (!c.joined) await api("POST", `/channels/${c.id}/join`);
            close();
            await open(c.id, true);
          } catch (e) {
            say(e);
          }
        });
        line.append(text, go);
        body.append(line);
      }
    });
  }
  function newMessage() {
    modal(tr("New message"), (body, close) => {
      const chosen = [];
      const box = make("input");
      const people = mk("div", { className: "chat-people" }, box);
      const found = make("div", "chat-people-found");
      const start = button("btn primary", tr("Start chat"));
      start.disabled = true;
      const add = (name) => {
        if (chosen.includes(name)) return;
        chosen.push(name);
        const pill = button("chat-person");
        pill.append(avatar(name, 20), name, " ×");
        pill.addEventListener("click", () => {
          chosen.splice(chosen.indexOf(name), 1);
          pill.remove();
          start.disabled = !chosen.length;
        });
        box.before(pill);
        box.value = "";
        found.replaceChildren();
        start.disabled = false;
        box.focus();
      };
      let timer;
      box.addEventListener("input", () => {
        clearTimeout(timer);
        timer = setTimeout(async () => {
          const q = box.value.trim();
          if (!q) return found.replaceChildren();
          const res = await fetch(`/api/v1/autocomplete?kind=user&q=${encodeURIComponent(q)}`).catch(() => null);
          const list = res?.ok ? await res.json() : [];
          found.replaceChildren(
            ...list.slice(0, 6).map((u) => {
              const name = typeof u === "string" ? u : (u.username ?? u.label ?? u.name);
              const b = button("chat-row");
              b.append(avatar(name, 24), make("span", "chat-row-name", name));
              b.addEventListener("click", () => add(name));
              return b;
            }),
          );
        }, 200);
      });
      box.addEventListener("keydown", (e) => {
        if (e.key === "Enter" && !composing(e)) {
          e.preventDefault();
          if (box.value.trim()) add(box.value.trim().replace(/^@/, ""));
          else if (chosen.length) start.click();
        }
      });
      const problem = notice();
      start.addEventListener("click", async () => {
        try {
          const { id } = await api("POST", "/dm", { usernames: chosen });
          close();
          await open(id, true);
        } catch (e) {
          show(problem, e.message);
        }
      });
      const named = make("label", "bf-field");
      box.placeholder = " ";
      named.append(people, make("span", "bf-field-label", tr("Type a username")));
      body.append(named, found, problem, start);
      box.focus();
    });
  }

  // --- Messages
  const sameDay = (a, b) => new Date(a).toDateString() === new Date(b).toDateString();
  const clock = (iso) => new Date(iso).toLocaleTimeString(LOCALE(), { hour: "2-digit", minute: "2-digit" });
  const dayName = (iso) => {
    const d = new Date(iso);
    const today = new Date();
    const yesterday = new Date(today.getTime() - 864e5);
    if (d.toDateString() === today.toDateString()) return tr("Today");
    if (d.toDateString() === yesterday.toDateString()) return tr("Yesterday");
    return d.toLocaleDateString(LOCALE(), {
      month: "long",
      day: "numeric",
      year: d.getFullYear() === today.getFullYear() ? undefined : "numeric",
    });
  };
  const followsOn = (m, prev) =>
    prev &&
    !m.reply_to_id &&
    !ignored.has(prev.username) &&
    !prev.deleted &&
    !m.deleted &&
    prev.user_id === m.user_id &&
    sameDay(prev.created_at, m.created_at) &&
    new Date(m.created_at).getTime() - new Date(prev.created_at).getTime() < GROUP_MINUTES * 6e4;

  function hoverBar(m) {
    const bar = make("div", "chat-actions");
    for (const e of QUICK) {
      const b = Object.assign(button("chat-btn", e), { title: tr("React with {emoji}", { emoji: e }) });
      b.addEventListener("click", () => react(m.id, e));
      bar.append(b);
    }
    const more = icon("smile", tr("Add a reaction"), (e) => {
      e.stopPropagation();
      emojiPanel(more, (emoji) => react(m.id, emoji));
    });
    bar.append(
      more,
      icon("reply", tr("Reply"), () => startReply(m)),
    );
    const options = icon("ellipsis", tr("More"), (e) => {
      e.stopPropagation();
      const mine = m.username === props.me;
      /** @type {[string, string, () => unknown, string?][]} */
      const items = [
        [
          "link",
          tr("Copy link"),
          () => navigator.clipboard?.writeText(`${location.origin}/chat/${m.channel_id}#m${m.id}`),
        ],
      ];
      if (mine) items.push(["pencil", tr("Edit"), () => startEdit(m)]);
      if (props.staff)
        items.push(["pin", m.pinned ? tr("Unpin") : tr("Pin"), () => api("POST", `/messages/${m.id}/pin`).catch(say)]);
      if (!mine) items.push(["flag", tr("Flag"), () => flagMessage(m)]);
      if (mine || props.staff)
        items.push(["trash-2", tr("Delete"), () => api("DELETE", `/messages/${m.id}`).catch(say), "is-danger"]);
      menu(options, items);
    });
    bar.append(options);
    return bar;
  }
  const react = (id, emoji) => api("POST", `/messages/${id}/react`, { emoji }).catch(say);
  function flagMessage(m) {
    modal(tr("Flag this message"), (body, close) => {
      const area = mk("textarea", { rows: 3 });
      const go = button("btn primary", tr("Flag"));
      go.addEventListener("click", () => {
        api("POST", `/messages/${m.id}/flag`, { reason: area.value }).then(() => say(null), say);
        close();
      });
      body.append(field(area, tr("What is wrong with this message?")), go);
    });
  }
  function renderMessage(m, prev) {
    const wrap = document.createDocumentFragment();
    if (!prev || !sameDay(prev.created_at, m.created_at)) {
      wrap.append(mk("div", { className: "chat-date" }, make("span", "", dayName(m.created_at))));
    }
    if (ignored.has(m.username) && !m.deleted) {
      const text = make("div", "cooked chat-text");
      text.innerHTML = policy.createHTML(m.cooked);
      const box = make("details", "chat-ignored bf-disclosure is-small");
      box.append(make("summary", "", tr("Message from {name}, who you ignore", { name: m.username })), text);
      wrap.append(mk("article", { className: "chat-message", id: `m${m.id}` }, box));
      return wrap;
    }
    const compact = followsOn(m, prev);
    const el = mk("article", {
      className: `chat-message${compact ? " is-followup" : ""}${m.pinned ? " is-pinned" : ""}${m.deleted ? " is-deleted" : ""}`,
      id: `m${m.id}`,
    });
    if (m.deleted) {
      el.append(
        make(
          "div",
          "chat-deleted",
          m.username ? tr("A message by {name} was deleted", { name: m.username }) : tr("A message was deleted"),
        ),
      );
      wrap.append(el);
      return wrap;
    }
    if (m.reply_to_id) {
      const quote = make("div", "chat-reply");
      quote.append(
        glyphOf("reply"),
        avatar(m.reply_author, 20),
        make("span", "chat-reply-text", m.reply_text ?? tr("Deleted message")),
      );
      el.append(quote);
    }
    const line = make("div", "chat-line");
    const gutter = make("div", "chat-gutter");
    // Each time shows its full date on hover.
    const stamp = (cls) =>
      Object.assign(make("time", cls, clock(m.created_at)), { dateTime: m.created_at, title: fullDate(m.created_at) });
    if (compact) gutter.append(stamp("chat-time-hover"));
    else gutter.append(avatar(m.username, 32));
    const body = make("div", "chat-body");
    if (!compact) {
      const meta = make("div", "chat-meta");
      const author = userLink(m.username ?? "deleted", { class: "chat-author bf-user-link" });
      meta.append(author, stamp("chat-time"));
      if (m.pinned) meta.append(make("span", "chat-time", tr("Pinned")));
      body.append(meta);
    }
    const text = make("div", "cooked chat-text");
    text.innerHTML = policy.createHTML(m.cooked); // cooked and sanitized by the server
    if (m.edited) {
      const last = text.lastElementChild ?? text;
      last.append(make("span", "chat-edited", " (edited)"));
    }
    body.append(text);
    if (m.reactions.length) {
      const pills = make("div", "chat-reactions");
      for (const r of m.reactions) {
        const b = Object.assign(
          button(`chat-reaction${r.users.includes(props.me) ? " is-mine" : ""}`, `${r.emoji} ${r.count}`),
          { title: r.users.join(", ") },
        );
        b.addEventListener("click", () => react(m.id, r.emoji));
        pills.append(b);
      }
      body.append(pills);
    }
    line.append(gutter, body);
    el.append(line);
    if (!current.archived) el.append(hoverBar(m));
    wrap.append(el);
    return wrap;
  }
  function paint() {
    const made = document.createDocumentFragment();
    if (older) made.append(older);
    order.forEach((id, i) => made.append(renderMessage(byId.get(id), byId.get(order[i - 1]))));
    log.replaceChildren(made);
  }
  let older = null;
  const atBottom = () => scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 80;
  function toBottom() {
    scroller.scrollTop = scroller.scrollHeight;
    down.hidden = true;
  }
  scroller.addEventListener("scroll", () => (down.hidden = atBottom()));

  async function loadMessages(before) {
    const params = new URLSearchParams({ ...filter, ...(before ? { before } : {}) });
    const page = await api("GET", `/channels/${current.id}/messages?${params}`);
    if (!before) {
      byId.clear();
      order = [];
    }
    const ids = [...page].reverse().map((m) => {
      byId.set(m.id, m);
      return m.id;
    });
    order = [...ids, ...order];
    older = null;
    if (page.length === 50) {
      older = button("chat-older", tr("Load older messages"));
      const oldest = order[0];
      older.addEventListener("click", () => loadMessages(oldest).catch(say));
    }
    const height = scroller.scrollHeight;
    paint();
    if (before) scroller.scrollTop = scroller.scrollHeight - height;
    else if (!Object.keys(filter).length) toBottom();
    return page;
  }
  function upsert(m) {
    const keep = atBottom() || m.username === props.me;
    if (!byId.has(m.id)) order.push(m.id);
    byId.set(m.id, m);
    paint();
    if (keep) toBottom();
    else down.hidden = false;
  }

  // --- Composer
  // The box grows with its text by CSS (`field-sizing: content`, capped by max-height in chat.css);
  // browsers without it are sized here.
  const sizesItself = CSS.supports("field-sizing", "content");
  function syncInput() {
    if (!sizesItself) {
      input.style.height = "auto";
      input.style.height = `${Math.min(input.scrollHeight, 160)}px`;
    }
    sendButton.disabled = !input.value.trim();
  }
  input.addEventListener("input", syncInput);
  function setContext(glyph, text, cancel) {
    context.replaceChildren(glyphOf(glyph), make("span", "chat-context-text", text), icon("x", tr("Cancel"), cancel));
    context.hidden = false;
  }
  function clearContext() {
    replyTo = null;
    editing = null;
    context.hidden = true;
  }
  function startReply(m) {
    editing = null;
    replyTo = m.id;
    setContext("reply", `${m.username ?? tr("deleted member")}: ${m.raw.slice(0, 120)}`, clearContext);
    input.focus();
  }
  function startEdit(m) {
    replyTo = null;
    editing = m.id;
    setContext("pencil", tr("Editing message"), () => {
      clearContext();
      input.value = "";
      syncInput();
    });
    input.value = m.raw;
    syncInput();
    input.focus();
  }
  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    const raw = input.value.trim();
    if (!raw || !current) return;
    try {
      const m = await (editing
        ? api("PUT", `/messages/${editing}`, { raw })
        : api("POST", `/channels/${current.id}/messages`, { raw, reply_to: replyTo }));
      const held = m?.held ? m : null;
      if (!held) upsert(m);
      if (!editing && !current.joined) {
        current.joined = true;
        await loadChannels();
        renderHead();
        joinBar.hidden = true;
      }
      input.value = "";
      clearContext();
      say(held ? { message: held.message } : null);
      syncInput();
      toBottom();
    } catch (err) {
      say(err);
    }
  });
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey && !composing(e)) {
      e.preventDefault();
      form.requestSubmit();
    } else if (e.key === "Escape" && !context.hidden) {
      // Cancelling an edit drops its text too, or Enter would post it as a new message.
      if (editing) input.value = "";
      clearContext();
    } else if (e.key === "ArrowUp" && !input.value) {
      // As in Discourse: the up arrow edits your last message.
      const mine = [...order]
        .reverse()
        .map((id) => byId.get(id))
        .find((m) => m.username === props.me && !m.deleted);
      if (mine) {
        e.preventDefault();
        startEdit(mine);
      }
    }
  });
  picker.addEventListener("change", async () => {
    const file = picker.files[0];
    picker.value = "";
    if (!file) return;
    try {
      const md = await uploadFile(file, () => {}, new AbortController().signal, "chat");
      input.setRangeText(`${md}\n`, input.selectionStart, input.selectionEnd, "end");
      syncInput();
      input.focus();
    } catch (err) {
      say(err);
    }
  });

  // --- Channel
  function drawerHead(title, back) {
    const top = make("header", "chat-drawer-head");
    if (back) top.append(icon("chevron-right", tr("Back"), showListView, "chat-back"));
    top.append(
      make("strong", "chat-drawer-title", title),
      icon("maximize-2", tr("Full page"), () => (location.href = current ? `/chat/${current.id}` : "/chat")),
      icon("x", tr("Close"), () => props.onClose?.()),
    );
    return top;
  }
  function showListView() {
    live?.close();
    current = null;
    showList();
    props.onChannel?.(0);
    loadChannels().catch(say);
  }
  function renderHead() {
    const c = current;
    head.replaceChildren();
    const title = make("div", "chat-title");
    title.append(c.kind === "dm" ? avatar(c.people[0] ?? props.me, 24) : hashGlyph());
    const names = make("div", "chat-title-text");
    names.append(make("strong", "", nameOf(c)));
    if (c.description) names.append(make("span", "", c.description));
    title.append(names);
    const tools = make("div", "chat-head-tools");
    const search = mk("input", { type: "search" });
    const searchBox = field(search, tr("Search this channel"));
    searchBox.classList.add("chat-search");
    searchBox.hidden = true;
    search.addEventListener("change", () => {
      filter = search.value.trim() ? { q: search.value.trim() } : {};
      loadMessages().catch(say);
    });
    tools.append(
      searchBox,
      icon("search", tr("Search this channel"), () => {
        searchBox.hidden = !searchBox.hidden;
        if (!searchBox.hidden) search.focus();
        else if (filter.q) {
          filter = {};
          search.value = "";
          loadMessages().catch(say);
        }
      }),
    );
    const pinned = icon("pin", tr("Pinned messages"), () => {
      filter = filter.pinned ? {} : { pinned: "true" };
      pinned.setAttribute("aria-pressed", String(Boolean(filter.pinned)));
      loadMessages().catch(say);
    });
    tools.append(pinned);
    if (c.kind === "public" && c.joined) {
      const settings = icon("settings", tr("Channel settings"), (e) => {
        e.stopPropagation();
        const mine = c.access === "invite" && (props.staff || c.created_by === myId);
        const act = (path, body) => () =>
          api("POST", `/channels/${c.id}/${path}`, body)
            .then(() => open(c.id))
            .catch(say);
        menu(settings, [
          ...(mine ? [["users", tr("Manage people"), manageMembers]] : []),
          [c.muted ? "bell" : "eye-off", c.muted ? tr("Unmute") : tr("Mute"), act("mute", { muted: !c.muted })],
          ["log-out", tr("Leave channel"), act("leave"), "is-danger"],
        ]);
      });
      tools.append(settings);
    }
    if (drawer) {
      head.className = "chat-channel-head chat-drawer-head";
      head.append(icon("chevron-right", tr("Back"), showListView, "chat-back"), title, tools);
      tools.append(
        icon("maximize-2", tr("Full page"), () => (location.href = `/chat/${c.id}`)),
        icon("x", tr("Close"), () => props.onClose?.()),
      );
    } else {
      head.append(title, tools);
    }
  }
  async function open(id, push) {
    say(null);
    filter = {};
    await loadChannels();
    current = channels.find((c) => c.id === id) ?? null;
    if (!current) {
      // Gone, for example after leaving an invite-only channel.
      live?.close();
      live = null;
      showListView();
      return;
    }
    await loadChannels(); // highlights the open one
    shell.classList.add("is-channel");
    if (push && !drawer) history.pushState(null, "", `/chat/${id}`);
    props.onChannel?.(id);
    clearContext();
    renderHead();
    const label = current.kind === "dm" ? nameOf(current) : `#${current.name}`;
    input.placeholder = tr("Chat with {name}", { name: label });
    form.hidden = current.archived;
    joinBar.hidden = !current.archived;
    if (current.archived) joinBar.textContent = tr("This channel is archived.");
    const page = await loadMessages();
    markRead(page[0]?.id);
    connect();
    if (!current.archived && !matchMedia("(pointer: coarse)").matches) input.focus();
  }
  const markRead = (id) => {
    if (current && id && (current.joined || current.kind === "dm")) {
      api("POST", `/channels/${current.id}/read`, { id })
        .then(loadChannels)
        .catch(() => {});
    }
  };
  // The log is not read out message by message: new ones from others are counted and announced
  // once the chatter pauses.
  let pending = 0;
  let announceTimer = 0;
  function announceLater() {
    pending += 1;
    clearTimeout(announceTimer);
    announceTimer = setTimeout(() => {
      announce(tn("js.count.new_chat_messages", pending));
      pending = 0;
    }, 2500);
  }
  function connect() {
    live?.close();
    if (typeof EventSource === "undefined") return;
    live = new EventSource(`/api/v1/live?channels=user,chat:${current.id}`);
    live.addEventListener("message", (e) => {
      const m = JSON.parse(e.data).data;
      // A search or the pinned list shows only its own results.
      if (Object.keys(filter).length) return;
      const isNew = !byId.has(m.id) && m.user_id !== myId;
      upsert(m);
      if (isNew) announceLater();
      if (document.visibilityState === "visible") markRead(m.id);
    });
    live.addEventListener("changed", (e) => {
      const m = JSON.parse(e.data).data;
      if (!Object.keys(filter).length || byId.has(m.id)) upsert(m);
    });
    live.addEventListener("chat", () => loadChannels().catch(() => {}));
  }
  addEventListener("popstate", () => {
    const m = /\/chat\/(\d+)/.exec(location.pathname);
    if (m && !drawer) open(+m[1]).catch(say);
  });

  api("GET", "/me")
    .then((me) => {
      ignored = new Set(me.ignored);
      myId = me.id;
      attach.hidden = !me.uploads;
      // Only the kinds chat takes are offered in the file picker.
      const ACCEPT = {
        png: "image/png",
        jpg: "image/jpeg",
        gif: "image/gif",
        webp: "image/webp",
        pdf: "application/pdf",
        zip: ".zip,application/zip",
        txt: ".txt,text/plain",
      };
      if (me.upload_types?.length) picker.accept = me.upload_types.map((t) => ACCEPT[t]).join(",");
      return loadChannels();
    })
    .then(() => {
      const joined = channels.filter((c) => c.joined || c.kind === "dm");
      const first = props.channel || (drawer ? 0 : joined[0]?.id);
      if (first) return open(first, !props.channel);
      if (!drawer) {
        form.hidden = true;
        head.replaceChildren(make("strong", "", tr("Chat")));
        log.replaceChildren(make("p", "chat-empty", tr("Join a channel or start a personal chat from the sidebar.")));
      }
    })
    .catch(say);
  return { close: () => live?.close() };
}
