#!/usr/bin/env node
// Screenshots of the running dev server, for checking UI work by eye (no dependencies: headless
// Chromium driven over the DevTools protocol with Node's built-in WebSocket).
//
//   node scripts/shot.mjs <url> <out.png> [options]
//     --cookie "a=1; b=2"   cookies to send (e.g. a throwaway admin's __Host-bf_session)
//     --click <selector>    click an element first; repeatable, runs in order
//     --hover <selector>    move the mouse over an element after the clicks (tooltips, :hover)
//     --wait <ms>           pause after load and after each click (default 600)
//     --width <px>          viewport width (default 1280; use 390 for phones)
//     --height <px>         viewport height (default 900)
//     --full                capture the whole page, not just the viewport
//     --light               light colour scheme (default dark)
//     --forced              Windows high-contrast style (forced-colors: active)
//     --chrome <path>       Chromium binary (default: newest Playwright Chromium, or $CHROME)
//     --eval <js>           evaluate an expression after the clicks and print its JSON result
//     --init <js>           run a script in every page before its own scripts (mock fetch, set storage)
//
// Prints the console errors and the layout warnings it finds: elements wider than the viewport
// and form controls whose boxes overlap.

import { spawn } from "node:child_process";
import { existsSync, readdirSync, writeFileSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const args = process.argv.slice(2);
const [url, out] = args;
if (!url || !out) {
  console.error("usage: node scripts/shot.mjs <url> <out.png> [--cookie ..] [--click sel].. [--hover sel] [--wait ms] [--width px] [--full] [--light]");
  process.exit(2);
}
const opt = (name, fallback) => {
  const i = args.indexOf(name);
  return i < 0 ? fallback : args[i + 1];
};
const clicks = args.flatMap((a, i) => (a === "--click" ? [args[i + 1]] : []));
const wait = Number(opt("--wait", 600));
const width = Number(opt("--width", 1280));
const height = Number(opt("--height", 900));

function findChrome() {
  if (opt("--chrome")) return opt("--chrome");
  if (process.env.CHROME) return process.env.CHROME;
  const root = join(process.env.LOCALAPPDATA ?? join(process.env.HOME ?? "", ".cache"), "ms-playwright");
  const dirs = existsSync(root) ? readdirSync(root).filter((d) => /^chromium-\d+$/.test(d)).sort((a, b) => +b.split("-")[1] - +a.split("-")[1]) : [];
  for (const d of dirs) {
    for (const exe of ["chrome-win64/chrome.exe", "chrome-win/chrome.exe", "chrome-linux64/chrome", "chrome-linux/chrome"]) {
      if (existsSync(join(root, d, exe))) return join(root, d, exe);
    }
  }
  throw new Error("No Chromium found: pass --chrome or set CHROME.");
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const profile = mkdtempSync(join(tmpdir(), "bf-shot-"));
const chrome = spawn(findChrome(), ["--headless=new", "--remote-debugging-port=0", `--user-data-dir=${profile}`, "--no-first-run", "--hide-scrollbars", `--window-size=${width},${height}`, "about:blank"], { stdio: ["ignore", "ignore", "pipe"] });
const wsUrl = await new Promise((resolve, reject) => {
  let buf = "";
  chrome.stderr.on("data", (d) => {
    buf += d;
    const m = buf.match(/DevTools listening on (ws:\/\/\S+)/);
    if (m) resolve(m[1]);
  });
  chrome.on("exit", () => reject(new Error(`Chromium exited: ${buf.slice(0, 500)}`)));
});

const browser = new WebSocket(wsUrl);
await new Promise((r) => browser.addEventListener("open", r, { once: true }));
let seq = 0;
const pending = new Map();
const listeners = [];
browser.addEventListener("message", (e) => {
  const m = JSON.parse(e.data);
  if (m.id && pending.has(m.id)) {
    const { resolve, reject } = pending.get(m.id);
    pending.delete(m.id);
    m.error ? reject(new Error(`${m.error.message}`)) : resolve(m.result);
  } else if (m.method) listeners.forEach((l) => l(m));
});
const send = (method, params = {}, sessionId) =>
  new Promise((resolve, reject) => {
    const id = ++seq;
    pending.set(id, { resolve, reject });
    browser.send(JSON.stringify({ id, method, params, sessionId }));
  });

const { targetId } = await send("Target.createTarget", { url: "about:blank" });
const { sessionId } = await send("Target.attachToTarget", { targetId, flatten: true });
const s = (method, params) => send(method, params, sessionId);
const errors = [];
listeners.push((m) => {
  if (m.sessionId !== sessionId) return;
  if (m.method === "Runtime.exceptionThrown") errors.push(m.params.exceptionDetails.exception?.description ?? m.params.exceptionDetails.text);
  if (m.method === "Runtime.consoleAPICalled" && m.params.type === "error") errors.push(m.params.args.map((a) => a.value ?? a.description).join(" "));
  if (m.method === "Log.entryAdded" && m.params.entry.level === "error") errors.push(`${m.params.entry.text} ${m.params.entry.url ?? ""}`);
});
await s("Page.enable");
await s("Runtime.enable");
await s("Log.enable");
await s("Network.enable");
await s("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 1, mobile: width < 600 });
await s("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: args.includes("--light") ? "light" : "dark" }, ...(args.includes("--forced") ? [{ name: "forced-colors", value: "active" }] : [])] });
const cookie = opt("--cookie");
if (cookie) {
  for (const pair of cookie.split(";").map((c) => c.trim()).filter(Boolean)) {
    const [name, ...rest] = pair.split("=");
    await s("Network.setCookie", { name, value: rest.join("="), url, secure: true, path: "/" });
  }
}

if (opt("--init")) await s("Page.addScriptToEvaluateOnNewDocument", { source: opt("--init") });
const loaded = new Promise((r) => listeners.push((m) => m.sessionId === sessionId && m.method === "Page.loadEventFired" && r()));
await s("Page.navigate", { url });
await loaded;
await sleep(wait);
for (const sel of clicks) {
  const r = await s("Runtime.evaluate", {
    expression: `(() => { const e = document.querySelector(${JSON.stringify(sel)}); if (!e) return false; e.click(); return true; })()`,
    returnByValue: true,
  });
  if (!r.result.value) errors.push(`--click: nothing matches ${sel}`);
  await sleep(wait);
}

if (opt("--hover")) {
  const sel = opt("--hover");
  const r = await s("Runtime.evaluate", {
    expression: `(() => { const e = document.querySelector(${JSON.stringify(sel)}); if (!e) return null; e.scrollIntoView({ block: "center" }); const b = e.getBoundingClientRect(); return [b.x + b.width / 2, b.y + b.height / 2]; })()`,
    returnByValue: true,
  });
  if (!r.result.value) errors.push(`--hover: nothing matches ${sel}`);
  else {
    const [x, y] = r.result.value;
    await s("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
    await sleep(wait);
  }
}

if (opt("--eval")) {
  const r = await s("Runtime.evaluate", { expression: opt("--eval"), returnByValue: true, awaitPromise: true });
  console.log(`eval: ${JSON.stringify(r.result.value ?? r.exceptionDetails?.exception?.description ?? r.result.description)}`);
  await sleep(wait); // let transitions it started (focus, scroll) finish
}

// Layout warnings: things wider than the viewport, and visible form controls that overlap.
const check = await s("Runtime.evaluate", {
  returnByValue: true,
  expression: `(() => {
    const out = [];
    const vw = document.documentElement.clientWidth;
    const name = (e) => e.tagName.toLowerCase() + (e.id ? "#" + e.id : "") + (e.className && typeof e.className === "string" ? "." + e.className.trim().split(/\\s+/).join(".") : "");
    if (document.documentElement.scrollWidth > vw + 1) {
      out.push("page scrolls sideways: " + document.documentElement.scrollWidth + "px > " + vw + "px");
      // The outermost elements that reach past the viewport: the ones to fix.
      const wide = [...document.body.querySelectorAll("*")].filter((e) => e.getBoundingClientRect().right > vw + 1 && !(e.parentElement && e.parentElement.getBoundingClientRect().right > vw + 1));
      for (const e of wide.slice(0, 5)) out.push("  too wide: " + name(e) + " right=" + Math.round(e.getBoundingClientRect().right) + "px");
    }
    const root = document.querySelector("dialog[open]") ?? document;
    const ctl = [...root.querySelectorAll("input:not([type=hidden]):not([type=radio]):not([type=checkbox]), select, textarea, button")]
      .filter((e) => e.offsetParent !== null)
      // Sticky headers and floating bars cover fields by design while the page scrolls.
      .filter((e) => { for (let a = e; a; a = a.parentElement) { const p = getComputedStyle(a).position; if (p === "fixed" || p === "sticky") return false; } return true; })
      .map((e) => [e, e.getBoundingClientRect()]).filter(([, r]) => r.width && r.height);
    for (let i = 0; i < ctl.length; i++) for (let j = i + 1; j < ctl.length; j++) {
      const [a, ra] = ctl[i], [b, rb] = ctl[j];
      if (a.contains(b) || b.contains(a)) continue;
      const x = Math.min(ra.right, rb.right) - Math.max(ra.left, rb.left), y = Math.min(ra.bottom, rb.bottom) - Math.max(ra.top, rb.top);
      if (x > 2 && y > 2) out.push("overlap: " + name(a) + " and " + name(b));
    }
    return out;
  })()`,
});

const shot = await s("Page.captureScreenshot", { format: "png", captureBeyondViewport: args.includes("--full") });
writeFileSync(out, Buffer.from(shot.data, "base64"));
console.log(`saved ${out}`);
for (const w of check.result.value ?? []) console.log(`layout: ${w}`);
for (const e of errors) console.log(`console: ${e}`);
browser.close();
chrome.kill();
