import test from "node:test";
import assert from "node:assert/strict";

// The token's cookie is HttpOnly, so the panel asks for it once and keeps it for its writes.
const calls = [];
globalThis.fetch = async (url, init = {}) => {
  calls.push({ url, init });
  if (url === "/api/v1/csrf") return new Response(JSON.stringify({ token: "a".repeat(64) }), { status: 200 });
  return new Response(null, { status: 204 });
};
globalThis.document = { documentElement: { lang: "en" }, getElementById: () => null };

const { ensureCsrf, csrf } = await import("../src/lib/csrf.js");

test("the token is empty until it is loaded, then loaded once", async () => {
  assert.equal(csrf(), "");
  const [a, b] = await Promise.all([ensureCsrf(), ensureCsrf()]);
  assert.equal(a, "a".repeat(64));
  assert.equal(b, a);
  assert.equal(csrf(), a);
  await ensureCsrf();
  assert.equal(calls.filter((c) => c.url === "/api/v1/csrf").length, 1);
});
