// Offline reading. Everything saved here is the guest's copy of a page, fetched without cookies:
// a member's own pages can name them, carry tokens or show private forums, and this device may be
// shared. So a saved page is one anyone could read, and signing out has nothing to clear.
const STRINGS = __BF_STRINGS__;
const VERSION = "v2";
const PAGES = `bf-pages-${VERSION}`;
const FILES = `bf-files-${VERSION}`;
const MAX_PAGES = 150;
const MAX_FILES = 800;
const MAX_FILES_PER_PAGE = 150;
const MAX_AGE = 14 * 86400 * 1000;
const FRESH = 10 * 60 * 1000; // a page saved this recently is not fetched again
const REFILL = 6 * 3600 * 1000;
const WAIT = 5000; // a saved page is better than waiting longer on a bad connection
const REFILL_KEY = "/__bf-refilled";

const readable = (url) => {
  if (url.origin !== location.origin) return false;
  const q = [...url.searchParams.keys()];
  if (q.some((k) => k !== "page")) return false;
  return /^\/(latest|tags)?$|^\/c\/[^/]+(\/\d+)?$|^\/t\/[^/]+\/\d+$|^\/tag\/[^/]+$/.test(url.pathname);
};

const esc = (s) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);

async function trim(name, max) {
  const cache = await caches.open(name);
  const keys = await cache.keys();
  for (const k of keys.slice(0, Math.max(0, keys.length - max))) await cache.delete(k);
}

// Saves the guest's copy of a page and the files it needs. Returns its HTML, or null if there is
// none for a guest, in which case any older copy goes too: the page is gone or no longer public.
async function save(href) {
  const url = new URL(href);
  const pages = await caches.open(PAGES);
  const res = await fetch(url, { credentials: "omit", redirect: "follow" }).catch(() => null);
  if (!res) return null; // no network says nothing about the page
  const final = new URL(res.url);
  const html = (res.headers.get("content-type") || "").includes("text/html");
  if (!res.ok || !html || !readable(final)) {
    await pages.delete(url.href);
    return null;
  }
  const text = await res.text();
  const headers = new Headers(res.headers);
  headers.delete("set-cookie");
  headers.set("x-bf-saved", String(Date.now()));
  const title = /<title>([^<]*)<\/title>/.exec(text);
  headers.set("x-bf-title", encodeURIComponent(title ? title[1] : final.pathname));
  // Rebuilt, because a browser refuses to show a stored response that was redirected.
  // Deleted first, so the oldest entries stay first in the cache and are the ones trimmed.
  await pages.delete(url.href);
  await pages.put(url.href, new Response(text, { status: 200, headers }));
  const files = await caches.open(FILES);
  // Files the page shows: its styles and icons, uploaded pictures and the members' avatars.
  const wanted = [
    ...new Set(text.match(/\/(?:assets|theme)\/[\w.\-/]+|\/uploads\/[\w.\-/]+|\/avatar\/[^"'\s?#<>)]+/g) || []),
  ].slice(0, MAX_FILES_PER_PAGE);
  // A few at a time, so a long topic does not open a hundred connections at once.
  for (let i = 0; i < wanted.length; i += 10) {
    await Promise.all(
      wanted.slice(i, i + 10).map(async (f) => {
        if (await files.match(f)) return;
        const got = await fetch(f, { credentials: "omit" }).catch(() => null);
        if (got?.ok) await files.put(f, got);
      }),
    );
  }
  await trim(PAGES, MAX_PAGES);
  await trim(FILES, MAX_FILES);
  return text;
}

// The front page and what it links to, so there is something to read before anything was visited.
async function refill() {
  const meta = await caches.open(PAGES);
  const stamp = await meta.match(REFILL_KEY);
  if (stamp && Date.now() - Number(await stamp.text()) < REFILL) return;
  await meta.put(REFILL_KEY, new Response(String(Date.now())));
  const home = await save(new URL("/", location.origin).href);
  if (!home) return;
  const links = new Set();
  for (const m of home.matchAll(/href="(\/[^"#]*)"/g)) {
    const u = new URL(m[1].replace(/&amp;/g, "&"), location.origin);
    if (readable(u)) links.add(u.href);
  }
  for (const href of [...links].slice(0, 25)) await save(href);
}

async function expire() {
  const pages = await caches.open(PAGES);
  for (const k of await pages.keys()) {
    const r = await pages.match(k);
    const at = Number(r.headers.get("x-bf-saved"));
    if (at && Date.now() - at > MAX_AGE) await pages.delete(k);
  }
}

async function savedPage(href) {
  const pages = await caches.open(PAGES);
  const r = await pages.match(href);
  if (!r) return null;
  const text = await r.text();
  const notice = `<div class="bf-offline-notice" role="status">${esc(STRINGS.notice)}</div>`;
  const shown = text.replace(/<body[^>]*>/, (b) => b + notice);
  return new Response(shown, { status: 200, headers: r.headers });
}

async function listPage(heading, text) {
  const pages = await caches.open(PAGES);
  const items = [];
  for (const k of await pages.keys()) {
    const u = new URL(k.url);
    if (u.pathname === REFILL_KEY) continue;
    const r = await pages.match(k);
    const title = decodeURIComponent(r.headers.get("x-bf-title") || u.pathname);
    items.push(`<li><a href="${esc(u.pathname + u.search)}">${esc(title)}</a></li>`);
  }
  const list = items.length ? `<h2>${esc(STRINGS.saved)}</h2><ul>${items.reverse().join("")}</ul>` : `<p>${esc(STRINGS.none)}</p>`;
  const body = `<!doctype html><html lang="${esc(STRINGS.lang)}" dir="${esc(STRINGS.dir)}"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="color-scheme" content="light dark"><title>${esc(heading)}</title><style>:root{color-scheme:light dark}body{font:16px/1.5 system-ui,sans-serif;max-width:40rem;margin:2rem auto;padding:0 1rem;background:light-dark(#f5f6f8,#111419);color:light-dark(#1a1f27,#e6e9ee)}a{color:light-dark(#0b64c0,#6fb1f5)}li{margin:.4rem 0}</style></head><body><h1>${esc(heading)}</h1><p>${esc(text)}</p>${list}</body></html>`;
  return new Response(body, { headers: { "Content-Type": "text/html; charset=utf-8", "Cache-Control": "no-store" } });
}

async function navigate(e) {
  const req = e.request;
  const url = new URL(req.url);
  if (req.method !== "GET") {
    return fetch(req).catch(() => listPage(STRINGS.heading, STRINGS.not_sent));
  }
  const isReadable = readable(url);
  const network = fetch(req);
  if (isReadable) {
    // Kept up to date in the background, only when the visit itself reaches the server.
    e.waitUntil(
      network
        .then(async (res) => {
          if (res.status >= 500) return; // says nothing about the page
          const saved = await (await caches.open(PAGES)).match(url.href);
          const age = saved ? Date.now() - Number(saved.headers.get("x-bf-saved")) : Infinity;
          if (age > FRESH) await save(url.href);
          await refill();
          await expire();
        })
        .catch(() => {}),
    );
  }
  const offline = async () => (isReadable && (await savedPage(url.href))) || listPage(STRINGS.heading, STRINGS.not_saved);
  const hasCopy = isReadable && (await (await caches.open(PAGES)).match(url.href));
  if (!hasCopy) return network.catch(offline);
  const timer = new Promise((resolve) => setTimeout(() => resolve(null), WAIT));
  const first = await Promise.race([network.catch(() => null), timer]);
  return first || offline();
}

self.addEventListener("install", () => self.skipWaiting());

self.addEventListener("activate", (e) => {
  e.waitUntil(
    (async () => {
      for (const k of await caches.keys()) {
        if (k.startsWith("bf-") && k !== PAGES && k !== FILES) await caches.delete(k);
      }
      await self.clients.claim();
      await refill().catch(() => {});
    })(),
  );
});

self.addEventListener("fetch", (e) => {
  const url = new URL(e.request.url);
  if (e.request.mode === "navigate") {
    e.respondWith(navigate(e));
    return;
  }
  // Files named by their content never change; the others are only used once the network fails.
  if (e.request.method !== "GET" || url.origin !== location.origin) return;
  if (url.pathname.startsWith("/assets/") || url.pathname.startsWith("/theme/")) {
    e.respondWith(
      caches.open(FILES).then(async (files) => {
        const hit = await files.match(url.pathname);
        if (hit) return hit;
        const res = await fetch(e.request);
        if (res.ok) await files.put(url.pathname, res.clone());
        return res;
      }),
    );
  } else if (url.pathname.startsWith("/uploads/") || url.pathname.startsWith("/avatar/")) {
    // Only ever read here: a member's private attachment must not be saved by viewing it, and
    // an avatar changes, so the network wins when there is one.
    e.respondWith(
      fetch(e.request).catch(async () => (await (await caches.open(FILES)).match(url.pathname)) || Response.error()),
    );
  }
});

self.addEventListener("push", (e) => {
  const d = e.data ? e.data.json() : {};
  e.waitUntil(self.registration.showNotification(d.title || "", { body: d.body || "", data: { url: d.url || "/" } }));
});

self.addEventListener("notificationclick", (e) => {
  e.notification.close();
  e.waitUntil(self.clients.openWindow(e.notification.data.url));
});
