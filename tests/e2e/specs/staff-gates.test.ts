// Staff areas are refused on the server before anything renders (CLAUDE.md "Staff areas").
import { beforeAll, describe, expect, it } from "vitest";
import { Client, useForum } from "../lib/forum.ts";
import { seed, type Seed } from "../lib/seed.ts";

const forum = useForum();
let s: Seed, alice: Client, bob: Client, guest: Client, readKey: string;

const keyOf = (r: { json(): any }) => {
  const j = r.json();
  return String(j.key ?? j.token ?? j.api_key ?? "");
};

beforeAll(async () => {
  s = await seed(forum);
  ({ alice, bob, guest } = s);
  const k = await alice.api("/api/v1/me/api-keys", { name: "readonly", scope: "read" });
  expect(k.status).toBeLessThan(300);
  readKey = keyOf(k);
  expect(readKey, `no key in ${k.text}`).toMatch(/^bf_/);
});

const adminPages = ["/admin", "/admin/", "/admin/users", "/admin/settings", "/admin/_app/version.json", "/admin/favicon.png"];
const staffPages = ["/modcp", "/review", "/review/dsa", "/review/deleted", "/review/messages"];
const adminApi: [string, string][] = [
  ["GET", "/api/v1/admin/stats"],
  ["GET", "/api/v1/admin/users"],
  ["GET", "/api/v1/admin/settings"],
  ["GET", "/api/v1/admin/webhooks"],
  ["POST", "/api/v1/admin/groups"],
  ["POST", "/api/v1/admin/webhooks"],
  ["POST", "/api/v1/admin/nodes"],
  ["PUT", "/api/v1/admin/settings/site_name"],
  ["DELETE", "/api/v1/admin/webhooks/1"],
];
const visitors = [["a guest", () => guest], ["a member", () => bob]] as const;

describe("the admin panel is a 404 to anyone but an administrator", () => {
  for (const [who, get] of visitors) {
    it(`${who}: every page and bundle is the site 404, with no redirect`, async () => {
      expect((await get().get("/no-such-page-at-all")).status).toBe(404);
      for (const p of adminPages) {
        const r = await get().get(p);
        expect(r.status, p).toBe(404);
        expect(r.location, `${p} redirected (it would confirm the panel exists)`).toBe("");
        expect(r.text, p).not.toMatch(/_app\/immutable|acp-/);
      }
    });
  }

  it("an administrator is served (control)", async () => {
    expect((await alice.get("/admin/")).status).toBe(200);
  });

  it("an administrator's read-only API key is not enough for the panel", async () => {
    expect((await forum.client().get("/admin/", { headers: { authorization: `Bearer ${readKey}` } })).status).toBe(404);
  });
});

describe("the admin API is refused before any handler or parser runs", () => {
  for (const [who, get] of visitors) {
    it(`${who}: 403 on every route, malformed bodies too, with no field names leaked`, async () => {
      for (const [method, path] of adminApi) {
        const good = method === "GET" ? await get().get(path) : await get().api(path, { name: "x" }, { method });
        expect(good.status, `${method} ${path}`).toBe(403);
        const rs = [good];
        if (method !== "GET") {
          const junk = await get().send(path, { method, body: "{not json", headers: { "content-type": "application/json", "x-csrf-token": get().csrf } });
          expect(junk.status, `${method} ${path} with junk`).toBe(403);
          const form = await get().send(path, { method, body: "a=b", headers: { "content-type": "application/x-www-form-urlencoded" } });
          expect(form.status, `${method} ${path} with a form`).toBe(403);
          rs.push(junk, form);
        }
        for (const r of rs) expect(r.text, path).not.toMatch(/missing field|expected|unknown field|invalid type/i);
      }
    });
  }

  it("a member's own write-scope API key is not an admin credential", async () => {
    const key = keyOf(await bob.api("/api/v1/me/api-keys", { name: "w", scope: "write" }));
    expect(key).toMatch(/^bf_/);
    for (const [method, path] of adminApi) {
      const h = { authorization: `Bearer ${key}` };
      const r = method === "GET" ? await forum.client().get(path, { headers: h }) : await forum.client().api(path, { name: "x" }, { method, noCsrf: true, headers: h });
      expect(r.status, `${method} ${path}`).toBe(403);
    }
  });

  it("an administrator's read-only key cannot use the admin API", async () => {
    expect((await forum.client().get("/api/v1/admin/stats", { headers: { authorization: `Bearer ${readKey}` } })).status).toBe(403);
  });

  it("a bogus bearer token is 403, not a server error", async () => {
    expect((await forum.client().get("/api/v1/admin/stats", { headers: { authorization: "Bearer bf_not-a-real-key" } })).status).toBe(403);
  });

  it("an administrator is served, and the refusals changed nothing (control)", async () => {
    expect((await alice.get("/api/v1/admin/stats")).status).toBe(200);
    expect((await forum.sql("SELECT count(*)::int AS n FROM groups WHERE name = 'x'")).rows[0].n).toBe(0);
  });
});

describe("server-rendered staff pages answer 403/404 before building a layout", () => {
  for (const [who, get] of visitors) {
    it(`${who}`, async () => {
      for (const p of staffPages) {
        const r = await get().get(p);
        expect([403, 404], `${p} gave ${r.status}`).toContain(r.status);
        expect(r.location, `${p} redirected`).toBe("");
        expect(r.text, p).not.toMatch(/Review queue|Moderator panel|name="kind"|data-review/i);
      }
    });
  }

  it("staff reach them (control)", async () => {
    for (const p of ["/modcp", "/review"]) expect((await alice.get(p)).status, p).toBe(200);
  });

  it("a member cannot decide a review item", async () => {
    const posts: [string, Record<string, string>][] = [
      ["/review/1", { decision: "approve" }],
      ["/review/1/claim", {}],
      ["/review/held/1", { decision: "approve" }],
      ["/review/dsa/notice/1", { decision: "remove", reason: "because" }],
    ];
    for (const [p, body] of posts) {
      const r = await bob.form(p, body);
      expect([403, 404], `POST ${p} gave ${r.status}`).toContain(r.status);
    }
  });
});
