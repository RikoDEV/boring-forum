// Staff notes on a post: kept apart from the post, seen and removed by staff only.
import { beforeAll, describe, expect, it } from "vitest";
import { Client, useForum } from "../lib/forum.ts";
import { seed, topic, type Seed } from "../lib/seed.ts";

const forum = useForum();
let s: Seed, alice: Client, bob: Client, carol: Client, guest: Client;
let post = 0;
let url = "";
const NOTE = "Check the other posts of this author before approving more";

const page = async (c: Client) => (await c.get(url)).text;
const noteId = (html: string) => html.match(/\/post\/\d+\/note\/(\d+)\/delete/)?.[1];

beforeAll(async () => {
  s = await seed(forum);
  ({ alice, bob, guest } = s);
  carol = await forum.member("carol");
  const t = await topic(bob, s.FORUM, "A topic with a staff note", "The first post, written by bob for this test");
  url = t.url;
  post = Number((await bob.get(url)).text.match(/data-post="(\d+)"/)![1]);
});

describe("staff notes on posts", () => {
  it("are added from the post's menu by staff, and refused to everyone else", async () => {
    expect((await alice.get(url)).text).toContain(`action="/post/${post}/note"`);
    for (const c of [bob, carol, guest]) expect((await c.get(url)).text).not.toContain(`/post/${post}/note`);
    expect((await bob.form(`/post/${post}/note`, { body: "my own note" })).status).toBe(403);
    expect((await carol.form(`/post/${post}/note`, { body: "a note" })).status).toBe(403);
    expect((await guest.form(`/post/${post}/note`, { body: "a note" })).status).toBeGreaterThanOrEqual(400);
    expect((await alice.form(`/post/${post}/note`, { body: "   " })).status).toBe(422);
    expect((await alice.form(`/post/${post}/note`, { body: "x".repeat(2001) })).status).toBe(422);
    expect((await alice.form(`/post/${post}/note`, { body: NOTE })).status).toBe(303);
  });

  it("are seen by staff alone: not by the post's author, not by anyone else", async () => {
    const staff = await page(alice);
    expect(staff).toContain(NOTE);
    expect(staff).toContain("bf-staff-note");
    for (const c of [bob, carol, guest]) {
      const html = await page(c);
      expect(html).not.toContain(NOTE);
      expect(html).not.toContain("bf-staff-note");
    }
    // Not in the post's own text, either: its author editing it sees only what they wrote.
    expect((await bob.get(`/post/${post}/history`)).text).not.toContain(NOTE);
  });

  it("cannot be removed by the post's author or anyone who is not staff", async () => {
    const id = noteId(await page(alice))!;
    for (const c of [bob, carol]) expect((await c.form(`/post/${post}/note/${id}/delete`, {})).status).toBe(403);
    expect(await page(alice)).toContain(NOTE);
  });

  it("go on when the post's author deletes it, and staff can remove them", async () => {
    const id = noteId(await page(alice))!;
    // A reply of bob's, with a note of its own; he deletes it and the note stays.
    const topicId = url.match(/\/t\/[^/]+\/(\d+)/)![1];
    expect((await bob.form(`/t/${topicId}/reply`, { body: "A reply that bob will delete again" })).status).toBe(303);
    const reply = Number([...(await bob.get(url)).text.matchAll(/data-post="(\d+)"/g)].map((m) => m[1]).at(-1));
    expect(reply).not.toBe(post);
    expect((await alice.form(`/post/${reply}/note`, { body: "Second note, on the reply" })).status).toBe(303);
    expect((await bob.form(`/post/${reply}/delete`, {})).status).toBeLessThan(400);
    expect(await page(alice)).toContain("Second note, on the reply");
    // A note id of another post is not found, and nothing is removed.
    expect((await alice.form(`/post/${reply}/note/${id}/delete`, {})).status).toBeGreaterThanOrEqual(400);
    expect(await page(alice)).toContain(NOTE);
    expect((await alice.form(`/post/${post}/note/${id}/delete`, {})).status).toBe(303);
    expect(await page(alice)).not.toContain(NOTE);
  });
});
