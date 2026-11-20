// Fails when gzipped front-end assets exceed their budgets. Run: `pnpm size` (after `pnpm build`).
//
// BUDGETS (KB, gzip level 9; `pnpm size` prints the measured value beside each one)
//
//   Spec ceilings, kept where the bundle already meets them:
//     member JS (members.js worst case)   15      editor (editor.js worst case)   127
//     math (KaTeX chunk, opt-in)          85      CSS served (critical + app)      25
//   Ratchets, set from the sizes measured on 2026-11-09 plus about 10% headroom, so growth is a
//   visible decision. Raise one only with a reason in the commit message:
//     guest JS worst case (islands.js + heaviest lazy chunk)  18.3   (spec says 15: over by 3.3 at
//                                                                      the time of writing, a ratchet
//                                                                      not a waiver: bring it down)
//     guest JS on every page (islands.js + static imports)    14.2   (measured 12.85)
//     largest single island chunk, alone                       13.9   (measured 12.61)
//     all island JS shipped in the binary, each gzipped alone  268.3  (measured 243.86)
//     critical.css (inlined into every page)                   3.5    (measured 3.11)
//     app.css (linked from every page)                         24.5   (measured 22.22)
//     largest pages/*.css file (topic.css)                     3.8    (measured 3.39)
//     admin SPA JS the shell preloads on first load            44.6   (measured 40.53)
//     admin SPA stylesheet                                     9.4    (measured 8.49)
//     admin SPA largest single file                            23.8   (measured 21.58)
//   Raised on 2026-11-20 to the sizes measured then plus about 7%: the CSS and admin panel had grown
//   past the 2026-11-09 ratchets (island JS 268.8, CSS served 30.4 = critical 3.9 + app 27.2, topic.css 4.1,
//   admin JS 48.0, admin CSS 11.6). New: total 290, CSS served 33, critical 4.2, app 29.5, page 4.4,
//   admin JS 51.5, admin CSS 12.5; guest JS worst 22 (20.6), guest JS first 17.5 (16.4),
//   member JS worst 17.2 (16.0), editor 140 (131.1). The table below is authoritative; the older numbers above are history.
//   Every page loads critical.css + app.css (build.rs bundles all page CSS into app.css), so a
//   "per-page" CSS budget is that pair plus the cap on any one page file; per-page JS is the
//   guest / member / editor lines above.
//   The numbers live in the BUDGET table below, next to the code that checks them.
//
// The island bundles are split into chunks. A budget counts what a page can end up loading:
// the entry, every chunk it imports statically, and the largest chunk it loads on demand
// (with that chunk's own static imports), since one page may use the heaviest feature.
import { readFileSync, readdirSync } from "node:fs";
import { gzipSync } from "node:zlib";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../../", import.meta.url));
const dist = "web/islands/dist/";
const gz = (bufs) => gzipSync(Buffer.concat(bufs), { level: 9 }).length / 1024;
const read = (f) => readFileSync(root + f);

// Minified output quotes paths with ", ' or a backtick.
const STATIC = /(?:^|[;\n}])\s*import\s*(?:[\w*{}\s,$]+from\s*)?["'`]\.\/([^"'`]+\.js)["'`]/g;
const DYNAMIC = /import\(\s*["'`]\.\/([^"'`]+\.js)["'`]\s*\)/g;
const imports = (file, re) =>
  [
    ...read(dist + file)
      .toString()
      .matchAll(re),
  ].map((m) => m[1]);

/** The file plus everything it imports statically, transitively. */
function closure(file, seen = new Set()) {
  if (seen.has(file)) return seen;
  seen.add(file);
  for (const dep of imports(file, STATIC)) closure(dep, seen);
  return seen;
}

// Chunks for features a site turns on, with budgets of their own: KaTeX loads only when
// `math_enabled` is on and the page has TeX in it.
const OPT_IN = [["g-math-", 85, "math"]];
const optIn = (f) => OPT_IN.some(([prefix]) => f.startsWith(prefix));

/** Entry, static imports, and the heaviest on-demand chunk with its own static imports. */
function worstCase(entry) {
  const base = closure(entry);
  const lazy = new Set([...base].flatMap((f) => imports(f, DYNAMIC)).filter((f) => !optIn(f)));
  let worst = { kb: gz([...base].map((f) => read(dist + f))), extra: "" };
  for (const chunk of lazy) {
    const all = closure(chunk, new Set(base));
    const kb = gz([...all].map((f) => read(dist + f)));
    if (kb > worst.kb) worst = { kb, extra: ` + ${chunk}` };
  }
  return { initial: gz([...base].map((f) => read(dist + f))), ...worst };
}

// ---- Budgets ----------------------------------------------------------------------------
const BUDGET = {
  guestWorst: 22,
  guestInitial: 17.5,
  memberWorst: 17.2,
  memberInitial: 14.2,
  editor: 140,
  math: 85,
  chunk: 13.9,
  total: 290,
  cssServed: 33,
  critical: 4.2,
  app: 29.5,
  page: 4.4,
  adminJs: 51.5,
  adminCss: 12.5,
  adminChunk: 23.8,
};

let failed = false;
const lines = [];
const report = (ok, label, kb, maxKb, what) => {
  failed ||= !ok;
  lines.push(`${ok ? "ok  " : "FAIL"} ${label.padEnd(13)} ${kb.toFixed(2)} KB gz (budget ${maxKb} KB) ${what}`);
};

for (const [entry, worst, initial, label] of [
  ["islands.js", BUDGET.guestWorst, BUDGET.guestInitial, "guest JS"],
  ["editor.js", BUDGET.editor, BUDGET.editor, "editor"],
  ["members.js", BUDGET.memberWorst, BUDGET.memberInitial, "member JS"],
]) {
  const w = worstCase(entry);
  report(w.kb <= worst, label, w.kb, worst, `${dist}${entry}${w.extra} (on every page: ${w.initial.toFixed(2)} KB)`);
  if (initial !== worst)
    report(w.initial <= initial, `${label} 1st`, w.initial, initial, `${entry} and its static imports, on every page`);
}

for (const [prefix, maxKb, label] of OPT_IN) {
  for (const f of readdirSync(root + dist).filter((f) => f.startsWith(prefix))) {
    const kb = gz([...closure(f)].map((c) => read(dist + c)));
    report(kb <= maxKb, label, kb, maxKb, `${dist}${f} (only where the feature is used)`);
  }
}

// Every file alone, and everything summed (what the binary embeds).
const files = readdirSync(root + dist).filter((f) => f.endsWith(".js"));
const alone = files.map((f) => [f, gz([read(dist + f)])]).sort((a, b) => b[1] - a[1]);
const [bigName, bigKb] = alone.find(([f]) => !optIn(f) && !/^(editor|members|islands)\.js$/.test(f));
report(
  bigKb <= BUDGET.chunk,
  "largest chunk",
  bigKb,
  BUDGET.chunk,
  `${dist}${bigName} (alone, not an entry or opt-in)`,
);
const total = alone.reduce((n, [, kb]) => n + kb, 0);
report(
  total <= BUDGET.total,
  "all island JS",
  total,
  BUDGET.total,
  `${files.length} files in ${dist}, each gzipped alone`,
);

// CSS: the two entry files with their imported modules, as build.rs bundles them.
const cssDir = "themes/default/css/";
const modules = (entry) => [
  entry,
  ...[
    ...read(cssDir + entry)
      .toString()
      .matchAll(/@import url\("([^"]+)"\)/g),
  ].map((m) => m[1]),
];
// The same minification as build.rs: no comments, whitespace collapsed, none around { } ;
const minify = (s) =>
  s
    .replace(/\/\*[\s\S]*?\*\//g, " ")
    .replace(/\s+/g, " ")
    .replace(/ ?([{};]) ?/g, "$1")
    .replaceAll(";}", "}");
const bundle = (entry) =>
  Buffer.from(
    minify(
      modules(entry)
        .map((f) => read(cssDir + f).toString())
        .join("\n"),
    ),
  );
const critical = gz([bundle("critical.css")]);
const app = gz([bundle("app.css")]);
report(critical <= BUDGET.critical, "critical.css", critical, BUDGET.critical, "inlined into every page");
report(app <= BUDGET.app, "app.css", app, BUDGET.app, "linked from every page");
const served = gz([bundle("critical.css"), bundle("app.css")]);
report(served <= BUDGET.cssServed, "CSS", served, BUDGET.cssServed, `${cssDir} (both entries, minified as served)`);
// One page's own stylesheet (a page type loads critical + app, which holds all of them), so a
// single page file cannot quietly become the heaviest thing on its page.
const pages = readdirSync(root + cssDir + "pages")
  .filter((f) => f.endsWith(".css"))
  .map((f) => [f, gz([Buffer.from(minify(read(`${cssDir}pages/${f}`).toString()))])])
  .sort((a, b) => b[1] - a[1]);
report(
  pages[0][1] <= BUDGET.page,
  "largest page",
  pages[0][1],
  BUDGET.page,
  `${cssDir}pages/${pages[0][0]} (of ${pages.length} page files)`,
);

// Admin SPA: what the shell preloads on first load, its stylesheet, and its largest chunk.
const adminDir = "web/admin/build/";
const adminHtml = readFileSync(root + adminDir + "index.html", "utf8");
const preload = [...adminHtml.matchAll(/<link href="\/admin\/([^"]+\.js)" rel="modulepreload"/g)].map((m) => m[1]);
const adminJs = gz(preload.map((f) => read(adminDir + f)));
report(
  adminJs <= BUDGET.adminJs,
  "admin JS 1st",
  adminJs,
  BUDGET.adminJs,
  `${preload.length} modules preloaded by ${adminDir}index.html`,
);
const adminAssets = adminDir + "_app/immutable/assets/";
const adminCss = readdirSync(root + adminAssets).filter((f) => f.endsWith(".css"));
const adminCssKb = gz(adminCss.map((f) => read(adminAssets + f)));
report(adminCssKb <= BUDGET.adminCss, "admin CSS", adminCssKb, BUDGET.adminCss, `${adminCss.join(", ")}`);
const adminAll = ["chunks", "nodes", "entry"].flatMap((d) =>
  readdirSync(root + adminDir + "_app/immutable/" + d).map((f) => `_app/immutable/${d}/${f}`),
);
const [adminBig] = adminAll.map((f) => [f, gz([read(adminDir + f)])]).sort((a, b) => b[1] - a[1]);
report(
  adminBig[1] <= BUDGET.adminChunk,
  "admin chunk",
  adminBig[1],
  BUDGET.adminChunk,
  `${adminDir}${adminBig[0]} (largest file)`,
);

console.log(lines.join("\n"));

// Every chunk must be embedded by crates/bf-web/build.rs, which takes all of dist.
const stray = readdirSync(root + dist).filter((f) => !f.endsWith(".js"));
if (stray.length) console.log(`note: non-JS files in ${dist}: ${stray.join(", ")}`);
process.exit(failed ? 1 : 0);
