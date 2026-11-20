// Member cards: hovering (or focusing) a link to a profile opens a card with who they are, how
// active they are, their latest badges, and Follow / Message / Profile. The card stays while the
// pointer is on it, so its buttons can be used; Escape or moving away closes it.
import { announce, countParts, csrf, el, icon, tn, toast, tr, userLink, when } from "./dom.js";

/// A badge medal as the profile draws it: a coloured shape for its tier, its icon, and its number.
function medal({ tier, icon: name, color = 0, level = 0, rare = false, label }) {
  const wrap = el("span", { class: "bf-medal-stacked", role: "listitem", "data-tip": label });
  if (rare) wrap.append(el("span", { class: "bf-medal-rare" }, tr("Rare")));
  const m = el("span", { class: `bf-medal bf-medal-${["gold", "silver"].includes(tier) ? tier : "bronze"}` });
  const shape = el("span", { class: `bf-medal-shape bf-medal-c${(((color | 0) % 10) + 10) % 10}` });
  shape.append(icon(name));
  m.append(shape);
  if (level > 0) m.append(el("span", { class: "bf-medal-level" }, level >= 1000 && level % 1000 === 0 ? `${level / 1000}k` : String(level)));
  wrap.append(m);
  return wrap;
}

// What plugins draw in the card is HTML the server has already sanitized.
const pluginPolicy = window.trustedTypes?.createPolicy("bf-plugin", { createHTML: (s) => s }) ?? {
  createHTML: (s) => s,
};

const cache = new Map();
const load = (name) => {
  if (!cache.has(name)) {
    cache.set(
      name,
      fetch(`/api/v1/users/${encodeURIComponent(name)}/card`, { headers: { Accept: "application/json" } })
        .then((r) => (r.ok ? r.json() : null))
        .catch(() => null),
    );
  }
  return cache.get(name);
};
const count = (n) => (n >= 10000 ? `${Math.round(n / 1000)}k` : n >= 1000 ? `${(n / 1000).toFixed(1)}k` : String(n));

/** A figure over its words: `stat(12, "count.posts")` is 12 over "posts", in the page's language. */
function stat(value, key) {
  const s = el("div", { class: "bf-hc-stat" });
  const [before, after] = countParts(key, value);
  s.append(el("strong", {}, count(value)), el("span", {}, `${before} ${after}`.trim()));
  return s;
}

function build(u) {
  const card = el("div", { class: "hovercard", role: "dialog", "aria-label": u.display_name || u.username });

  // Cover, or the accent wash when there is none; the avatar overlaps its lower edge.
  const cover = el("div", { class: "bf-hc-cover" });
  if (u.cover) {
    const img = el("img", { src: u.cover, alt: "" });
    img.style.objectPosition = `50% ${u.cover_y ?? 50}%`;
    cover.append(img);
  }
  const top = el("div", { class: "bf-hc-top" });
  const pic = el("a", { class: "bf-hc-avatar", href: u.url, tabindex: "-1" });
  pic.append(
    el("img", {
      class: "avatar",
      src: `/avatar/${encodeURIComponent(u.username)}.svg`,
      width: "72",
      height: "72",
      alt: "",
    }),
  );
  if (u.trust) {
    // The trust level sits on the avatar; its border says whether they are online.
    const lvl = el("span", {
      class: "bf-lvl",
      "data-online": u.online ? "1" : "0",
      "data-tip": `${tr("Trust level {n}: {name}", { n: u.trust.level, name: u.trust.name })} · ${u.online ? tr("Online") : tr("Offline")}`,
    });
    lvl.textContent = String(u.trust.level);
    pic.append(lvl);
  } else if (u.online) {
    pic.append(el("span", { class: "bf-online", role: "img", "aria-label": tr("Online") }));
  }
  const who = el("div", { class: "bf-hc-who" });
  const name = el("a", { class: "bf-hc-name", href: u.url }, u.display_name || u.username);
  who.append(name);
  const line = el("div", { class: "bf-hc-line" });
  if (u.display_name) line.append(el("span", { class: "muted" }, `@${u.username}`));
  const group = el("span", { class: "bf-hc-group" }, u.group);
  if (u.group_colour) group.style.setProperty("--bf-hc-group", u.group_colour);
  line.append(group);
  who.append(line);
  if (u.title) who.append(el("div", { class: "bf-hc-title" }, u.title));
  top.append(pic, who);
  card.append(cover, top);

  const body = el("div", { class: "bf-hc-body" });
  if (u.status) body.append(el("p", { class: "bf-hc-status" }, u.status));
  if (u.bio) body.append(el("p", { class: "bf-hc-bio" }, u.bio));
  const meta = el("ul", { class: "bf-hc-meta" });
  const fact = (i, ...parts) => {
    const li = el("li");
    const span = el("span");
    span.append(...parts);
    li.append(icon(i), span);
    meta.append(li);
  };
  if (u.location) fact("map-pin", u.location);
  fact("calendar", `${tr("Joined")} `, when(u.joined, u.joined_at, { "data-format": "month" }));
  if (u.online) fact("clock", tr("Online now"));
  else if (u.last_seen) fact("clock", `${tr("Last seen")} `, when(u.last_seen, u.last_seen_at));
  body.append(meta);

  const stats = el("div", { class: "bf-hc-stats" });
  stats.append(stat(u.posts, "count.posts"), stat(u.likes, "count.likes"));
  if (u.solved) stats.append(stat(u.solved, "count.solutions"));
  const followers = u.followers != null ? stat(u.followers, "count.followers") : null;
  if (followers) stats.append(followers);
  body.append(stats);

  if (u.rank || u.badges?.length) {
    // The same medals as the profile's latest badges, a little smaller: the reputation rank first.
    const medals = el("div", { class: "bf-hc-medals", role: "list", "aria-label": tr("Achievements") });
    if (u.rank) medals.append(medal({ tier: u.rank_tier, icon: "star", color: u.rank_color, label: u.rank }));
    for (const b of u.badges ?? []) medals.append(medal({ ...b, label: b.name }));
    body.append(medals);
  }
  if (u.slot_html) {
    const extra = el("div", { class: "bf-hc-plugin" });
    extra.innerHTML = pluginPolicy.createHTML(u.slot_html);
    body.append(extra);
  }
  card.append(body);

  const actions = el("div", { class: "bf-hc-actions" });
  if (u.following != null) {
    const follow = el("button", {
      type: "button",
      class: `btn${u.following ? "" : " primary"}`,
      "aria-pressed": String(u.following),
    });
    const paint = () => {
      follow.replaceChildren(
        icon(u.following ? "user-check" : "user-plus"),
        document.createTextNode(` ${u.following ? tr("Following") : tr("Follow")}`),
      );
      follow.className = `btn${u.following ? "" : " primary"}`;
      follow.setAttribute("aria-pressed", String(u.following));
    };
    paint();
    follow.addEventListener("click", async () => {
      follow.disabled = true;
      const res = await fetch(`/u/${u.id}/follow`, {
        method: "POST",
        body: new URLSearchParams({ csrf: csrf() }),
        redirect: "manual",
      }).catch(() => null);
      follow.disabled = false;
      if (!res || !(res.ok || res.type === "opaqueredirect")) {
        toast(tr("That did not work. Try again."), true);
        return;
      }
      u.following = !u.following;
      if (followers) {
        u.followers += u.following ? 1 : -1;
        followers.querySelector("strong").textContent = count(u.followers);
        const [before, after] = countParts("count.followers", u.followers);
        followers.querySelector("span").textContent = `${before} ${after}`.trim();
      }
      paint();
      announce(
        u.following
          ? tr("Following {name}", { name: u.username })
          : tr("Stopped following {name}", { name: u.username }),
      );
    });
    actions.append(follow);
  }
  if (u.can_message) {
    const msg = el("a", { class: "btn", href: `/messages?to=${encodeURIComponent(u.username)}` });
    msg.append(icon("mail"), document.createTextNode(` ${tr("Message")}`));
    actions.append(msg);
  }
  const profile = el("a", { class: "btn bf-hc-profile", href: u.url });
  profile.append(document.createTextNode(`${u.own ? tr("Your profile") : tr("Profile")} `), icon("chevron-right"));
  actions.append(profile);
  card.append(actions);
  return card;
}

// Topic previews. A link to a topic (`/t/slug/id`, maybe with a post number or `/unread`) gets
// one wherever it appears — lists, search, profiles, notifications, links in posts — except:
// links into the topic being read, reply and quote links that have their own preview, and page
// furniture (breadcrumbs, pagers, post headers, the topic's own header).
const TOPIC_HREF = /^\/t\/[^/?#]+\/(\d+)(?:\/(?:\d+|unread|last))?\/?(?:[?#].*)?$/;
const NOT_TOPIC_CARD = ".crumbs, .pager, .post-head, .bf-topic-head, blockquote, .bf-dialog, [data-context], a.unread";
const here = () => TOPIC_HREF.exec(location.pathname)?.[1];
function topicOf(a) {
  const id = TOPIC_HREF.exec(a.getAttribute("href"))?.[1];
  if (!id || id === here() || a.closest(NOT_TOPIC_CARD)) return null;
  return id;
}

const topics = new Map();
const loadTopic = (id) => {
  if (!topics.has(id)) {
    topics.set(
      id,
      fetch(`/api/v1/topics/${id}/card`, { headers: { Accept: "application/json" } })
        .then((r) => (r.ok ? r.json() : null))
        .catch(() => null),
    );
  }
  return topics.get(id);
};

function buildTopic(t) {
  const card = el("div", { class: "hovercard bf-topic-card", role: "dialog", "aria-label": t.title });
  if (t.image) {
    const pic = el("div", { class: "bf-tc-image" });
    pic.append(el("img", { src: t.image, alt: "", loading: "lazy" }));
    card.append(pic);
  }
  const body = el("div", { class: "bf-tc-body" });

  // Where it is, and what state it is in.
  const where = el("div", { class: "bf-tc-where" });
  where.append(el("a", { class: "bf-tc-forum", href: t.forum_url }, t.forum));
  const flag = (cls, i, text) => {
    const s = el("span", { class: `bf-badge ${cls}` });
    s.append(icon(i), document.createTextNode(` ${text}`));
    where.append(s);
  };
  if (t.pinned) flag("is-accent", "pin", tr("Pinned"));
  if (t.solved) flag("is-ok", "circle-check", tr("Solved"));
  if (t.locked) flag("", "lock", tr("Locked"));
  body.append(where);

  const title = el("a", { class: "bf-tc-title", href: t.url });
  if (t.prefix) title.append(el("span", { class: "bf-badge is-accent" }, t.prefix), document.createTextNode(" "));
  title.append(document.createTextNode(t.title));
  body.append(title);

  const by = el("div", { class: "bf-tc-by" });
  by.append(
    el("img", {
      class: "avatar",
      src: `/avatar/${encodeURIComponent(t.author ?? "deleted")}.svg`,
      width: "20",
      height: "20",
      alt: "",
    }),
  );
  const started = el("span", { class: "muted" }, " · ");
  started.append(when(t.started, t.started_at));
  by.append(t.author ? userLink(t.author) : document.createTextNode(tr("deleted member")), started);
  body.append(by);

  if (t.gist) body.append(el("p", { class: "bf-tc-excerpt bf-tc-gist", "data-tip": tr("A summary made by AI") }, t.gist));
  else if (t.excerpt) body.append(el("p", { class: "bf-tc-excerpt" }, t.excerpt));
  if (t.tags?.length) {
    const tags = el("div", { class: "bf-tc-tags" });
    for (const name of t.tags) tags.append(el("span", { class: "bf-badge" }, name));
    body.append(tags);
  }

  const stats = el("div", { class: "bf-hc-stats bf-tc-stats" });
  stats.append(stat(t.replies, "count.replies"), stat(t.views, "count.views"));
  stats.append(t.votes != null ? stat(t.votes, "count.votes") : stat(t.likes, "count.likes"));
  stats.append(stat(t.participants, "count.people"));
  body.append(stats);
  card.append(body);

  const foot = el("div", { class: "bf-hc-actions bf-tc-foot" });
  if (t.last_poster && t.replies > 0) {
    const last = el("span", { class: "bf-tc-last small muted" }, `${tr("Last reply by")} `);
    last.append(userLink(t.last_poster), ", ", when(t.last_posted, t.last_posted_at));
    foot.append(last);
  }
  if (t.unread) {
    const jump = el("a", { class: "btn primary", href: t.unread_url });
    jump.append(document.createTextNode(`${tn("js.count.new", t.unread)} `), icon("chevron-right"));
    foot.append(jump);
  } else {
    const go = el("a", { class: "btn", href: t.url });
    go.append(document.createTextNode(`${tr("Open")} `), icon("chevron-right"));
    foot.append(go);
  }
  card.append(foot);
  return card;
}

// Below the link, or above it when there is no room; always inside the window.
function place(card, anchor) {
  const r = anchor.getBoundingClientRect();
  const w = card.offsetWidth;
  const h = card.offsetHeight;
  const left = Math.min(Math.max(8, r.left), innerWidth - w - 8);
  const below = r.bottom + 8 + h <= innerHeight || r.top < h + 8;
  card.style.left = `${scrollX + left}px`;
  card.style.top = `${scrollY + (below ? r.bottom + 8 : r.top - h - 8)}px`;
}

export function hovercards() {
  let card = null;
  let anchor = null;
  let openTimer;
  let closeTimer;
  const close = () => {
    clearTimeout(openTimer);
    clearTimeout(closeTimer);
    card?.remove();
    card = null;
    anchor = null;
  };
  // Never closes under the pointer. Focus leaving the card also closes it (for keyboard use),
  // but a click inside the card moves focus too: on its text, or on Follow, which is disabled
  // while the request runs. The pointer is still on the card then, so it stays.
  const closeSoon = () => {
    clearTimeout(openTimer);
    clearTimeout(closeTimer);
    closeTimer = setTimeout(() => {
      if (card?.matches(":hover") || anchor?.matches(":hover")) return;
      close();
    }, 250);
  };
  // What a link is a card for: [anchor, kind, key], or null. Members by name; topics by id.
  const target = (e) => {
    const a = e.target.closest?.('a[href^="/u/"], a[href^="/t/"]');
    if (!a || a.closest(".hovercard, .bf-menu, nav") || !a.getClientRects().length) return null;
    const href = a.getAttribute("href");
    if (href.startsWith("/u/")) {
      const name = decodeURIComponent(href.split(/[/?#]/)[2] ?? "").replace(/^\d+-/, "");
      return name ? [a, "user", name] : null;
    }
    const id = topicOf(a);
    return id ? [a, "topic", id] : null;
  };
  const KINDS = { user: [load, build], topic: [loadTopic, buildTopic] };
  const open = (a, kind, key, delay) => {
    clearTimeout(closeTimer);
    if (anchor === a) return;
    clearTimeout(openTimer);
    const [fetchCard, makeCard] = KINDS[kind];
    fetchCard(key); // start early, while the pointer settles
    openTimer = setTimeout(async () => {
      const data = await fetchCard(key);
      if (!data || !a.isConnected) return;
      close();
      anchor = a;
      card = makeCard(data);
      // The gap between a link and its card can lie over another link (a topic's card covers its
      // byline): crossing it queues that link's card, and the pointer leaves it straight into
      // this one, so no mouseout cancels it. Reaching the card cancels it instead.
      card.addEventListener("mouseenter", () => {
        clearTimeout(closeTimer);
        clearTimeout(openTimer);
      });
      card.addEventListener("mouseleave", closeSoon);
      card.addEventListener(
        "focusout",
        (e) => !card?.contains(e.relatedTarget) && e.relatedTarget !== anchor && closeSoon(),
      );
      document.body.append(card);
      place(card, a);
    }, delay);
  };
  document.addEventListener("mouseover", (e) => {
    if (!matchMedia("(hover: hover)").matches) return;
    const t = target(e);
    if (t) open(t[0], t[1], t[2], t[1] === "topic" ? 600 : 450);
  });
  document.addEventListener("mouseout", (e) => {
    if (target(e) && !card?.contains(e.relatedTarget)) closeSoon();
  });
  // Keyboard: the card follows focus on a profile or topic link, a little slower than the pointer.
  document.addEventListener("focusin", (e) => {
    if (card?.contains(e.target)) return;
    const t = target(e);
    if (t && e.target.matches(":focus-visible")) open(t[0], t[1], t[2], 800);
    else if (card) closeSoon();
  });
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && card) {
      const back = anchor;
      close();
      back?.focus();
    }
  });
  addEventListener("scroll", () => card && anchor && place(card, anchor), { passive: true });
}
