// Sudo mode, __Host- cookies, session rotation and API key expiry.
import { beforeAll, describe, expect, it } from "vitest";
import { Client, until, useForum } from "../lib/forum.ts";

const forum = useForum();
const PASSWORD = "correct horse battery staple";
let c: Client;

describe("account security", () => {
  beforeAll(async () => {
    c = forum.client();
    await c.get("/register");
    await c.form("/register", { username: "sudoer", email: "sudoer@example.com", password: PASSWORD });
    await c.get("/login");
    await c.form("/login", { login: "sudoer", password: PASSWORD });
  });

  it("uses __Host- cookies", () => {
    expect(c.jar.has("__Host-bf_session")).toBe(true);
    expect(c.jar.has("__Host-bf_csrf")).toBe(true);
    expect(c.jar.has("bf_session")).toBe(false);
  });

  it("confirming rotates the session, and refuses a wrong password", async () => {
    const before = c.jar.get("__Host-bf_session");
    expect((await c.form("/sudo?next=/settings", { password: "not the password" })).status).toBe(422);
    expect(c.jar.get("__Host-bf_session")).toBe(before);
    const ok = await c.form("/sudo?next=/settings", { password: PASSWORD });
    expect(ok.status).toBe(303);
    expect(ok.location).toContain("/settings");
    expect(c.jar.get("__Host-bf_session")).not.toBe(before);
    expect((await c.get("/settings")).status).toBe(200);
  });

  it("does not redirect away from the site", async () => {
    const ok = await c.form("/sudo?next=//evil.example/", { password: PASSWORD });
    expect(ok.location).toContain("/settings");
    expect(ok.location).not.toContain("evil.example");
  });

  it("gives new API keys an expiry", async () => {
    const made = await c.form("/settings/api-keys", { name: "ci", scope: "read", days: "30" });
    expect(made.status).toBe(201);
    expect((await c.get("/settings")).text).toMatch(/expires \d{4}-\d{2}-\d{2}/);
  });
});

describe("admin badges", () => {
  it("saves a new badge's icon, and restyles a built-in one", async () => {
    const made = await c.api("/api/v1/admin/badges", { name: "Pathfinder", category: "Custom", tier: "gold", icon: "rocket", color: 4, level: 5, min_posts: 1, points: 2, enabled: true });
    expect(made.status).toBe(201);
    let list = JSON.parse((await c.get("/api/v1/admin/badges")).text);
    expect(list.find((b: any) => b.name === "Pathfinder")).toMatchObject({ icon: "rocket", color: 4, level: 5, tier: "gold" });
    const own = list.find((b: any) => b.slug === "first_like");
    const edited = await c.api(`/api/v1/admin/badges/${own.id}`, { ...own, name: "Liked It", icon: "star", color: 2 }, { method: "PUT" });
    expect(edited.status).toBe(204);
    list = JSON.parse((await c.get("/api/v1/admin/badges")).text);
    expect(list.find((b: any) => b.id === own.id)).toMatchObject({ name: "Liked It", icon: "star", color: 2, slug: "first_like" });
    const bad = await c.api("/api/v1/admin/badges", { name: "Bad", category: "Custom", tier: "gold", icon: "../x", min_posts: 1 });
    expect(bad.status).toBe(422);
  });
});

describe("admin dashboard", () => {
  it("has 30 days of numbers, for admins only", async () => {
    expect((await forum.client().get("/api/v1/admin/analytics")).status).toBe(403);
    const res = await c.get("/api/v1/admin/analytics");
    expect(res.status).toBe(200);
    const body = JSON.parse(res.text);
    expect(body.daily.length).toBe(30);
    expect(body.active_30d).toBeGreaterThan(0);
    expect(Array.isArray(body.top_forums)).toBe(true);
  });

  it("counts page views, people apart from crawlers, and refuses non-admins", async () => {
    const ua = { "user-agent": "Mozilla/5.0 Firefox/130", referer: "https://www.example.org/post" };
    for (const path of ["/", "/login"]) expect((await forum.client().get(path, { headers: ua })).status).toBe(200);
    await forum.client().get("/", { headers: { "user-agent": "Googlebot/2.1" } });
    expect((await forum.client().get("/api/v1/admin/traffic")).status).toBe(403);
    // Views are stored in batches a few seconds after the page is served.
    const body = await until(
      async () => JSON.parse((await c.get("/api/v1/admin/traffic?days=7")).text),
      (t) => t.views >= 2 && t.daily.at(-1).crawler >= 1,
    );
    expect(body.daily.length).toBe(7);
    expect(body.referrers.map((r: any) => r.name)).toContain("example.org");
    expect(body.entries.map((r: any) => r.name)).toContain("/");
    expect(body.bounce_rate).not.toBeNull();
  });
});

describe("admin panel header", () => {
  it("is the site's header, for admins only", async () => {
    expect((await forum.client().get("/api/v1/admin/shell")).status).toBe(403);
    const shell = await c.get("/api/v1/admin/shell");
    expect(shell.status).toBe(200);
    const body = JSON.parse(shell.text);
    expect(body.html).toContain('class="site-header"');
    expect(body.html).toContain("/admin/");
    expect(body.css.length).toBeGreaterThan(0);
    expect(body.critical).toContain(".bf-header-bar");
    expect(body.loader).toContain("import(");
  });
  it("carries the CSRF token the site's scripts need, the same one the panel's API calls use", async () => {
    // The header's islands and the Markdown editor read it from a meta tag the panel adds; without it
    // their requests have no token and fail with "Your session expired".
    const body = JSON.parse((await c.get("/api/v1/admin/shell")).text);
    expect(body.csrf).toMatch(/^[0-9a-f]{64}$/);
    expect(body.csrf).toBe(JSON.parse((await c.get("/api/v1/csrf")).text).token);
  });
});
