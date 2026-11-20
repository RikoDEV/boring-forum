// Permalinks: redirects for addresses the forum does not know.
import { beforeAll, describe, expect, it } from "vitest";
import { Client, useForum } from "../lib/forum.ts";
import { seed, topic, type Seed } from "../lib/seed.ts";

const forum = useForum();
let s: Seed, alice: Client, bob: Client, guest: Client;
let topicId = "";
let topicPath = "";

const add = (body: unknown) => alice.api("/api/v1/admin/permalinks", body);
const go = async (path: string) => {
  const r = await guest.get(path);
  return { status: r.status, to: r.location ? new URL(r.location).pathname + new URL(r.location).search : null };
};

beforeAll(async () => {
  s = await seed(forum);
  ({ alice, bob, guest } = s);
  const t = await topic(bob, s.FORUM, "A topic that moved here", "The first post of the topic that moved here");
  topicId = t.id;
  topicPath = t.url;
});

describe("permalinks", () => {
  it("are for admins to manage", async () => {
    expect((await guest.get("/api/v1/admin/permalinks")).status).toBe(403);
    expect((await bob.get("/api/v1/admin/permalinks")).status).toBe(403);
    expect((await bob.api("/api/v1/admin/permalinks", { url: "x/1", kind: "url", target: "/latest" })).status).toBe(403);
  });

  it("send an address the forum does not know to what it points at, for good", async () => {
    expect((await go("/old/viewtopic.php")).status).toBe(404);
    expect((await add({ url: "/old/viewtopic.php/", kind: "topic", target_id: Number(topicId) })).status).toBe(201);
    expect(await go("/old/viewtopic.php")).toEqual({ status: 301, to: topicPath });
    // Slashes and case do not matter; the query string is looked up first.
    expect((await go("/OLD/viewtopic.php/")).to).toBe(topicPath);
    expect((await add({ url: "old/viewtopic.php?t=7", kind: "forum", target_id: s.FORUM })).status).toBe(201);
    expect((await go("/old/viewtopic.php?t=7")).to).toMatch(new RegExp(`^/c/[^/]+/${s.FORUM}$`));
    expect((await go("/old/viewtopic.php?t=8")).to).toBe(topicPath);
  });

  it("can point at a post, a member or any address", async () => {
    const post = Number((await guest.get(topicPath)).text.match(/data-post="(\d+)"/)![1]);
    expect((await add({ url: "old/post", kind: "post", target_id: post })).status).toBe(201);
    expect((await go("/old/post")).to).toBe(`/p/${post}`);
    expect((await add({ url: "old/people/bob", kind: "member", target: "@Bob" })).status).toBe(201);
    expect((await go("/old/people/bob")).to).toBe("/u/bob");
    expect((await add({ url: "old/latest", kind: "url", target: "/latest" })).status).toBe(201);
    expect((await go("/old/latest")).to).toBe("/latest");
    expect((await add({ url: "old/away", kind: "url", target: "https://example.org/elsewhere" })).status).toBe(201);
    expect((await guest.get("/old/away")).location).toBe("https://example.org/elsewhere");
  });

  it("can point at a tag, given by its name as the picker offers it or by its address name", async () => {
    const t = await bob.form(`/c/${s.FORUM}/new`, { title: "A topic with a tag", body: "The body of the topic that has a tag", tags: "Moved Tag" });
    expect(t.status).toBe(303);
    expect((await add({ url: "old/tags/moved", kind: "tag", target: "Moved Tag" })).status).toBe(201);
    const row = JSON.parse((await alice.get("/api/v1/admin/permalinks?q=old/tags")).text).permalinks[0];
    expect(row.target).toBe("moved-tag");
    expect((await go("/old/tags/moved")).to).toBe("/tag/moved-tag");
    expect((await add({ url: "old/tags/again", kind: "tag", target: "moved-tag" })).status).toBe(201);
    expect((await go("/old/tags/again")).to).toBe("/tag/moved-tag");
  });

  it("refuse what could not work or could do harm", async () => {
    for (const bad of [
      { url: "", kind: "url", target: "/latest" },
      { url: "x/1", kind: "topic", target_id: 999999 },
      { url: "x/1", kind: "forum" },
      { url: "x/1", kind: "tag", target: "no-such-tag" },
      { url: "x/1", kind: "member", target: "nobody-here" },
      { url: "x/1", kind: "url", target: "javascript:alert(1)" },
      { url: "x/1", kind: "url", target: "//evil.example/x" },
      { url: "x/1", kind: "url", target: "/x/1" },
      { url: "api/v1/topics", kind: "url", target: "/latest" },
      { url: "admin/forums", kind: "url", target: "/latest" },
      { url: "https://elsewhere.example/a", kind: "url", target: "/latest" },
      { url: "x/1", kind: "nonsense", target: "/latest" },
    ])
      expect((await add(bad)).status, JSON.stringify(bad)).toBe(422);
    // An address has one permalink, whatever its case.
    expect((await add({ url: "OLD/LATEST", kind: "url", target: "/latest" })).status).toBe(422);
  });

  it("are listed with where they lead, can be edited and deleted", async () => {
    const list = JSON.parse((await alice.get("/api/v1/admin/permalinks")).text).permalinks;
    const row = list.find((p: any) => p.url === "old/viewtopic.php");
    expect(row).toMatchObject({ kind: "topic", destination: topicPath, missing: false });
    expect(JSON.parse((await alice.get("/api/v1/admin/permalinks?q=viewtopic")).text).permalinks.length).toBe(2);
    expect((await alice.api(`/api/v1/admin/permalinks/${row.id}`, { url: "old/moved", kind: "url", target: "/top" }, { method: "PUT" })).status).toBe(204);
    expect((await go("/old/viewtopic.php")).status).toBe(404);
    expect((await go("/old/moved")).to).toBe("/top");
    expect((await alice.api(`/api/v1/admin/permalinks/${row.id}`, undefined, { method: "DELETE" })).status).toBe(204);
    expect((await go("/old/moved")).status).toBe(404);
  });
});
