import { defineConfig } from "vitest/config";

// Each test file gets its own database and server, so files run in parallel.
// Checks inside a file build on each other and run in order.
export default defineConfig({
  test: {
    include: ["specs/**/*.test.ts"],
    globalSetup: ["./lib/global-setup.ts"],
    pool: "forks",
    testTimeout: 30_000,
    hookTimeout: 120_000,
    bail: 0,
    sequence: { concurrent: false, shuffle: false },
  },
});
