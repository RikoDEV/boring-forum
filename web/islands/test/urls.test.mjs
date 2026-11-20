import test from "node:test";
import assert from "node:assert/strict";
import { extractUrls } from "../src/editor/urls.js";

test("finds distinct links and stops at closing brackets", () => {
  assert.deepEqual(extractUrls("a https://x.io/a [b](https://y.io/b) https://x.io/a"), [
    "https://x.io/a",
    "https://y.io/b",
  ]);
});
test("no links", () => {
  assert.deepEqual(extractUrls("plain"), []);
  assert.deepEqual(extractUrls(undefined), []);
});
