import assert from "node:assert/strict";
import { test } from "node:test";
import { EMOTICONS, ENDS, emoticonBefore } from "../src/emoticons.js";

test("an emoticon is found only as a word of its own", () => {
  assert.deepEqual(emoticonBefore("hello :)", 8), [6, EMOTICONS[":)"]]);
  assert.deepEqual(emoticonBefore(":D", 2), [0, EMOTICONS[":D"]]);
  assert.deepEqual(emoticonBefore("a\n<3", 4), [2, EMOTICONS["<3"]]);
  assert.equal(emoticonBefore("a:)", 3), null);
  assert.equal(emoticonBefore("see 10:30", 9), null);
  assert.equal(emoticonBefore("http://x.test/:)", 16), null);
  assert.equal(emoticonBefore("option B)", 9), null);
  assert.equal(emoticonBefore("plain text", 10), null);
  assert.equal(emoticonBefore("", 0), null);
});

test("a space, a full stop or the like ends an emoticon; a letter does not", () => {
  for (const c of [" ", "\n", ".", ",", "!", "?"]) assert.ok(ENDS.test(c), JSON.stringify(c));
  for (const c of ["a", ")", ":", ""]) assert.ok(!ENDS.test(c), JSON.stringify(c));
});
