// Settings of a theme: the colour schemes it shows, the groups it is limited to, whether members
// can pick it, and turning the built-in look off once another theme is the default.
import { beforeAll, describe, expect, it } from "vitest";
import { Client, useForum } from "../lib/forum.ts";
import { seed, setting, type Seed } from "../lib/seed.ts";

const forum = useForum();
let s: Seed, alice: Client, bob: Client, guest: Client;
let id = 0;

const themeLink = (html: string) => html.match(/<link rel="stylesheet" href="(\/theme\/[0-9a-f]+\.css)">/)?.[1];
const css = async (c: Client) => (await c.get(themeLink((await c.get("/")).text)!)).text;
const patch = (body: unknown, theme = id) => alice.api(`/api/v1/admin/themes/${theme}`, body, { method: "PATCH" });
const list = async () => JSON.parse((await alice.get("/api/v1/admin/themes")).text);

beforeAll(async () => {
  s = await seed(forum);
  ({ alice, bob, guest } = s);
  const made = await alice.api("/api/v1/admin/themes", { slug: "night", name: "Night", kind: "theme" });
  expect(made.status).toBe(201);
  id = JSON.parse(made.text).id;
});

describe("theme settings", () => {
  it("keep the built-in look until another theme is the site's default", async () => {
    expect((await list()).builtin_enabled).toBe(true);
    // No default theme yet: the built-in look is what everyone has.
    expect((await setting(alice, "builtin_theme_enabled", false)).status).toBe(422);
    expect((await setting(alice, "default_theme_id", id)).status).toBe(204);
    expect(themeLink((await bob.get("/")).text)).toBeDefined();
  });

  it("show the colour schemes the theme chose, and refuse ones that do not exist", async () => {
    expect((await patch({ light_scheme: "nope" })).status).toBe(422);
    expect((await patch({ group_ids: [0] })).status).toBe(422);
    expect((await patch({ light_scheme: "forest", dark_scheme: "plum" })).status).toBe(204);
    const sheet = await css(guest);
    expect(sheet).toContain("--bf-color-accent:light-dark(#17693c,#cfa1ea)");
    // A scheme a theme uses cannot be deleted from under it.
    const schemes = JSON.parse((await alice.get("/api/v1/admin/color-schemes")).text).schemes;
    const forest = schemes.find((x: any) => x.slug === "forest");
    expect((await alice.api(`/api/v1/admin/color-schemes/${forest.id}`, undefined, { method: "DELETE" })).status).toBe(422);
    // Cleared, it follows the site's defaults again.
    expect((await patch({ light_scheme: "", dark_scheme: "" })).status).toBe(204);
    expect(await css(guest)).not.toContain("--bf-color-accent:light-dark(#17693c,#cfa1ea)");
  });

  it("can be limited to groups: others are not offered it and do not get it", async () => {
    expect((await patch({ group_ids: [4] })).status).toBe(204);
    // Administrators (group 4) have it; a member and a guest do not, though it is the site's default.
    expect(themeLink((await alice.get("/")).text)).toBeDefined();
    expect(themeLink((await bob.get("/")).text)).toBeUndefined();
    expect(themeLink((await guest.get("/")).text)).toBeUndefined();
    expect((await bob.form("/settings/theme", { theme: String(id), scheme: "" })).status).toBe(422);
    expect((await alice.form("/settings/theme", { theme: String(id), scheme: "" })).status).toBe(303);
    expect((await patch({ group_ids: [] })).status).toBe(204);
    expect(themeLink((await bob.get("/")).text)).toBeDefined();
  });

  it("can be hidden from members' choices", async () => {
    expect((await patch({ selectable: false })).status).toBe(204);
    expect((await bob.form("/settings/theme", { theme: String(id), scheme: "" })).status).toBe(422);
    expect((await patch({ selectable: true })).status).toBe(204);
    expect((await bob.form("/settings/theme", { theme: String(id), scheme: "" })).status).toBe(303);
  });

  it("let the built-in look go, and keep a default theme in place while it is gone", async () => {
    expect((await setting(alice, "builtin_theme_enabled", false)).status).toBe(204);
    expect((await list()).builtin_enabled).toBe(false);
    expect((await setting(alice, "default_theme_id", 0)).status).toBe(422);
    expect((await patch({ enabled: false })).status).toBe(422);
    expect((await alice.api(`/api/v1/admin/themes/${id}`, undefined, { method: "DELETE" })).status).toBe(422);
    // Someone who may not use the default still gets a theme rather than the built-in look.
    const other = await alice.api("/api/v1/admin/themes", { slug: "day", name: "Day", kind: "theme", css: ".day{color:red}" });
    const dayId = JSON.parse(other.text).id;
    expect((await patch({ group_ids: [4] })).status).toBe(204);
    expect(await css(bob)).toContain(".day{");
    // Back on, the default can be the built-in look again.
    expect((await setting(alice, "builtin_theme_enabled", true)).status).toBe(204);
    expect((await setting(alice, "default_theme_id", 0)).status).toBe(204);
    expect((await patch({ group_ids: [] })).status).toBe(204);
    expect((await alice.api(`/api/v1/admin/themes/${dayId}`, undefined, { method: "DELETE" })).status).toBe(204);
  });
});
