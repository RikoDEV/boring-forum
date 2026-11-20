// Screenshot diffs of a few key screens in English, Polish and Arabic (right to left), against
// baselines in browser/__screenshots__/<platform>/ (see lib/snapshot.ts). A visual change that
// is meant: `npm run test:browser:update`, look at the images, commit them.
import type { Browser } from "playwright-core";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { LANGS, launch, open, type Variant } from "../lib/browser.ts";
import { useForum } from "../lib/forum.ts";
import { compare } from "../lib/snapshot.ts";
import { build, visit, type World } from "./screens.ts";

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

/** Screen name and the widths it is checked at, in dark mode. */
const SHOTS: [string, number[]][] = [
  ["home", [1280, 390]],
  ["forum", [1280, 390]],
  ["topic", [1280, 390]],
  ["login", [1280]],
  ["admin-overview", [1280]],
  ["admin-logs", [1280]],
  ["settings", [1280, 390]],
];

for (const lang of LANGS) {
  describe(`screenshots (${lang})`, () => {
    for (const [name, widths] of SHOTS) {
      for (const width of widths) {
        it(`${name} at ${width}px`, async (ctx) => {
          const screen = world.screens.find((s) => s.name === name)!;
          const v: Variant = { lang, scheme: "dark", width };
          // bypassCSP: the page gets a style that stills motion and hides times, which change
          // with every run. Nothing else about the page differs.
          const { ctx: bc, page } = await open(browser, forum, screen.as === "admin" ? world.alice : null, v, { bypassCSP: true });
          try {
            await visit(page, forum, screen);
            await page.addStyleTag({
              content: "*,*::before,*::after{animation:none!important;transition:none!important;caret-color:transparent!important}time,.bf-online,.badge{visibility:hidden!important}",
            });
            const png = await page.screenshot({ animations: "disabled", caret: "hide" });
            const out = compare(`${lang}-${name}-${width}`, png);
            if (out.status === "missing" && process.env.CI) return ctx.skip(out.detail);
            expect(out.status === "match" || out.status === "created", out.detail).toBe(true);
          } finally {
            await bc.close();
          }
        });
      }
    }
  });
}
