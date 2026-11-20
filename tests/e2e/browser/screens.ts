// The screens the browser specs visit, one list so the axe and screenshot specs agree.
import type { Page } from "playwright-core";
import type { Client, Forum } from "../lib/forum.ts";
import { seed, topic } from "../lib/seed.ts";

export type Screen = {
  name: string;
  path: string;
  /** Who looks at it. */
  as: "guest" | "admin";
  /** A selector that is there once the page has drawn (the admin panel is a single-page app). */
  ready?: string;
};

export type World = { alice: Client; screens: Screen[] };

/** Alice (admin), a forum with one topic, and the list of screens. */
export async function build(f: Forum): Promise<World> {
  const s = await seed(f);
  const t = await topic(s.alice, s.FORUM, "Welcome to the forum", "A first post, long enough to read as a real one.");
  await s.bob.form(`/t/${t.id}/reply`, { body: "A reply from bob, so the topic has two posts." });
  return {
    alice: s.alice,
    screens: [
      { name: "home", path: "/", as: "guest" },
      { name: "forum", path: `/c/introductions/${s.FORUM}`, as: "guest" },
      { name: "topic", path: t.url, as: "guest" },
      { name: "login", path: "/login", as: "guest" },
      { name: "profile", path: "/u/alice", as: "guest" },
      { name: "admin-overview", path: "/admin/", as: "admin", ready: ".acp-shell" },
      { name: "admin-text", path: "/admin/text", as: "admin", ready: ".acp-shell .acp-box" },
      { name: "admin-logs", path: "/admin/logs", as: "admin", ready: ".acp-shell table" },
      { name: "admin-audit", path: "/admin/audit", as: "admin", ready: ".acp-shell table" },
      { name: "settings", path: "/settings", as: "admin" },
    ],
  };
}

/** Loads a screen and waits until it has drawn and its scripts have run. */
export async function visit(page: Page, forum: Forum, screen: Screen) {
  const res = await page.goto(forum.base + screen.path, { waitUntil: "load" });
  if (!res || res.status() >= 400) throw new Error(`${screen.name}: ${screen.path} answered ${res?.status()}`);
  await page.waitForLoadState("networkidle").catch(() => {});
  if (screen.ready) {
    await page
      .waitForSelector(screen.ready, { timeout: 15_000 })
      .catch((e: Error) => Promise.reject(new Error(`${screen.name}: ${screen.ready} never appeared on ${screen.path} (${e.message.split("\n")[0]})`)));
  }
  await page.evaluate(() => document.fonts?.ready);
}
