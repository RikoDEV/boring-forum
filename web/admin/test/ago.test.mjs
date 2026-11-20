import test from "node:test";
import assert from "node:assert/strict";
import { agoText } from "../src/lib/ago.js";

const now = Date.parse("2026-06-15T12:00:00Z");
test("relative within 30 days", () => {
  assert.equal(agoText("2026-06-15T11:59:50Z", "en", now), "now");
  assert.match(agoText("2026-06-15T11:00:00Z", "en", now), /1 hr/);
  assert.match(agoText("2026-06-13T12:00:00Z", "en", now), /2 days/);
});
test("a date after 30 days, nothing for none", () => {
  assert.match(agoText("2026-01-02T12:00:00Z", "en", now), /2026/);
  assert.equal(agoText(null, "en", now), "");
  assert.equal(agoText("nonsense", "en", now), "");
});
