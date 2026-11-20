import test from "node:test";
import assert from "node:assert/strict";

globalThis.document = { documentElement: { lang: "en" }, getElementById: () => null };
const { fmt } = await import("../src/dom.js");

const topics = (one, few, other) => [{ sel: "n", cases: { one: [one], few: [few], other: [other] }, def: "other" }];
const plural = [{ sel: "n", cases: { one: ["1 topic"], other: [{ v: "n", num: true }, " topics"] }, def: "other" }];

test("plain string and variables", () => {
  assert.equal(fmt("Hello", {}, "en"), "Hello");
  assert.equal(fmt(["Hi ", { v: "name" }, "!"], { name: "Ann" }, "en"), "Hi Ann!");
});
test("missing variable shows its name", () => {
  assert.equal(fmt(["Hi ", { v: "name" }], {}, "en"), "Hi {name}");
});
test("plural en", () => {
  assert.equal(fmt(plural, { n: 1 }, "en"), "1 topic");
  assert.equal(fmt(plural, { n: 5 }, "en"), "5 topics");
});
test("plural pl", () => {
  const m = topics("one", "few", "other");
  const got = [1, 3, 5, 12, 22].map((n) => fmt(m, { n }, "pl"));
  assert.deepEqual(got, ["one", "few", "other", "other", "few"]);
});
test("plural ar", () => {
  const m = [
    {
      sel: "n",
      cases: { zero: ["zero"], one: ["one"], two: ["two"], few: ["few"], many: ["many"], other: ["other"] },
      def: "other",
    },
  ];
  const got = [0, 1, 2, 5, 11, 100].map((n) => fmt(m, { n }, "ar"));
  assert.deepEqual(got, ["zero", "one", "two", "few", "many", "other"]);
});
test("exact number case wins over category", () => {
  const m = [{ sel: "n", cases: { 0: ["none"], one: ["one"], other: ["many"] }, def: "other" }];
  assert.equal(fmt(m, { n: 0 }, "en"), "none");
  assert.equal(fmt(m, { n: 1 }, "en"), "one");
  assert.equal(fmt(m, { n: 2 }, "en"), "many");
});
test("string selector and default", () => {
  const m = [{ sel: "g", cases: { f: ["she"], m: ["he"], o: ["they"] }, def: "o" }];
  assert.equal(fmt(m, { g: "f" }, "en"), "she");
  assert.equal(fmt(m, { g: "x" }, "en"), "they");
  assert.equal(fmt(m, {}, "en"), "they");
});
test("nested select", () => {
  const inner = [{ sel: "n", cases: { one: ["1 post"], other: [{ v: "n" }, " posts"] }, def: "other" }];
  const m = [
    { sel: "who", cases: { me: ["You have ", ...inner], other: [{ v: "name" }, " has ", ...inner] }, def: "other" },
  ];
  assert.equal(fmt(m, { who: "me", n: 3 }, "en"), "You have 3 posts");
  assert.equal(fmt(m, { who: "x", name: "Bo", n: 1 }, "en"), "Bo has 1 post");
});
test("number formatting", () => {
  const m = [{ v: "n", num: true }];
  assert.equal(fmt(m, { n: 1234567 }, "en"), new Intl.NumberFormat("en").format(1234567));
  assert.equal(fmt(m, { n: 1234567 }, "pl"), new Intl.NumberFormat("pl").format(1234567));
  assert.notEqual(fmt(m, { n: 1234567 }, "en"), "1234567");
  assert.equal(fmt([{ v: "n" }], { n: 1234567 }, "en"), "1234567");
});
