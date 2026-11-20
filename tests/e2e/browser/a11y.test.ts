// Accessibility with axe-core on the main screens: dark and light, desktop and phone width, in
// English, Polish and Arabic (right to left). A violation fails the test unless it is listed
// in KNOWN below with the reason, so the list is the backlog and a new one cannot slip in.
import AxeBuilder from "@axe-core/playwright";
import type { Browser } from "playwright-core";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { LANGS, launch, open, type Variant } from "../lib/browser.ts";
import { useForum } from "../lib/forum.ts";
import { build, visit, type World } from "./screens.ts";

/** `rule@screen` (or just `rule`) that are known and not yet fixed, with why. */
const KNOWN: Record<string, string> = {};

const forum = useForum();
let browser: Browser;
let world: World;

beforeAll(async () => {
  world = await build(forum);
  browser = await launch();
});
afterAll(async () => {
  await browser?.close();
});

const WIDTHS = [1280, 390];
const SCHEMES = ["dark", "light"] as const;

for (const lang of LANGS) {
  describe(`axe (${lang})`, () => {
    for (const scheme of SCHEMES) {
      for (const width of WIDTHS) {
        const v: Variant = { lang, scheme, width };
        it(`${scheme}, ${width}px`, async () => {
          const found: string[] = [];
          for (const screen of world.screens) {
            const { ctx, page } = await open(browser, forum, screen.as === "admin" ? world.alice : null, v);
            try {
              await visit(page, forum, screen);
              const dir = await page.evaluate(() => document.documentElement.dir);
              expect(dir, `${screen.name} direction`).toBe(lang === "ar" ? "rtl" : "ltr");
              const result = await new AxeBuilder({ page })
                .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa", "best-practice"])
                .analyze();
              for (const viol of result.violations) {
                if (KNOWN[`${viol.id}@${screen.name}`] || KNOWN[viol.id]) continue;
                const where = viol.nodes
                  .slice(0, 3)
                  .map((n) => n.target.join(" "))
                  .join(" | ");
                found.push(`${screen.name}: ${viol.id} (${viol.impact}) x${viol.nodes.length}: ${where}`);
              }
            } finally {
              await ctx.close();
            }
          }
          expect(found, found.join("\n")).toEqual([]);
        });
      }
    }
  });
}
