import test from "node:test";
import assert from "node:assert/strict";
import { tokens } from "../src/lib/code.js";

const joined = (text, lang) =>
  tokens(text, lang)
    .map((t) => t[1])
    .join("");

test("robots.txt: directives are keywords, URLs strings, # starts a comment", () => {
  const text = "User-agent: *\nDisallow: /admin # private\n# a comment\nSitemap: https://f.test/sitemap.xml\n";
  assert.equal(joined(text, "robots"), text);
  const t = tokens(text, "robots");
  assert.deepEqual(t[0], ["k", "User-agent"]);
  assert.ok(t.some(([c, s]) => c === "c" && s.startsWith("# private")));
  assert.ok(t.some(([c, s]) => c === "s" && s === "https://f.test/sitemap.xml"));
});

test("host lists: each line is a string, comments stay comments", () => {
  const text = "https://a.example\n  https://b.example  \n# why\n\n";
  assert.equal(joined(text, "hosts"), text);
  const strings = tokens(text, "hosts").filter(([c]) => c === "s");
  assert.deepEqual(
    strings.map((t) => t[1]),
    ["https://a.example", "https://b.example"],
  );
});

test("an unknown language is left plain", () => {
  assert.deepEqual(tokens("a: b", "nope"), [["", "a: b"]]);
});
