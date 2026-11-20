// The GIF picker: a dialog that searches a GIF provider and hands back the picture to insert.
//
// The providers (KLIPY, GIPHY) require that this browser asks their API and loads their media
// directly, so nothing here goes through the forum's server and the picture inserted is the
// provider's own address, as returned. The page's Content-Security-Policy opens exactly the
// chosen provider's hosts (crates/bf-web/src/security.rs).
import { tr } from "../dom.js";

const PER_PAGE = 24;
const BIG = 3_000_000;

/** @typedef {{ id: string, slug: string, title: string, thumb: { url: string, width: number, height: number }, url: string }} Gif */
/** @typedef {{ key: string, provider: string, format: string, filter: string, country: string, limit: number }} Config */

const num = (v) => Number(v) || 0;

/** A stable, anonymous id of this browser: the providers want one per user for their ranking. */
const customerId = () => {
  try {
    let id = localStorage.getItem("bf-gif-id");
    if (!id) {
      id = crypto.randomUUID();
      localStorage.setItem("bf-gif-id", id);
    }
    return id;
  } catch {
    return "anonymous";
  }
};

const RATING = { off: "r", low: "pg-13", medium: "pg", high: "g" };

/** Each provider: its name, and how to ask it for a page of results and read the answer. */
const PROVIDERS = {
  klipy: {
    name: "KLIPY",
    /** @param {Config} c @param {{ q: string, page: number, signal: AbortSignal }} a */
    async page(c, { q, page, signal }) {
      const url = new URL(`https://api.klipy.com/api/v1/${c.key}/gifs/${q ? "search" : "trending"}`);
      url.searchParams.set("page", String(page));
      url.searchParams.set("per_page", String(PER_PAGE));
      url.searchParams.set("customer_id", customerId());
      url.searchParams.set("locale", c.country);
      url.searchParams.set("content_filter", c.filter);
      url.searchParams.set("format_filter", "webp,gif");
      if (q) url.searchParams.set("q", q);
      const res = await fetch(url, { signal });
      if (!res.ok) throw new Error(String(res.status));
      const body = await res.json();
      if (body.result === false) throw new Error("refused");
      const data = body.data ?? {};
      const rows = Array.isArray(data) ? data : (data.data ?? []);
      /** @type {Gif[]} */
      const items = [];
      for (const r of rows) {
        const small = r.file?.sm?.webp ?? r.file?.sm?.gif ?? r.file?.xs?.webp ?? r.file?.xs?.gif;
        const full = r.file?.md?.[c.format] ?? r.file?.md?.gif ?? r.file?.md?.webp ?? r.file?.hd?.[c.format];
        if (!small?.url || !full?.url) continue;
        items.push({
          id: String(r.id ?? r.slug),
          slug: String(r.slug ?? ""),
          title: String(r.title ?? ""),
          thumb: { url: small.url, width: num(small.width), height: num(small.height) },
          url: full.url,
        });
      }
      return { items, more: data.has_next === true };
    },
    /** Tells the provider which GIF was chosen for which search: it ranks results by it. */
    share(c, gif, q) {
      if (!gif.slug) return;
      fetch(`https://api.klipy.com/api/v1/${c.key}/gifs/share/${encodeURIComponent(gif.slug)}`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ customer_id: customerId(), q }),
        keepalive: true,
      }).catch(() => {});
    },
  },
  giphy: {
    name: "GIPHY",
    async page(c, { q, page, signal }) {
      const url = new URL(`https://api.giphy.com/v1/gifs/${q ? "search" : "trending"}`);
      url.searchParams.set("api_key", c.key);
      url.searchParams.set("limit", String(PER_PAGE));
      url.searchParams.set("offset", String((page - 1) * PER_PAGE));
      url.searchParams.set("rating", RATING[c.filter] ?? "g");
      url.searchParams.set("lang", document.documentElement.lang?.slice(0, 2) || "en");
      url.searchParams.set("random_id", customerId());
      if (q) url.searchParams.set("q", q);
      const res = await fetch(url, { signal });
      if (!res.ok) throw new Error(String(res.status));
      const body = await res.json();
      /** @type {Gif[]} */
      const items = [];
      for (const r of body.data ?? []) {
        const im = r.images ?? {};
        const small = im.fixed_width ?? im.fixed_width_small;
        const thumb = small?.webp ?? small?.url;
        // The original is the sharpest, but can be huge: a big one gives way to the downsized copy.
        const o = im.original ?? {};
        const asWebp = c.format === "webp" && o.webp;
        const size = num(asWebp ? o.webp_size : o.size);
        const full = !o.url || size > BIG ? (im.downsized?.url ?? im.fixed_width?.url) : asWebp ? o.webp : o.url;
        if (!thumb || !full) continue;
        items.push({
          id: String(r.id),
          slug: "",
          title: String(r.title ?? ""),
          thumb: { url: thumb, width: num(small.width), height: num(small.height) },
          url: full,
        });
      }
      const p = body.pagination ?? {};
      return { items, more: num(p.offset) + num(p.count) < num(p.total_count) };
    },
    share() {},
  },
};

/** Alt text for the picture: the title, with nothing that could end the markdown early. */
const altText = (title) => title.replace(/[[\]()\n\r]+/g, " ").replace(/\s+/g, " ").trim().slice(0, 80) || "GIF";

/**
 * Opens the picker. `onPick` gets the markdown for the chosen GIF.
 * @param {Config} cfg
 * @param {(markdown: string) => void} onPick
 */
export function openGifPicker(cfg, onPick) {
  const provider = PROVIDERS[cfg.provider] ?? PROVIDERS.klipy;
  if (document.querySelector(".composer-gif")) return;

  const box = document.createElement("dialog");
  box.className = "composer-gif";
  box.setAttribute("aria-label", tr("Insert a GIF"));

  const head = document.createElement("div");
  head.className = "composer-gif-head";
  const search = document.createElement("input");
  search.type = "search";
  search.autocomplete = "off";
  search.spellcheck = false;
  // The provider's terms ask for this text, naming it.
  search.placeholder = tr("Search {provider}", { provider: provider.name });
  search.setAttribute("aria-label", search.placeholder);
  const close = document.createElement("button");
  close.type = "button";
  close.className = "secondary composer-gif-close";
  close.textContent = tr("Close");
  head.append(search, close);

  const scroller = document.createElement("div");
  scroller.className = "composer-gif-scroll";
  const grid = document.createElement("div");
  grid.className = "composer-gif-grid";
  const note = document.createElement("p");
  note.className = "composer-gif-note muted";
  note.setAttribute("role", "status");
  const retry = document.createElement("button");
  retry.type = "button";
  retry.className = "secondary";
  retry.textContent = tr("Try again");
  retry.hidden = true;
  const sentinel = document.createElement("div");
  scroller.append(grid, note, retry, sentinel);

  const foot = document.createElement("p");
  foot.className = "composer-gif-foot muted";
  foot.textContent = tr("Powered by {provider}", { provider: provider.name });
  box.append(head, scroller, foot);
  document.body.append(box);

  let q = "";
  let page = 0;
  let loaded = 0;
  let more = true;
  let busy = false;
  let abort = new AbortController();
  let debounce = 0;

  const say = (text, canRetry = false) => {
    note.textContent = text;
    retry.hidden = !canRetry;
  };

  async function load() {
    if (busy || !more) return;
    busy = true;
    const mine = abort;
    say(tr("Loading…"));
    try {
      const r = await provider.page(cfg, { q, page: page + 1, signal: mine.signal });
      if (mine.signal.aborted) return;
      page += 1;
      for (const g of r.items) {
        const b = document.createElement("button");
        b.type = "button";
        b.className = "composer-gif-item";
        const img = document.createElement("img");
        img.src = g.thumb.url;
        img.alt = g.title || "GIF";
        img.loading = "lazy";
        img.decoding = "async";
        if (g.thumb.width && g.thumb.height) {
          img.width = g.thumb.width;
          img.height = g.thumb.height;
        }
        b.append(img);
        b.addEventListener("click", () => {
          provider.share(cfg, g, q);
          box.close();
          onPick(`![${altText(g.title)}](${g.url})`);
        });
        grid.append(b);
      }
      loaded += r.items.length;
      more = r.more && r.items.length > 0 && !(cfg.limit && loaded >= cfg.limit);
      if (!loaded) say(tr("No GIFs found."));
      else if (!more && cfg.limit && loaded >= cfg.limit) say(tr("That is all for now: try a more specific search."));
      else say("");
    } catch (e) {
      if (!mine.signal.aborted) say(tr("GIFs could not be loaded."), true);
    } finally {
      busy = false;
    }
    // A short first page may not fill the dialog: keep going while the end is in view.
    if (more && !mine.signal.aborted && sentinel.getBoundingClientRect().top < scroller.getBoundingClientRect().bottom + 200) load();
  }

  function restart() {
    abort.abort();
    abort = new AbortController();
    grid.replaceChildren();
    page = 0;
    loaded = 0;
    more = true;
    busy = false;
    scroller.scrollTop = 0;
    load();
  }

  search.addEventListener("input", () => {
    clearTimeout(debounce);
    debounce = window.setTimeout(() => {
      const next = search.value.trim();
      if (next !== q) {
        q = next;
        restart();
      }
    }, 300);
  });
  retry.addEventListener("click", () => {
    busy = false;
    load();
  });
  new IntersectionObserver((entries) => entries.some((e) => e.isIntersecting) && load(), { root: scroller, rootMargin: "300px" }).observe(sentinel);
  close.addEventListener("click", () => box.close());
  box.addEventListener("click", (e) => {
    if (e.target === box) box.close();
  });
  box.addEventListener("close", () => {
    abort.abort();
    clearTimeout(debounce);
    box.remove();
  });
  box.showModal();
  search.focus();
  load();
}
