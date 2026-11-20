import { defineConfig } from "vitest/config";

// Browser checks (axe accessibility, screenshot diffs): they need a Chromium, so they are not part of
// the Docker `scripts/test.sh` run. `npm run test:browser`, or the `browser` job in
// .github/workflows/web.yml. The same server-per-file harness as the HTTP specs.
export default defineConfig({
  test: {
    include: ["browser/**/*.test.ts"],
    globalSetup: ["./lib/global-setup.ts"],
    pool: "forks",
    testTimeout: 60_000,
    hookTimeout: 120_000,
    bail: 0,
    sequence: { concurrent: false, shuffle: false },
  },
});
