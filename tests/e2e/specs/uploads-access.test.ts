// Who may fetch an uploaded file, hotlinking, and what happens to files of a thrown-away draft.
import { beforeAll, describe, expect, it } from "vitest";
import { Client, useForum } from "../lib/forum.ts";
import { seed, setting, topic, type Seed } from "../lib/seed.ts";

const forum = useForum();
let s: Seed, alice: Client, bob: Client, carol: Client, guest: Client;

/** A file nobody else has uploaded: the text makes its content (and so its address) unique. */
const file = (name: string, text: string) => ({ name, data: text });
const upload = async (c: Client, f: { name: string; data: string }) => {
  const r = await c.multipart("/api/v1/uploads", { file: f });
  expect(r.status).toBe(201);
  return JSON.parse(r.text) as { url: string; markdown: string };
};
const fromSite = { "sec-fetch-site": "cross-site", "sec-fetch-dest": "image" };
/** A person following a link: a document is asked for, and may be sent without a signature. */
const nav = { "sec-fetch-mode": "navigate", "sec-fetch-dest": "document", "sec-fetch-site": "none" };
/** The first address of a file in a page, signed or not. */
const linkIn = (html: string, ext: string) => html.match(new RegExp(`/uploads/[0-9a-f]{64}\.${ext}[^"'\s<]*`))![0].replaceAll("&amp;", "&");

beforeAll(async () => {
  s = await seed(forum);
  ({ alice, bob, guest } = s);
  carol = await forum.member("carol");
});

describe("who may open a file", () => {
  it("a file used nowhere yet is open, as before", async () => {
    const f = await upload(bob, file("loose.txt", "not posted anywhere yet"));
    expect((await guest.get(f.url, { headers: nav })).status).toBe(200);
  });

  it("a file in a public post is open to everyone", async () => {
    const f = await upload(bob, file("public.txt", "in a public post"));
    await topic(bob, s.FORUM, "A post with a file", `Here it is: ${f.markdown}`);
    const r = await guest.get(f.url, { headers: nav });
    expect(r.status).toBe(200);
    // Only a signed link may be kept by shared caches; this plain address is for the one asking.
    expect(r.headers.get("cache-control")).toContain("private");
  });

  it("a file in a forum guests cannot read is not theirs, and never caches publicly", async () => {
    expect((await alice.api(`/api/v1/admin/nodes/${s.OFF}/permissions`, { 2: 95, 3: 127, 4: 127 }, { method: "PUT" })).status).toBe(204);
    const f = await upload(alice, file("members.txt", "for members of the off-topic forum"));
    await topic(alice, s.OFF, "Members only file", `Take it: ${f.markdown}`);
    expect((await guest.get(f.url, { headers: nav })).status).toBe(404);
    const r = await bob.get(f.url, { headers: nav });
    expect(r.status).toBe(200);
    expect(r.headers.get("cache-control")).toContain("private");
    expect(r.headers.get("vary")).toContain("Cookie");
  });

  it("and not to a member the forum is closed to", async () => {
    expect((await alice.api(`/api/v1/admin/nodes/${s.OFF}/permissions`, { 3: 127, 4: 127 }, { method: "PUT" })).status).toBe(204);
    const f = await upload(alice, file("staff.txt", "staff of the off-topic forum only"));
    await topic(alice, s.OFF, "Staff file", `Staff: ${f.markdown}`);
    expect((await bob.get(f.url, { headers: nav })).status).toBe(404);
    expect((await alice.get(f.url, { headers: nav })).status).toBe(200);
  });

  it("a forum can let members read and see pictures but not open attached files", async () => {
    const png = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==", "base64");
    const pic = JSON.parse((await alice.multipart("/api/v1/uploads", { file: { name: "inline.png", data: png } })).text) as { url: string; markdown: string };
    const doc = await upload(alice, file("nodownload.txt", "readable post, file not for members"));
    await topic(alice, s.OFF, "Download right", `Picture ${pic.markdown} and file ${doc.markdown}`);
    // Members: view, read, create, reply, attach (31) but not download (64).
    expect((await alice.api(`/api/v1/admin/nodes/${s.OFF}/permissions`, { 2: 31, 3: 127, 4: 127 }, { method: "PUT" })).status).toBe(204);
    expect((await bob.get(doc.url, { headers: nav })).status).toBe(404);
    expect((await bob.get(pic.url.split("#")[0])).status).toBe(200);
    expect((await alice.get(doc.url, { headers: nav })).status).toBe(200);
    expect((await alice.api(`/api/v1/admin/nodes/${s.OFF}/permissions`, { 2: 95, 3: 127, 4: 127 }, { method: "PUT" })).status).toBe(204);
    expect((await bob.get(doc.url, { headers: nav })).status).toBe(200);
  });

  it("a file in a private message is for its participants", async () => {
    const f = await upload(alice, file("pm.txt", "only for bob, from alice"));
    const r = await alice.form("/messages", { to: "bob", subject: "A file for you", body: `Here: ${f.markdown}` });
    expect(r.status).toBe(303);
    expect((await bob.get(f.url, { headers: nav })).status).toBe(200);
    expect((await carol.get(f.url, { headers: nav })).status).toBe(404);
    expect((await guest.get(f.url, { headers: nav })).status).toBe(404);
  });

  it("a file shared in a public post stays public even if it was also in a message", async () => {
    const f = await upload(alice, file("both.txt", "in a message and then in a post"));
    await alice.form("/messages", { to: "bob", subject: "First", body: `Here: ${f.markdown}` });
    expect((await carol.get(f.url, { headers: nav })).status).toBe(404);
    await topic(alice, s.FORUM, "Now public", `Now for all: ${f.markdown}`);
    expect((await carol.get(f.url, { headers: nav })).status).toBe(200);
  });

  it("members-only turns guests away from files in posts", async () => {
    const f = await upload(bob, file("memberscheck.txt", "a public post's file"));
    await topic(bob, s.FORUM, "Members only setting", `File: ${f.markdown}`);
    expect((await guest.get(f.url, { headers: nav })).status).toBe(200);
    expect((await setting(alice, "uploads_members_only", true)).status).toBe(204);
    expect((await guest.get(f.url, { headers: nav })).status).toBe(404);
    const r = await carol.get(f.url, { headers: nav });
    expect(r.status).toBe(200);
    expect(r.headers.get("cache-control")).toContain("private");
    expect((await setting(alice, "uploads_members_only", false)).status).toBe(204);
    expect((await guest.get(f.url, { headers: nav })).status).toBe(200);
  });
});

describe("links that expire", () => {
  let page = "";
  let link = "";
  it("a guest's page has signed links to files, a member's does not", async () => {
    const f = await upload(bob, file("expiring.txt", "a public file with an expiring link"));
    const t = await topic(bob, s.FORUM, "A file that expires", `Get it: ${f.markdown}`);
    page = (await guest.get(t.url)).text;
    link = linkIn(page, "txt");
    expect(link).toMatch(/\?x=\d+\.[0-9a-f]{32}$/);
    expect(linkIn((await bob.get(t.url)).text, "txt")).not.toContain("?x=");
    // The JSON API signs them too.
    expect((await guest.get(`/api/v1/topics/${t.id}`)).text).toMatch(/uploads\/[0-9a-f]{64}\.txt\?x=\d+\./);
  });

  it("the signed link works for a guest with nothing else to show", async () => {
    const r = await guest.get(link);
    expect(r.status).toBe(200);
    expect(r.headers.get("cache-control")).toMatch(/^public, max-age=\d+$/);
  });

  it("the same address without its signature, or with a changed one, does not", async () => {
    const bare = link.split("?")[0];
    expect((await guest.get(bare)).status).toBe(403);
    expect((await guest.get(link.replace(/\.[0-9a-f]{32}$/, `.${"0".repeat(32)}`))).status).toBe(403);
    expect((await guest.get(link.replace(/\?x=\d+/, "?x=1"))).status).toBe(403);
    // The file's own address, followed as a link, still opens (a link in an email, say).
    expect((await guest.get(bare, { headers: nav })).status).toBe(200);
  });

  it("another site's page cannot use it, signature or not; a signed-in member needs none", async () => {
    expect((await guest.get(link, { headers: fromSite })).status).toBe(403);
    expect((await guest.get(link.split("?")[0], { headers: { referer: "https://other.example/blog" } })).status).toBe(403);
    expect((await bob.get(link.split("?")[0])).status).toBe(200);
    expect((await guest.get(link, { headers: { "sec-fetch-site": "same-origin", "sec-fetch-dest": "empty" } })).status).toBe(200);
  });

  it("a page revisited in a later window is sent again, not 'not modified'", async () => {
    const t = await topic(bob, s.FORUM, "Validators", "Nothing to sign here, but the page has a validator");
    const first = await guest.get(t.url);
    const tag = first.headers.get("etag");
    if (tag) {
      expect(tag).toMatch(/-w\d+"$/);
      expect((await guest.get(t.url, { headers: { "if-none-match": tag } })).status).toBe(304);
      expect((await guest.get(t.url, { headers: { "if-none-match": tag.replace(/-w\d+"$/, '-w1"') } })).status).toBe(200);
    }
  });

  it("pictures stay plain unless the site says all", async () => {
    const png = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==", "base64");
    const r = await bob.multipart("/api/v1/uploads", { file: { name: "dot.png", data: png } });
    const up = JSON.parse(r.text) as { url: string; markdown: string };
    const t = await topic(bob, s.FORUM, "A picture", `Look: ${up.markdown}`);
    expect(linkIn((await guest.get(t.url)).text, "png")).not.toContain("?x=");
    expect((await guest.get(up.url.split("#")[0], { headers: fromSite })).status).toBe(200);
    expect((await setting(alice, "hotlink_protection", "all")).status).toBe(204);
    const signed = linkIn((await guest.get(t.url)).text, "png");
    expect(signed).toContain("?x=");
    expect((await guest.get(signed.split("#")[0])).status).toBe(200);
    expect((await guest.get(up.url.split("#")[0])).status).toBe(403);
    expect((await guest.get(signed.split("#")[0], { headers: fromSite })).status).toBe(403);
    expect((await setting(alice, "hotlink_protection", "off")).status).toBe(204);
    expect(linkIn((await guest.get(t.url)).text, "png")).not.toContain("?x=");
    expect((await guest.get(up.url.split("#")[0])).status).toBe(200);
    expect((await setting(alice, "hotlink_protection", "files")).status).toBe(204);
  });
});

describe("files of a draft thrown away", () => {
  it("are deleted when nothing else uses them, kept when something does", async () => {
    const lost = await upload(bob, file("lost.txt", "uploaded for a draft that was thrown away"));
    const kept = await upload(bob, file("kept.txt", "uploaded for a draft, and posted elsewhere"));
    await topic(bob, s.FORUM, "Posts the kept file", `Kept: ${kept.markdown}`);
    const ctx = `new-topic:${s.FORUM}`;
    expect((await bob.api(`/api/v1/drafts?context=${ctx}`, { body: `${lost.markdown}\n${kept.markdown}` }, { method: "PUT" })).status).toBe(204);
    expect((await guest.get(lost.url, { headers: nav })).status).toBe(200);
    // Posting clears the draft without purging: nothing is deleted behind a post being saved.
    expect((await bob.api(`/api/v1/drafts?context=${ctx}`, undefined, { method: "DELETE" })).status).toBe(204);
    expect((await guest.get(lost.url, { headers: nav })).status).toBe(200);
    expect((await bob.api(`/api/v1/drafts?context=${ctx}`, { body: `${lost.markdown}\n${kept.markdown}` }, { method: "PUT" })).status).toBe(204);
    expect((await bob.api(`/api/v1/drafts?context=${ctx}&purge=1`, undefined, { method: "DELETE" })).status).toBe(204);
    expect((await guest.get(lost.url, { headers: nav })).status).toBe(404);
    expect((await guest.get(kept.url, { headers: nav })).status).toBe(200);
  });

  it("not another member's file", async () => {
    const theirs = await upload(bob, file("theirs.txt", "bob's file that carol cannot delete"));
    const ctx = `new-topic:${s.FORUM}`;
    expect((await carol.api(`/api/v1/drafts?context=${ctx}`, { body: theirs.markdown }, { method: "PUT" })).status).toBe(204);
    expect((await carol.api(`/api/v1/drafts?context=${ctx}&purge=1`, undefined, { method: "DELETE" })).status).toBe(204);
    expect((await guest.get(theirs.url, { headers: nav })).status).toBe(200);
  });
});
