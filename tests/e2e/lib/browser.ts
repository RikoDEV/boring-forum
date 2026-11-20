// A Chromium for the browser specs, and signed-in pages for a Forum from lib/forum.ts.
import { existsSync, mkdirSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { chromium, type Browser, type BrowserContext, type Page } from "playwright-core";
import { REPO } from "./env.ts";
import type { Client, Forum } from "./forum.ts";

export const LANGS = ["en", "pl", "ar"] as const;
export type Lang = (typeof LANGS)[number];
export type Scheme = "dark" | "light";
export type Variant = { lang: Lang; scheme: Scheme; width: number };

/** Chromium: $CHROME, else the newest Playwright download in ms-playwright (any revision, so a
 *  machine that has one needs no second download), else the one playwright-core installs. */
function chromePath(): string | undefined {
  if (process.env.CHROME) return process.env.CHROME;
  const root =
    process.env.PLAYWRIGHT_BROWSERS_PATH ??
    (process.platform === "win32"
      ? join(process.env.LOCALAPPDATA ?? "", "ms-playwright")
      : join(process.env.HOME ?? "", process.platform === "darwin" ? "Library/Caches/ms-playwright" : ".cache/ms-playwright"));
  if (!existsSync(root)) return undefined;
  const dirs = readdirSync(root)
    .filter((d) => /^chromium-\d+$/.test(d))
    .sort((a, b) => Number(b.split("-")[1]) - Number(a.split("-")[1]));
  // The revision playwright-core wants comes first, when present.
  for (const d of dirs) {
    for (const exe of ["chrome-win64/chrome.exe", "chrome-win/chrome.exe", "chrome-linux64/chrome", "chrome-linux/chrome", "chrome-mac/Chromium.app/Contents/MacOS/Chromium"]) {
      if (existsSync(join(root, d, exe))) return join(root, d, exe);
    }
  }
  return undefined;
}

export const launch = (): Promise<Browser> =>
  chromium.launch({ executablePath: chromePath(), args: ["--disable-dev-shm-usage"] });

/** A page for a variant (language, colour scheme, width), signed in as `who` or a guest. */
export async function open(browser: Browser, forum: Forum, who: Client | null, v: Variant, extra: { bypassCSP?: boolean } = {}) {
  const ctx: BrowserContext = await browser.newContext({
    viewport: { width: v.width, height: v.width < 600 ? 844 : 800 },
    colorScheme: v.scheme,
    locale: v.lang,
    reducedMotion: "reduce",
    bypassCSP: extra.bypassCSP ?? false,
  });
  // `bf_lang` is the language cookie the server reads first (viewer.rs); the rest of the jar
  // is the member's session. `__Host-` cookies are Secure, which Chromium allows on 127.0.0.1.
  const host = new URL(forum.base).hostname;
  const cookies = [{ name: "bf_lang", value: v.lang, domain: host, path: "/" }];
  for (const [name, value] of who?.jar ?? []) {
    // Playwright derives Secure from the url; a __Host- cookie must carry it and no Domain attribute.
    cookies.push({ name, value, domain: host, path: "/", ...(name.startsWith("__Host-") ? { secure: true } : {}) } as (typeof cookies)[number]);
  }
  await ctx.addCookies(cookies);
  const page: Page = await ctx.newPage();
  return { ctx, page };
}

const OUT = join(REPO, "tests/e2e/test-results");
export const saveOutput = (name: string, data: Buffer) => {
  mkdirSync(OUT, { recursive: true });
  const file = join(OUT, name);
  writeFileSync(file, data);
  return file;
};
