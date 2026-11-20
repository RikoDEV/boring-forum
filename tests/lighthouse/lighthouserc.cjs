// Lighthouse CI: the home page, a topic and a profile, on a phone-sized run (Lighthouse's default)
// against the real server. `npm run lhci` here, or the `browser` job in .github/workflows/web.yml.
//
// The floors sit a little under what the pages scored on 2026-11-09 (median of three runs, phone
// emulation, a debug build): performance 99-100, accessibility 100, best practices 96-100, with
// run-to-run noise of a few points in performance (an occasional layout shift on the topic page).
// Raise them when a page gets better.
//   Locally: CHROME_PATH=<a Chromium>, BF_BIN=<server binary>, BF_DATABASE_URL=<an empty database>
const port = process.env.LH_PORT ?? "8099";
const base = `http://127.0.0.1:${port}`;

module.exports = {
  ci: {
    collect: {
      startServerCommand: "node start.mjs",
      startServerReadyPattern: "READY",
      startServerReadyTimeout: 180000,
      url: [`${base}/`, `${base}/t/welcome-to-the-forum/1`, `${base}/u/alice`],
      numberOfRuns: 3,
      settings: { chromeFlags: "--no-sandbox --headless=new" },
    },
    assert: {
      assertions: {
        "categories:performance": ["error", { minScore: 0.9, aggregationMethod: "median-run" }],
        "categories:accessibility": ["error", { minScore: 0.97, aggregationMethod: "median-run" }],
        "categories:best-practices": ["error", { minScore: 0.9, aggregationMethod: "median-run" }],
      },
    },
    upload: { target: "filesystem", outputDir: ".lighthouseci" },
  },
};
