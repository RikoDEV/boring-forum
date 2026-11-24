import test from "node:test";
import assert from "node:assert/strict";

globalThis.document = { documentElement: { lang: "en" }, getElementById: () => null };
const { clickKey, compact, countable, postOf, written } = await import("../src/link-counts.js");

/** A link with just what the helpers read. */
const link = (href, { matches = [], inside = [], img = false, data = {} } = {}) => ({
  getAttribute: (n) => (n === "href" ? href : null),
  matches: (sel) =>
    matches.some((m) =>
      sel
        .split(",")
        .map((s) => s.trim())
        .includes(m),
    ),
  querySelector: () => (img ? {} : null),
  closest: (sel) => (inside.includes(sel) ? {} : null),
  dataset: data,
});

test("a post's link has its own key", () => {
  assert.notEqual(clickKey(1, "https://a.io"), clickKey(2, "https://a.io"));
  assert.notEqual(clickKey(1, "https://a.io"), clickKey(1, "https://b.io"));
});

test("big counts are short", () => {
  assert.equal(compact(7, "en"), "7");
  assert.equal(compact(161, "en"), "161");
  assert.equal(compact(1234, "en"), "1.2K");
});

test("web links are counted, pictures and mentions are not", () => {
  assert.equal(countable(link("https://example.com/a")), true);
  assert.equal(countable(link("http://example.com/a")), true);
  assert.equal(countable(link("//example.com/a")), true);
  assert.equal(countable(link("/t/some-topic/3")), false, "a path is not a web address");
  assert.equal(countable(link("mailto:a@b.io")), false);
  assert.equal(
    countable(link("https://example.com/a.png", { img: true })),
    false,
    "a picture opens, it is not followed",
  );
  assert.equal(countable(link("https://example.com/u", { matches: [".mention"] })), false);
  assert.equal(countable(link("https://example.com/f", { matches: ["[download]"] })), false);
  assert.equal(countable(link("https://example.com/a", { inside: [".composer-preview"] })), false);
});

test("a link keeps the address it was written with", () => {
  assert.equal(written(link("https://a.io/")), "https://a.io/");
  assert.equal(written(link("https://a.io/", { data: { url: "https://a.io" } })), "https://a.io");
});

test("the post of a link comes from its post", () => {
  const post = { dataset: { post: "9" }, querySelector: () => ({ dataset: { quote: "7" } }) };
  assert.equal(postOf({ closest: () => post }), "7");
  assert.equal(postOf({ closest: () => ({ dataset: { post: "9" }, querySelector: () => null }) }), "9");
  assert.equal(postOf({ closest: () => null }), undefined);
});
