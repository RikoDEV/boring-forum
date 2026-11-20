// Screenshot comparison against baselines kept in the repo.
//
// Baselines live in browser/__screenshots__/<platform>/<name>.png. Fonts and text rendering
// differ between operating systems, so each platform has its own set; a platform with no
// baseline yet records one with `UPDATE_SNAPSHOTS=1` (npm run test:browser:update) and commits it.
// In CI a missing baseline is skipped, not failed, and the actual image is left in
// test-results/ for the workflow to upload, so the first run on a platform produces them.
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { PNG } from "pngjs";
import pixelmatch from "pixelmatch";
import { REPO } from "./env.ts";
import { saveOutput } from "./browser.ts";

const BASE = join(REPO, "tests/e2e/browser/__screenshots__", process.platform);
/** The share of pixels allowed to differ (anti-aliasing noise), and pixelmatch's colour tolerance. */
const MAX_RATIO = 0.002;
const THRESHOLD = 0.1;

export type Outcome = { status: "match" | "created" | "missing" | "mismatch"; detail: string };

export function compare(name: string, actual: Buffer): Outcome {
  const file = join(BASE, `${name}.png`);
  if (process.env.UPDATE_SNAPSHOTS) {
    mkdirSync(dirname(file), { recursive: true });
    writeFileSync(file, actual);
    return { status: "created", detail: file };
  }
  if (!existsSync(file)) {
    saveOutput(`${name}.actual.png`, actual);
    return { status: "missing", detail: `no baseline for ${process.platform}: ${file}` };
  }
  const want = PNG.sync.read(readFileSync(file));
  const got = PNG.sync.read(actual);
  if (want.width !== got.width || want.height !== got.height) {
    saveOutput(`${name}.actual.png`, actual);
    return { status: "mismatch", detail: `size ${got.width}x${got.height}, baseline ${want.width}x${want.height}` };
  }
  const diff = new PNG({ width: want.width, height: want.height });
  const n = pixelmatch(want.data, got.data, diff.data, want.width, want.height, { threshold: THRESHOLD });
  const ratio = n / (want.width * want.height);
  if (ratio <= MAX_RATIO) return { status: "match", detail: `${n} px differ` };
  saveOutput(`${name}.actual.png`, actual);
  saveOutput(`${name}.diff.png`, PNG.sync.write(diff));
  return { status: "mismatch", detail: `${n} px differ (${(ratio * 100).toFixed(2)}%, limit ${MAX_RATIO * 100}%)` };
}
