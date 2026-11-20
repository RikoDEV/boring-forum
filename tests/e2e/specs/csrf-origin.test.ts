// CSRF token and Origin enforcement on state-changing form and JSON endpoints.
// Tokens are checked by each handler; the cross-origin rule is the `origin_check` middleware, which
// answers 403 before any handler runs.
import { beforeAll, describe, expect, it } from "vitest";
import { Client, useForum } from "../lib/forum.ts";
import { seed, topic, type Seed } from "../lib/seed.ts";

const forum = useForum();
let s: Seed, alice: Client, bob: Client, T: string;

beforeAll(async () => {
  s = await seed(forum);
  ({ alice, bob } = s);
  T = (await topic(alice, s.FORUM, "CSRF target topic")).id;
});

type Case = { name: string; run: (c: Client, o: { headers?: Record<string, string>; noCsrf?: boolean; token?: string }) => Promise<{ status: number }>; as: () => Client };

const formCase = (name: string, path: () => string, fields: Record<string, string>): Case => ({
  name,
  as: () => bob,
  run: (c, o) => c.form(path(), o.token !== undefined ? { ...fields, csrf: o.token } : fields, { headers: o.headers, noCsrf: o.noCsrf }),
});
const apiCase = (name: string, path: string, body: unknown, who: () => Client, method = "POST"): Case => ({
  name,
  as: who,
  run: (c, o) => c.api(path, body, { method, headers: { ...(o.token !== undefined ? { "x-csrf-token": o.token } : {}), ...o.headers }, noCsrf: o.noCsrf || o.token !== undefined }),
});

const cases = (): Case[] => [
  formCase("reply form", () => `/t/${T}/reply`, { body: "A reply that is long enough" }),
  formCase("new topic form", () => `/c/${s.FORUM}/new`, { title: "Csrf made topic", body: "A first post long enough to pass" }),
  formCase("notification settings form", () => "/settings/notifications", { reply_app: "on" }),
  formCase("avatar removal form", () => "/settings/avatar/remove", {}),
  apiCase("member API key (JSON)", "/api/v1/me/api-keys", { name: "csrf", scope: "read" }, () => bob),
  apiCase("draft save (JSON PUT)", `/api/v1/drafts?context=reply:${T}`, { body: "draft" }, () => bob, "PUT"),
  apiCase("admin group create (JSON)", "/api/v1/admin/groups", { name: "CsrfGroup" }, () => alice),
];

const cross = [
  ["a foreign Origin", { origin: "https://evil.example" }],
  ["Origin: null (sandboxed frame)", { origin: "null" }],
  ["a lookalike of our own origin", { origin: "" }], // filled in per run: base + suffix
  ["Sec-Fetch-Site: cross-site", { "sec-fetch-site": "cross-site" }],
] as const;

describe("a missing or wrong token is refused", () => {
  it("without a token", async () => {
    for (const c of cases()) {
      const r = await c.run(c.as(), { noCsrf: true });
      // 400 from the token check, or 422 where the form extractor sees the missing field first.
      expect([400, 422], `${c.name} accepted a request with no token (${r.status})`).toContain(r.status);
    }
  });

  it("with a garbage token, and with an empty one", async () => {
    for (const c of cases()) {
      for (const token of ["not-the-token", ""]) {
        const r = await c.run(c.as(), { token });
        expect(r.status, `${c.name} accepted token ${JSON.stringify(token)}`).toBe(400);
      }
    }
  });

  it("with another member's valid token", async () => {
    // alice's token is perfectly valid for alice, but bob's session must not accept it.
    const r = await bob.form(`/t/${T}/reply`, { csrf: alice.csrf, body: "Replying with a stolen token" });
    expect(r.status).toBe(400);
    const j = await bob.api("/api/v1/me/api-keys", { name: "stolen", scope: "read" }, { headers: { "x-csrf-token": alice.csrf }, noCsrf: true });
    expect(j.status).toBe(400);
  });

  it("a guest's token does not work for a member session", async () => {
    const g = forum.client();
    await g.get("/login");
    expect(g.csrf).not.toBe("");
    const r = await bob.form(`/t/${T}/reply`, { csrf: g.csrf, body: "Replying with a guest token" });
    expect(r.status).toBe(400);
  });

  it("login and registration need one too", async () => {
    const g = forum.client();
    await g.get("/login");
    expect((await g.form("/login", { login: "bob", password: "whatever long enough" }, { noCsrf: true })).status).toBeGreaterThanOrEqual(400);
    await g.get("/register");
    expect((await g.form("/register", { username: "nocsrf", email: "nocsrf@example.com", password: "another long passphrase" }, { noCsrf: true })).status).toBeGreaterThanOrEqual(400);
  });

  it("a valid token is accepted (control)", async () => {
    const ok = await bob.form(`/t/${T}/reply`, { body: "A reply with the right token" });
    expect(ok.status).toBe(303);
    expect((await bob.api("/api/v1/me/api-keys", { name: "good", scope: "read" })).status).toBeLessThan(300);
  });
});

describe("a cross-origin request is refused, even with a valid token", () => {
  for (const [label, hdr] of cross) {
    it(label, async () => {
      const headers: Record<string, string> = { ...hdr };
      if ("origin" in hdr && hdr.origin === "") headers.origin = `${forum.base}.evil.example`;
      for (const c of cases()) {
        const r = await c.run(c.as(), { headers });
        expect(r.status, `${c.name} went through with ${label}`).toBe(403);
      }
    });
  }

  it("is refused before the token is even looked at", async () => {
    const r = await bob.form(`/t/${T}/reply`, { body: "x" }, { noCsrf: true, headers: { origin: "https://evil.example" } });
    expect(r.status).toBe(403);
  });

  it("nothing was written by the refused requests", async () => {
    const n = await forum.sql("SELECT count(*)::int AS n FROM items WHERE title = 'Csrf made topic'");
    expect(n.rows[0].n).toBe(0);
    const g = await forum.sql("SELECT count(*)::int AS n FROM groups WHERE name = 'CsrfGroup'");
    expect(g.rows[0].n).toBe(0);
  });

  it("our own origin, and same-origin fetch metadata, are accepted (control)", async () => {
    let n = 0;
    for (const headers of [{ origin: forum.base }, { origin: forum.base, "sec-fetch-site": "same-origin" }, { "sec-fetch-site": "same-origin" }]) {
      const r = await bob.form(`/t/${T}/reply`, { body: `A same-origin reply number ${++n}, long enough` }, { headers });
      expect(r.status, JSON.stringify(headers)).toBe(303);
    }
  });

  it("reading is not blocked by origin (only writes are)", async () => {
    expect((await bob.get("/", { headers: { origin: "https://evil.example", "sec-fetch-site": "cross-site" } })).status).toBe(200);
  });
});

describe("webhook-style endpoints exempt from Origin still need their own secret", () => {
  it("inbound mail and bounce webhooks refuse a call with no token", async () => {
    for (const path of ["/api/v1/webhooks/inbound-mail", "/api/v1/webhooks/bounce"]) {
      const g = forum.client();
      const r = await g.api(path, { from: "x@example.com" }, { noCsrf: true, headers: { origin: "https://evil.example" } });
      expect(r.status, `${path} did not refuse`).toBeGreaterThanOrEqual(400);
      expect(r.status, `${path}: the Origin gate should not be what answers`).not.toBe(500);
    }
  });
});
