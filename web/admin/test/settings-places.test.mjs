// Every setting the server defines is edited in exactly one place in the panel: in a group on the
// Settings page, among the settings of a feature's page, or by that page's own controls. A new
// setting fails here until it is given a home.
import test from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { CODE, OWN_CONTROLS, keysOn, meta, movedGroup, placeOf } from "../src/lib/settings-meta.js";

const defs = new URL("../../../crates/bf-core/src/settings/defs/", import.meta.url);
const defined = new Set(
  readdirSync(defs).flatMap((file) =>
    [
      ...readFileSync(new URL(file, defs), "utf8").matchAll(
        /(?:\b(?:def|int|text|flag|switch|number)\(\s*|\bkey: )"([a-z][a-z0-9_]+)"/g,
      ),
    ].map((m) => m[1]),
  ),
);
const { GROUPS, ICONS } = meta();
const grouped = GROUPS.flatMap(([, , , keys]) => keys);

test("the server's settings were found", () => {
  assert.ok(defined.size > 280, String(defined.size));
});

test("each setting has exactly one home", () => {
  const homeless = [];
  const doubled = [];
  for (const key of defined) {
    const inGroup = grouped.filter((k) => k === key).length;
    const elsewhere = placeOf(key) ? 1 : 0;
    if (inGroup + elsewhere === 0) homeless.push(key);
    if (inGroup + elsewhere > 1) doubled.push(key);
  }
  assert.deepEqual(homeless, [], "give these a group in Settings, or a page");
  assert.deepEqual(doubled, [], "these can be edited in two places");
});

test("nothing is listed that the server does not have", () => {
  // Custom code settings are listed through CODE; rank names have an editor of their own.
  const unknown = [...grouped, ...Object.keys(OWN_CONTROLS), ...Object.keys(CODE)].filter((k) => !defined.has(k));
  assert.deepEqual(unknown, []);
});

test("every group has something in it and an icon", () => {
  for (const [id, , , keys] of GROUPS) {
    assert.ok(keys.length, id);
    assert.ok(ICONS[id], `${id} has no icon`);
  }
});

test("a setting on a feature's page is found there, and old links lead to it", () => {
  assert.equal(placeOf("mail_accent_colour"), "/mail#setting=mail_accent_colour");
  assert.equal(placeOf("ai_enabled"), "/ai#setting=ai_enabled");
  assert.equal(placeOf("backup_enabled"), "/backups");
  assert.equal(placeOf("plugin:poll:limit"), "/plugins#setting=plugin%3Apoll%3Alimit");
  // These two only sound like a feature's.
  assert.equal(placeOf("chat_webhook_url"), "");
  assert.equal(placeOf("ai_crawlers"), "");
  assert.equal(movedGroup("email"), "/mail#tab=settings");
  assert.ok(keysOn("/chat").has("chat_enabled"));
  assert.deepEqual(keysOn("/maintenance").order([{ key: "offline_message" }, { key: "offline" }]), [
    { key: "offline" },
    { key: "offline_message" },
  ]);
});
