// Colour schemes: admins edit palettes, the site's default reaches everyone, members pick one.
import { beforeAll, describe, expect, it } from "vitest";
import { Client, useForum } from "../lib/forum.ts";
import { seed, type Seed } from "../lib/seed.ts";

const forum = useForum();
let s: Seed, alice: Client, bob: Client, guest: Client;

const link = (html: string) => html.match(/<link rel="stylesheet" href="(\/scheme\/[0-9a-f]+\.css)">/)?.[1];
const sheet = async (c: Client) => (await c.get(link((await c.get("/")).text)!)).text;
const json = (r: { text: string }) => JSON.parse(r.text);
const api = (c: Client, path: string, body: unknown, method = "POST") => c.api(`/api/v1/admin/color-schemes${path}`, body, { method });

beforeAll(async () => {
  s = await seed(forum);
  ({ alice, bob, guest } = s);
});

describe("colour schemes", () => {
  it("start with the three that used to be written into the CSS, and are for admins to edit", async () => {
    expect((await guest.get("/api/v1/admin/color-schemes")).status).toBe(403);
    expect((await bob.get("/api/v1/admin/color-schemes")).status).toBe(403);
    const r = json(await alice.get("/api/v1/admin/color-schemes"));
    expect(r.schemes.map((x: any) => x.slug)).toEqual(["standard", "forest", "plum"]);
    expect([r.default_light, r.default_dark]).toEqual(["standard", "standard"]);
    expect(r.base.length).toBe(11);
    expect(r.derived.length).toBeGreaterThan(0);
    expect(r.schemes[1].light.accent).toBe("#17693c");
  });

  it("are one stylesheet linked from every page, with the default applied to everyone", async () => {
    const css = await sheet(guest);
    expect(css).toContain(":root{--bf-color-bg:light-dark(#f5f6f8,#111419)");
    expect(css).toContain('[data-scheme="forest"]');
    expect(css).toContain("--bf-color-accent-soft:color-mix(");
    expect(link((await bob.get("/")).text)).toBe(link((await guest.get("/")).text));
  });

  it("can be made, edited, and refuse a colour that is not a plain hex value", async () => {
    const made = await api(alice, "", { name: "Night Owl", copy_from: 1 });
    expect(made.status).toBe(201);
    const scheme = json(made);
    expect(scheme.slug).toBe("night-owl");
    const edit = (light: any, name = "Night Owl", selectable = true) => api(alice, `/${scheme.id}`, { name, light, dark: scheme.dark, selectable }, "PUT");
    for (const bad of ["red", "#fff", "#12345g", "#123456;}body{display:none"])
      expect((await edit({ ...scheme.light, accent: bad })).status).toBe(422);
    expect((await edit({ ...scheme.light, accent: undefined })).status).toBe(422);
    expect((await edit(scheme.light, "  ")).status).toBe(422);
    expect((await edit({ ...scheme.light, accent: "#AB12CD" })).status).toBe(204);
    expect(await sheet(guest)).toContain('[data-scheme="night-owl"]');
    expect(await sheet(guest)).toContain("--bf-color-accent:light-dark(#ab12cd,");
    // The same name is a different address.
    expect(json(await api(alice, "", { name: "Night Owl" })).slug).toBe("night-owl-2");
  });

  it("are picked by members, and made the site's default by an admin", async () => {
    expect((await bob.form("/settings/theme", { theme: "", scheme: "night-owl" })).status).toBe(303);
    expect((await bob.get("/")).text).toMatch(/<html[^>]*data-scheme="night-owl"/);
    expect((await guest.get("/")).text).not.toContain("data-scheme=");
    const list = json(await alice.get("/api/v1/admin/color-schemes")).schemes;
    const owl = list.find((x: any) => x.slug === "night-owl");
    expect((await api(alice, "/default", { id: owl.id, light: true, dark: true }, "PUT")).status).toBe(204);
    expect(await sheet(guest)).toMatch(/^:root\{--bf-color-bg:[^}]*--bf-color-accent:light-dark\(#ab12cd,/);
    expect((await api(alice, "/default", { id: 99999, light: true }, "PUT")).status).toBe(404);
    // The default cannot be deleted; once another is, it can, and members who had it go back to the default.
    expect((await api(alice, `/${owl.id}`, undefined, "DELETE")).status).toBe(422);
    const standard = list.find((x: any) => x.slug === "standard");
    // Light and dark mode can have different defaults: one scheme's light palette, another's dark.
    expect((await api(alice, "/default", { id: standard.id, light: true }, "PUT")).status).toBe(204);
    expect((await api(alice, `/${owl.id}`, undefined, "DELETE")).status).toBe(422);
    expect(await sheet(guest)).toMatch(/^:root\{[^}]*--bf-color-accent:light-dark\(#0b64c0,#[0-9a-f]{6}\)/);
    expect((await api(alice, "/default", { id: standard.id, dark: true }, "PUT")).status).toBe(204);
    expect((await api(alice, `/${owl.id}`, undefined, "DELETE")).status).toBe(204);
    expect((await bob.get("/")).text).not.toContain("data-scheme=");
    expect(await sheet(guest)).not.toContain('data-scheme="night-owl"');
  });
});
