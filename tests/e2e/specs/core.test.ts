// Registration, topics, moderation, profiles, uploads, drafts.
import { describe, expect, it } from "vitest";
import { Client, idFrom, useForum } from "../lib/forum.ts";
import { mailLink } from "../lib/seed.ts";

const forum = useForum();
let alice: Client, bob: Client, guest: Client;
let FORUM: number;

const PNG = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==", "base64");
const dot = { name: "dot.png", data: PNG };

describe("registration and first topic", () => {
  it("healthz", async () => {
    guest = forum.client();
    expect((await guest.get("/healthz")).status).toBe(200);
  });
  it("register admin", async () => {
    alice = forum.client();
    expect((await alice.register("alice", "alice@example.com", "correct horse battery")).status).toBe(303);
    expect((await alice.get("/")).text).toContain("alice");
  });
  it("create category and forum", async () => {
    expect((await alice.api("/api/v1/admin/nodes", { kind: "category", title: "General" })).status).toBe(201);
    const r = await alice.api("/api/v1/admin/nodes", { kind: "forum", title: "Introductions", parent_id: 1, description: "Say **hi**" });
    FORUM = Number(r.text.match(/\d+/)![0]);
    expect(FORUM).toBe(2);
  });
  it("register bob", async () => {
    bob = forum.client();
    expect((await bob.register("bob")).status).toBe(303);
  });
  it("new topic redirects to topic", async () => {
    const r = await alice.form(`/c/${FORUM}/new`, { title: "Hello from Alice", body: "First post, hi @bob! **bold** <script>alert(1)</script>" });
    expect(r.location).toContain("/t/hello-from-alice/1");
  });
  it("guest sees a safe topic with SEO data", async () => {
    const t = (await guest.get("/t/hello-from-alice/1")).text;
    expect(t).toContain("Hello from Alice");
    expect(t).toContain("<strong>bold</strong>");
    expect(t).not.toContain("<script>alert");
    expect(t).toContain("DiscussionForumPosting");
    expect(t).toContain(`rel="canonical" href="${forum.base}/t/hello-from-alice/1"`);
  });
  it("wrong slug redirects", async () => {
    expect((await guest.get("/t/wrong/1")).status).toBe(308);
  });
  it("bob cannot create forums", async () => {
    expect((await bob.api("/api/v1/admin/nodes", { kind: "forum", title: "x" })).status).toBe(403);
  });
  it("bob was mentioned", async () => {
    expect((await bob.get("/notifications")).text).toContain("alice mentioned you in Hello from Alice");
  });
  it("replies need csrf and notify", async () => {
    expect((await bob.form("/t/1/reply", { body: "Hi Alice, welcome aboard!" })).status).toBe(303);
    expect((await bob.form("/t/1/reply", { csrf: "", body: "Hi Alice, welcome aboard!" })).status).toBe(400);
    expect((await alice.get("/notifications")).text).toContain("bob posted in Hello from Alice");
  });
  it("only staff lock and pin", async () => {
    expect((await bob.form("/t/1/mod", { action: "pin" })).status).toBe(403);
    expect((await alice.form("/t/1/mod", { action: "lock" })).status).toBe(303);
    expect((await bob.form("/t/1/reply", { body: "Another reply here" })).status).toBe(403);
    expect((await alice.get("/t/hello-from-alice/1")).text).toContain("locked this topic");
  });
  it("topic is listed everywhere", async () => {
    expect((await guest.get("/c/introductions/2")).text).toContain("Hello from Alice");
    expect((await guest.get("/")).text).toContain("Introductions");
    expect((await alice.get("/search?q=welcome")).text).toContain("Hello from Alice");
    expect((await guest.get("/sitemap.xml")).text).toContain("/t/hello-from-alice/1");
    expect((await guest.get("/latest.rss")).text).toContain("<title>Hello from Alice</title>");
  });
  it("preview cooks", async () => {
    expect((await alice.api("/api/v1/preview", { raw: "*hi*" })).text).toContain("<em>hi</em>");
  });
  it("profiles and redirects", async () => {
    expect((await guest.get("/u/BOB")).status).toBe(200);
    expect((await guest.get("/u/alice")).text).toContain("Hello from Alice");
    expect((await guest.get("/profile/999-x/")).status).toBe(404);
  });
  it("list filters", async () => {
    expect((await guest.get("/c/introductions/2?show=unanswered")).text).not.toContain("Hello from Alice");
    expect((await guest.get("/c/introductions/2?sort=views")).text).toContain('name="robots" content="noindex"');
    expect((await alice.get("/c/introductions/2?show=mine")).text).toContain("Hello from Alice");
  });
});

describe("watched words and review", () => {
  it("word rules", async () => {
    expect((await alice.api("/api/v1/admin/watched-words", { action: "block", pattern: "badword" })).status).toBe(201);
    expect((await alice.api("/api/v1/admin/watched-words", { action: "require_approval", pattern: "casino" })).status).toBe(201);
    expect((await bob.api("/api/v1/admin/watched-words", { action: "block", pattern: "x" })).status).toBe(403);
    expect((await bob.form(`/c/${FORUM}/new`, { title: "A badword title here", body: "Some long enough body text" })).status).toBe(422);
    expect((await bob.form(`/c/${FORUM}/new`, { title: "Best casino in town", body: "Come visit our lovely place" })).status).toBe(202);
    expect((await guest.get("/c/introductions/2")).text).not.toContain("Best casino");
  });
  it("approval queue", async () => {
    expect((await bob.get("/review")).status).toBe(403);
    const review = (await alice.get("/review")).text;
    expect(review).toContain("Awaiting approval");
    const rid = idFrom(review, /action="\/review\/(\d+)"/);
    expect((await alice.form(`/review/${rid}`, { decision: "keep" })).status).toBe(303);
    expect((await guest.get("/c/introductions/2?sort=newest")).text).toContain("Best casino in town");
  });
});

describe("unread", () => {
  it("reading and marking clear unread", async () => {
    expect((await alice.get("/unread")).text).toContain("Best casino in town");
    const casino = (await guest.get("/latest")).text.match(/\/t\/best-casino-in-town\/\d+/)![0];
    await alice.get(casino);
    expect((await alice.get("/unread")).text).not.toContain("Best casino");
    const cid = casino.match(/\d+/)![0];
    expect((await bob.form(`/t/${cid}/reply`, { body: "Bumping my own topic now" })).status).toBe(303);
    expect((await alice.get("/latest")).text).toContain("1 new");
    expect((await alice.form("/read-all")).status).toBe(303);
    expect((await alice.get("/unread")).text).not.toContain("Best casino");
  });
});

describe("topic moderation", () => {
  let OFF: string, SRC: string, NEW: string, P1: string, DEL: string, replies: string[], srcUrl: string;
  it("set up a topic with replies", async () => {
    OFF = (await alice.api("/api/v1/admin/nodes", { kind: "forum", title: "Offtopic", parent_id: 1 })).text.match(/\d+/)![0];
    const r = await alice.form(`/c/${FORUM}/new`, { title: "Split source topic", body: "The original first post text" });
    SRC = idFrom(r.location, /\/t\/[^/]+\/(\d+)/);
    srcUrl = `/t/split-source-topic/${SRC}`;
    await bob.form(`/t/${SRC}/reply`, { body: "First reply that will move" });
    await bob.form(`/t/${SRC}/reply`, { body: "Second reply that will move" });
  });
  it("IPs are for staff", async () => {
    // Posts do not print addresses; staff look them up in the moderator panel.
    expect((await alice.get(srcUrl)).text).not.toContain("· 127.0.0.1");
    expect((await alice.get("/modcp?tab=tools&ip=127.0.0.1")).text).toContain("bob");
    expect((await bob.get("/modcp?tab=tools&ip=127.0.0.1")).status).toBeGreaterThanOrEqual(400);
    expect((await bob.get(srcUrl)).text).not.toContain("· 127.0.0.1");
  });
  it("edit history", async () => {
    const page = (await alice.get(srcUrl)).text;
    P1 = idFrom(page, /data-edit-post="(\d+)"/);
    replies = [...page.matchAll(/name="post" value="(\d+)"/g)].map((m) => m[1]);
    expect((await alice.get(`/post/${P1}/edit`)).status).toBe(405); // no page: editing is in the post
    const source = JSON.parse((await alice.get(`/api/v1/posts/${P1}/source`)).text);
    expect(source.base).toBe(0);
    expect(source.raw.length).toBeGreaterThan(0);
    expect((await forum.client().get(`/api/v1/posts/${P1}/source`)).status).toBe(403);
    // The edit form must say which version it started from (concurrent edits are compared under a row lock).
    expect((await alice.form(`/post/${P1}/edit`, { body: "The edited first post text", reason: "typo fix" })).status).toBe(400);
    expect((await alice.form(`/post/${P1}/edit`, { body: "The edited first post text", reason: "typo fix", base: source.base })).status).toBe(303);
    const hist = (await alice.get(`/post/${P1}/history`)).text;
    expect(hist).toContain("The original first post text");
    expect(hist).toContain("typo fix");
  });
  it("split", async () => {
    expect((await bob.form(`/t/${SRC}/split`, { title: "Stolen split topic", post: replies })).status).toBe(403);
    const r = await alice.form(`/t/${SRC}/split`, { title: "Split result topic", post: replies });
    expect(r.location).toContain("split-result-topic");
    NEW = idFrom(r.location, /\/t\/[^/]+\/(\d+)/);
    expect((await guest.get(`/t/split-result-topic/${NEW}`)).text).toContain("Second reply that will move");
    expect((await alice.get(srcUrl)).text).not.toContain("First reply that will move");
  });
  it("move and merge", async () => {
    expect((await alice.form(`/t/${NEW}/move`, { to: OFF })).status).toBe(303);
    expect((await alice.get(`/c/offtopic/${OFF}`)).text).toContain("Split result topic");
    expect((await alice.form(`/t/${NEW}/merge`, { into: SRC })).status).toBe(303);
    expect((await guest.get(`/t/split-result-topic/${NEW}`)).location).toBe(forum.base + srcUrl);
    expect((await alice.get(srcUrl)).text).toContain("Second reply that will move");
  });
  it("delete and restore", async () => {
    const all = [...(await alice.get(srcUrl)).text.matchAll(/action="\/post\/(\d+)\/delete"/g)];
    DEL = all.at(-1)![1];
    const mark = `action="/post/${DEL}/restore"`;
    expect((await alice.form(`/post/${DEL}/delete`)).status).toBe(303);
    expect((await alice.get("/review/deleted")).text).toContain(mark);
    // 404, not 403: a member who may not moderate must not learn that a deleted post exists at
    // this id, or the status code becomes a way to enumerate deletions.
    expect((await bob.form(`/post/${DEL}/restore`)).status).toBe(404);
    expect((await alice.form(`/post/${DEL}/restore`)).status).toBe(303);
    expect((await alice.get("/review/deleted")).text).not.toContain(mark);
  });

  describe("profiles, ignore, follows", () => {
    let WT: string;
    it("avatars", async () => {
      expect((await guest.get("/avatar/alice.svg")).text).toContain(">A</text>");
      expect((await guest.get("/avatar/alice.png")).status).toBe(404);
    });
    it("profile fields", async () => {
      expect((await bob.form("/settings/profile", { website: "javascript:alert(1)" })).status).toBe(422);
      expect((await bob.form("/settings/profile", { location: "Kraków", website: "https://bob.example", about: "I like **forums**" })).status).toBe(303);
      const p = (await guest.get("/u/bob")).text;
      expect(p).toContain("Kraków");
      expect(p).toContain('href="https://bob.example" rel="nofollow ugc noopener"');
      expect(p).toContain("<strong>forums</strong>");
    });
    it("ignore", async () => {
      expect((await bob.form("/u/1/ignore", { on: 1 })).status).toBe(303);
      expect((await bob.get(srcUrl)).text).toContain("Post by alice, who you ignore");
      expect((await bob.form("/u/1/ignore", { on: 0 })).status).toBe(303);
      expect((await bob.form("/u/2/ignore", { on: 1 })).status).toBe(422);
    });
    it("watching a forum notifies in app and by mail", async () => {
      // Mail goes only to an address the member has confirmed.
      const verify = await mailLink(forum, "bob@example.com", "/verify/");
      expect((await bob.form(verify)).text).toContain("Email verified");
      expect((await bob.form(`/c/${FORUM}/follow`, { level: "watch" })).status).toBe(303);
      expect((await bob.get(`/c/introductions/${FORUM}`)).text).toContain('value="watch" aria-current="true"');
      expect((await bob.form("/settings/notifications", { reply_app: "on", reply_email: "on", mention_app: "on" })).status).toBe(303);
      const r = await alice.form(`/c/${FORUM}/new`, { title: "Watched forum topic", body: "Hello watchers of this forum" });
      WT = idFrom(r.location, /\/t\/[^/]+\/(\d+)/);
      expect((await bob.get("/notifications")).text).toContain("alice posted in Watched forum topic");
      const mail = await forum.mail((m) => m.includes("To: bob@example.com") && m.includes("Watched forum topic"));
      expect(mail).toContain('alice posted in "Watched forum topic"');
    });
    it("muting and auto-watch", async () => {
      expect((await bob.form(`/t/${WT}/follow`, { level: "mute" })).status).toBe(303);
      await alice.form(`/t/${WT}/reply`, { body: "A reply bob should not hear about" });
      const n = (await bob.get("/notifications")).text.split("\n").filter((l) => l.includes("alice posted in Watched forum topic")).length;
      expect(n).toBe(1);
      expect((await alice.get(`/t/watched-forum-topic/${WT}`)).text).toContain('value="watch" aria-current="true"');
    });

    describe("uploads, avatars, drafts", () => {
      let IMG: string;
      const svg = { name: "x.svg", data: '<svg xmlns="http://www.w3.org/2000/svg" onload="alert(1)"/>' };
      const fake = { name: "fake.png", data: "<html><script>alert(1)</script></html>" };
      const notes = { name: "notes.txt", data: "plain notes\n" };
      it("image upload", async () => {
        const u = (await bob.multipart("/api/v1/uploads", { file: dot })).text;
        expect(u).toContain('"is_image":true');
        expect(u).toContain('"width":1');
        IMG = u.match(/\/uploads\/[0-9a-f]{64}\.png/)![0];
        expect(u).toContain(`![dot.png](${IMG}#`);
        const u2 = (await bob.multipart("/api/v1/uploads", { file: dot })).text;
        expect(u2.match(/\/uploads\/[0-9a-f]{64}\.png/)![0]).toBe(IMG);
      });
      it("safe serving headers", async () => {
        const h = (await guest.get(IMG)).headers;
        expect(h.get("content-type")).toBe("image/png");
        expect(h.get("x-content-type-options")).toBe("nosniff");
        expect(h.get("content-security-policy")).toContain("sandbox");
      });
      it("refuses bad uploads", async () => {
        expect((await bob.multipart("/api/v1/uploads", { file: svg })).status).toBe(422);
        expect((await bob.multipart("/api/v1/uploads", { file: fake })).status).toBe(422);
        expect((await bob.multipart("/api/v1/uploads", { file: dot }, { noCsrf: true })).status).toBe(400);
        expect((await guest.multipart("/api/v1/uploads", { file: dot }, { headers: { "x-csrf-token": bob.csrf } })).status).toBe(403);
      });
      it("files are attachments; unknown paths 404", async () => {
        const txt = (await bob.multipart("/api/v1/uploads", { file: notes })).text.match(/\/uploads\/[0-9a-f]{64}\.txt/)![0];
        expect((await guest.get(txt, { headers: { "sec-fetch-mode": "navigate", "sec-fetch-dest": "document" } })).headers.get("content-disposition")).toContain("attachment; filename*=UTF-8''notes.txt");
        expect((await guest.get(`/uploads/${"b".repeat(64)}.png`)).status).toBe(404);
        expect((await guest.get("/uploads/..%2f..%2fCargo.toml")).status).toBe(404);
      });
      it("avatar upload", async () => {
        expect((await bob.multipart("/settings/avatar", { csrf: bob.csrf, file: dot }, { noCsrf: true })).status).toBe(303);
        // A still picture is sent at the avatar size, from the same upload.
        const small = IMG.replace(/\.png$/, "-128.png");
        expect((await guest.get("/avatar/bob.svg")).location).toBe(forum.base + small);
        const served = await guest.get(small);
        expect(served.status).toBe(200);
        expect(served.headers.get("content-type")).toBe("image/png");
        expect((await bob.multipart("/settings/avatar", { csrf: bob.csrf, file: notes }, { noCsrf: true })).status).toBe(422);
        expect((await bob.form("/settings/avatar/remove")).status).toBe(303);
        expect((await guest.get("/avatar/bob.svg")).text).toContain(">B</text>");
      });
      it("drafts", async () => {
        const url = `/api/v1/drafts?context=reply:${WT}`;
        expect((await bob.api(url, { body: "half-written reply" }, { method: "PUT" })).status).toBe(204);
        expect((await bob.get(url)).text).toContain("half-written reply");
        expect((await alice.get(url)).status).toBe(204);
        expect((await bob.get("/api/v1/drafts?context=../x")).status).toBe(400);
      });
    });
  });
});

describe("composer and header endpoints", () => {
  it("creates a topic the way a browser submits it, with an empty schedule field", async () => {
    const r = await alice.form(`/c/${FORUM}/new`, { title: "Sent from a browser form", body: "A first post long enough to pass", tags: "sheet-tag, second", publish_minutes: "" });
    expect(r.status).toBe(303);
    expect((await alice.get(new URL(r.location).pathname)).text).toContain("sheet-tag");
  });
  it("has no new-topic page: the form is on the forum page", async () => {
    const r = await alice.get(`/c/${FORUM}/new`);
    expect(r.status).toBe(303);
    expect(r.location).toContain("#new-topic");
    const page = (await alice.get(new URL(r.location).pathname)).text;
    expect(page).toContain('id="new-topic"');
    expect(page).toContain("<optgroup");
    expect((await guest.get(new URL(r.location).pathname)).text).not.toContain('id="new-topic"');
  });
  it("header links have addresses, and admins get the admin panel", async () => {
    const page = (await alice.get("/")).text;
    expect(page).toContain('<a href="/latest">');
    expect(page).not.toContain('<a href="">');
    expect(page).toContain('<a href="/admin/">');
    expect((await bob.get("/")).text).not.toContain('<a href="/admin/">');
  });
  it("status updates: post, reply, notify, lock, delete", async () => {
    const aliceId = 1;
    expect((await bob.form(`/u/${aliceId}/status`, { body: "Not my profile" })).status).toBe(403);
    const posted = await alice.form(`/u/${aliceId}/status`, { body: "Status with <b>markup</b>" });
    expect(posted.status).toBe(303);
    const id = idFrom(posted.location, /#status-(\d+)/);
    const feed = (await guest.get("/u/alice?tab=status")).text;
    expect(feed).toContain("Status with &#60;b&#62;markup&#60;/b&#62;");
    expect((await bob.form(`/status/${id}/reply`, { body: "A reply from bob" })).status).toBe(303);
    expect((await alice.get("/notifications")).text).toContain("bob replied to your status");
    expect((await bob.form(`/status/${id}/lock`)).status).toBe(403);
    expect((await alice.form(`/status/${id}/lock`)).status).toBe(303);
    expect((await bob.form(`/status/${id}/reply`, { body: "Too late to reply" })).status).toBe(422);
    expect((await bob.form(`/status/${id}/delete`)).status).toBe(403);
    expect((await alice.form(`/status/${id}/delete`)).status).toBe(303);
    expect((await guest.get("/u/alice?tab=status")).text).not.toContain("markup");
  });
  it("counts profile views from others, and keeps the visitor list private", async () => {
    const views = (html: string) => Number(html.match(/Profile views<\/span><b>(\d+)</)![1]);
    const url = "/u/alice";
    const before = views((await alice.get(url)).text);
    await bob.get(url);
    await bob.get(`${url}?tab=topics`);
    await bob.get(url, { headers: { "sec-purpose": "prefetch;prerender" } });
    const page = (await alice.get(url)).text;
    expect(views(page)).toBe(before + 1);
    expect(page).toContain("Recent profile visitors");
    expect((await bob.get(url)).text).not.toContain("Recent profile visitors");
  });
  it("warning box for the member and staff only", async () => {
    const url = "/u/bob";
    expect((await alice.form("/u/2/warn", { points: "2", reason: "Box test reason" })).status).toBe(303);
    const mine = (await bob.get(url)).text;
    expect(mine).toContain("warning points");
    expect(mine).toContain("Box test reason");
    expect((await alice.get(url)).text).toContain("Box test reason");
    expect((await guest.get(url)).text).not.toContain("Box test reason");
  });
  it("followers box, follow button state and hiding", async () => {
    const url = "/u/alice";
    const before = (await bob.get(url)).text;
    const wasFollowing = before.includes("Unfollow");
    if (wasFollowing) await bob.form("/u/1/follow");
    expect((await bob.form("/u/1/follow")).location).toContain(url);
    const page = (await bob.get(url)).text;
    expect(page).toContain("Unfollow");
    expect(page).toContain('href="/u/bob"');
    expect((await bob.get(`${url}?tab=followers`)).text).toContain("bf-tab-title");
    expect((await alice.form("/settings/hide-followers")).status).toBe(303);
    expect((await guest.get(url)).text).not.toContain("bf-followers");
    expect((await alice.get(url)).text).toContain("Only you and staff can see this.");
    await alice.form("/settings/hide-followers");
  });
  it("removed pages are gone, settings keeps the rest", async () => {
    expect((await alice.get("/streams")).status).toBe(404);
    expect((await alice.get("/settings/visitors")).status).toBe(405);
    const page = (await alice.get("/settings")).text;
    expect(page).not.toContain("My sidebar links");
    expect(page).not.toContain("/streams");
    expect(page).toContain('name="skin_tone" value="3"');
    expect(page).toContain("bf-sessions");
  });
  it("serves flags by code only", async () => {
    const ok = await guest.get("/assets/flags/pl.svg");
    expect(ok.status).toBe(200);
    expect(ok.text).toContain("<svg");
    expect((await guest.get("/assets/flags/nope.svg")).status).toBe(404);
  });
  it("profile visitor setting toggles", async () => {
    expect((await bob.get("/settings")).text).toContain("Off: no list");
    expect((await bob.form("/settings/visitors", { on: "1" })).status).toBe(303);
    expect((await bob.get("/settings")).text).toContain("Turn off and delete the list");
    expect((await bob.form("/settings/visitors")).status).toBe(303);
    expect((await bob.get("/settings")).text).toContain("Off: no list");
  });
  it("cover can be positioned and removed, within limits", async () => {
    expect((await alice.form("/settings/cover/position", { y: "101" })).status).toBe(422);
    expect((await alice.form("/settings/cover/position", { y: "20" })).status).toBe(204);
    expect((await guest.form("/settings/cover/position", { y: "20" })).status).toBeGreaterThanOrEqual(400);
    expect((await alice.form("/settings/cover/remove")).status).toBe(303);
  });
  it("search page: marks matches, shows names, escapes, and filters by end date", async () => {
    const page = (await alice.get("/search?q=welcome")).text;
    expect(page.toLowerCase()).toContain("<mark>welcome</mark>");
    expect(page).not.toContain("StopSel");
    expect(page).not.toContain("&#60;/b&#62;");
    const names = (await alice.get("/search?q=alice")).text;
    expect(names).toContain('class="bf-browsing-chip" href="/u/alice"');
    expect((await alice.get("/search?q=alice&author=alice")).text).not.toContain('class="bf-browsing-chip" href="/u/alice"');
    expect((await alice.get("/search?q=%3Cscript%3Ealert")).text).not.toContain("<script>alert");
    expect((await alice.get("/search?q=welcome&before=2000-01-01")).text).toContain("No results");
    expect((await guest.get("/search")).text).toContain("Search the forum");
  });
  it("search results say what they are", async () => {
    const rows = JSON.parse((await alice.get("/api/v1/search?q=ali")).text);
    expect(rows.find((r: { kind: string; title: string }) => r.kind === "member" && r.title === "alice")).toBeTruthy();
  });
  it("search pages keep their filters", async () => {
    for (let i = 0; i < 22; i++) await alice.form(`/c/${FORUM}/new`, { title: `Pagination number ${i} zebrafinch`, body: "Enough words about a zebrafinch to pass" });
    const one = (await alice.get("/search?q=zebrafinch&author=alice")).text;
    expect(one).toContain("results for");
    expect(one).toContain('rel="next" href="/search?q=zebrafinch&#38;author=alice&#38;page=2"');
    expect(one.split("bf-feed-title").length - 1).toBe(20);
    const two = (await alice.get("/search?q=zebrafinch&author=alice&page=2")).text;
    expect(two.split("bf-feed-title").length - 1).toBeGreaterThan(0);
    expect(two).toContain('rel="prev"');
  });
  it("link previews only for addresses in posts", async () => {
    expect((await guest.get("/api/v1/onebox?url=https://example.invalid/never-posted")).status).toBe(404);
    expect((await alice.form(`/c/${FORUM}/new`, { title: "Post with a dead link", body: "https://localhost.invalid/x" })).status).toBe(303);
    expect((await guest.get("/api/v1/onebox?url=https://localhost.invalid/x")).status).toBe(404);
  });
  it("reactions live in the post actions", async () => {
    const r = await alice.form(`/c/${FORUM}/new`, { title: "Reaction topic", body: "A first post long enough to pass" });
    const url = new URL(r.location).pathname;
    const id = idFrom((await bob.get(url)).text, /action="\/post\/(\d+)\/react"/);
    expect((await bob.form(`/post/${id}/react`, { reaction: "2" })).status).toBe(303);
    const page = (await bob.get(url)).text;
    expect(page).toContain("bf-react-pill is-mine");
    expect(page).toContain("🙏 1");
    expect((await guest.get(url)).text).toContain("bf-react-pill");
  });
  it("message lists show text, not Markdown", async () => {
    const r = await alice.form("/messages", { to: "bob", subject: "Markdown preview", body: "**strong words** here" });
    expect(r.status).toBe(303);
    const list = (await bob.get("/messages")).text;
    expect(list).toContain("strong words here");
    expect(list).not.toContain("**strong words**");
    expect((await bob.get("/api/v1/me/messages")).text).not.toContain("**");
  });
  it("topic author badge and richer summary", async () => {
    const r = await alice.form(`/c/${FORUM}/new`, { title: "Summary topic for stats", body: "A first post long enough to pass" });
    const url = new URL(r.location).pathname;
    const id = idFrom(r.location, /\/t\/[^/]+\/(\d+)/);
    await alice.form(`/t/${id}/reply`, { body: "The author replies to their own topic" });
    await bob.form(`/t/${id}/reply`, { body: "Somebody else replies to this topic" });
    const page = (await guest.get(url)).text;
    expect(page.split("bf-op-pill").length - 1).toBe(1);
    const sum = (await guest.get(`/t/${id}/summary`)).text;
    expect(sum).toContain("Back to the full topic");
    expect(sum).toContain("Participants");
    expect(sum).toContain("Most active");
  });
  it("admin members: filters, paging, trust level, sign out", async () => {
    const staff = JSON.parse((await alice.get("/api/v1/admin/users?status=staff")).text);
    expect(staff.map((u: { username: string }) => u.username)).toContain("alice");
    expect(staff.map((u: { username: string }) => u.username)).not.toContain("bob");
    const one = JSON.parse((await alice.get("/api/v1/admin/users?id=2")).text);
    expect(one).toHaveLength(1);
    expect(one[0].total).toBe(1);
    const patch = (b: unknown) => alice.api("/api/v1/admin/users/2", b, { method: "PATCH" });
    expect((await patch({ trust_level: 9 })).status).toBe(422);
    expect((await patch({ trust_level: 3 })).status).toBeLessThan(300);
    expect(JSON.parse((await alice.get("/api/v1/admin/users?id=2")).text)[0].trust_level).toBe(3);
    expect((await bob.api("/api/v1/admin/users/1/logout")).status).toBeGreaterThanOrEqual(400);
    expect((await alice.api("/api/v1/admin/users/1/logout")).status).toBe(422);
  });
  it("custom pages: drafts, visibility, safe content, unique address", async () => {
    const make = (b: unknown) => alice.api("/api/v1/admin/pages", b);
    expect((await bob.api("/api/v1/admin/pages", { title: "No", visibility: "public" })).status).toBeGreaterThanOrEqual(400);
    const r = await make({ title: "About Us", body: "**Hello** <script>alert(1)</script>", visibility: "public", published: false });
    expect(r.status).toBe(201);
    const { id, slug } = JSON.parse(r.text);
    expect(slug).toBe("about-us");
    expect((await guest.get("/page/about-us")).status).toBe(404);
    expect((await alice.get("/page/about-us")).status).toBe(200);
    expect((await make({ title: "About Us", visibility: "public" })).status).toBe(422);
    const put = (b: unknown) => alice.api(`/api/v1/admin/pages/${id}`, b, { method: "PUT" });
    expect((await put({ title: "About Us", body: "**Hello** <script>alert(1)</script>", visibility: "members", published: true })).status).toBe(204);
    expect((await guest.get("/page/about-us")).status).toBe(403);
    // A page is its blocks: the content is a custom widget in the page's own area.
    const w = await alice.api("/api/v1/admin/widgets", { kind: "custom", area: `page${id}_top`, title: "About text", config: { markdown: "**Hello** <script>alert(1)</script>" } });
    expect(w.status).toBe(201);
    const page = (await bob.get("/page/about-us")).text;
    expect(page).toContain("<strong>Hello</strong>");
    expect(page).not.toContain("<script>alert");
    expect((await alice.api(`/api/v1/admin/pages/${id}`, undefined, { method: "DELETE" })).status).toBe(204);
  });
  it("forum icons are checked, and widgets show up at once in every area", async () => {
    const icon = (b: unknown) => alice.api(`/api/v1/admin/nodes/${FORUM}/icon`, b, { method: "PUT" });
    expect((await icon({ icon: "<script>" })).status).toBe(422);
    expect((await icon({ icon: "", image: "https://evil.test/x.png" })).status).toBe(422);
    expect((await icon({ icon: "rocket" })).status).toBe(204);
    expect((await alice.get("/")).text).toContain("#rocket");
    expect((await icon({ icon: "🎮" })).status).toBe(204);
    expect((await guest.get("/")).status).toBe(200);
    await icon({ icon: "" });
    const w = await alice.api("/api/v1/admin/widgets", { kind: "custom", area: "home_main", title: "Fresh block", config: { markdown: "hi there" } });
    expect(w.status).toBe(201);
    const page = (await alice.get("/")).text;
    expect(page).toContain("Fresh block");
    expect(page).toContain('data-area-label="Main column"');
    const wid = JSON.parse(w.text).id;
    expect((await alice.api(`/api/v1/admin/widgets/${wid}`, undefined, { method: "DELETE" })).status).toBe(204);
    expect((await alice.get("/")).text).not.toContain("Fresh block");
  });
  it("every home block can be moved, removed and added back", async () => {
    const list = async () => JSON.parse((await alice.get("/api/v1/admin/widgets")).text).widgets as { id: number; kind: string; area: string }[];
    const forums = (await list()).find((w) => w.kind === "forum_list")!;
    expect((await guest.get("/")).text).toContain('class="forums"');
    expect((await alice.api("/api/v1/admin/widgets", { kind: "forum_list", area: "home_top" })).status).toBe(422);
    expect((await alice.api("/api/v1/admin/widgets/arrange", { area: "home_bottom", ids: [forums.id] }, { method: "PUT" })).status).toBe(204);
    expect((await list()).find((w) => w.id === forums.id)!.area).toBe("home_bottom");
    expect((await alice.api(`/api/v1/admin/widgets/${forums.id}`, undefined, { method: "DELETE" })).status).toBe(204);
    expect((await alice.get("/")).text).not.toContain('class="forums"');
    expect((await alice.api("/api/v1/admin/widgets", { kind: "forum_list", area: "home_main" })).status).toBe(201);
    expect((await alice.get("/")).text).toContain('class="forums"');
  });
  it("latest status updates block, escaped and honouring profile privacy", async () => {
    await alice.form("/u/1/status", { body: "Block <b>status</b> text" });
    const w = await alice.api("/api/v1/admin/widgets", { kind: "latest_statuses", area: "home_top", title: "Status news", cache_seconds: 0 });
    expect(w.status).toBe(201);
    const page = (await guest.get("/?x=1")).text;
    expect(page).toContain("Status news");
    expect(page).toContain("Block &lt;b&gt;status&lt;/b&gt; text");
    await alice.api(`/api/v1/admin/widgets/${JSON.parse(w.text).id}`, undefined, { method: "DELETE" });
  });
  it("online dots honour a hidden last-seen", async () => {
    await alice.get("/");
    await forum.sql("UPDATE users SET last_seen_at = now(), hide_last_seen = false WHERE username = 'alice'");
    const ask = async () => JSON.parse((await guest.get("/api/v1/online?names=alice,nobody")).text);
    expect(await ask()).toEqual(["alice"]);
    await forum.sql("UPDATE users SET hide_last_seen = true WHERE username = 'alice'");
    expect(await ask()).toEqual([]);
    await forum.sql("UPDATE users SET hide_last_seen = false WHERE username = 'alice'");
  });
  it("shows the review badge on every page, to moderators only", async () => {
    const badge = /href="\/review">[^<]*<span class="badge">\d+<\/span>/;
    await forum.sql("INSERT INTO reviewables (kind, target_type, target_id, node_id) SELECT 'flag', 'post', 0, min(id) FROM nodes WHERE kind = 'forum'");
    for (const path of ["/", "/latest", "/settings", "/messages", "/search?q=x", "/modcp"]) {
      expect((await alice.get(path)).text, path).toMatch(badge);
    }
    expect((await bob.get("/settings")).text).not.toMatch(badge);
    await forum.sql("DELETE FROM reviewables WHERE target_id = 0 AND target_type = 'post'");
  });
  it("uses Discourse-style addresses, and short forms lead to the full one", async () => {
    const to = async (path: string) => {
      const r = await guest.get(path);
      expect(r.status, path).toBe(308);
      return r.location.replace(forum.base, "");
    };
    expect(await to("/t/hello-from-alice")).toBe("/t/hello-from-alice/1");
    expect(await to("/t/1")).toBe("/t/1/1");
    expect(await to("/c/introductions")).toBe("/c/introductions/2");
    expect((await guest.get("/t/nothing-called-this")).status).toBe(404);
    expect((await guest.get("/t/x/not-a-number")).status).toBe(404);
    expect((await guest.get("/c/x/not-a-number")).status).toBe(404);
  });
  it("lists members, groups, badges, the leaderboard and the about page", async () => {
    const users = (await guest.get("/u")).text;
    expect(users).toContain('href="/u/alice"');
    expect(users).toContain('href="/u/bob"');
    const found = (await guest.get("/u?q=ali")).text;
    expect(found).toContain('href="/u/alice"');
    expect(found).not.toContain('href="/u/bob"');
    expect((await guest.get("/u?q=%25")).text).not.toContain('href="/u/alice"');
    expect((await guest.get("/u?sort=reputation")).status).toBe(200);
    expect((await guest.get("/leaderboard")).status).toBe(200);
    const groups = (await guest.get("/g")).text;
    expect(groups).toContain('href="/g/');
    expect(groups).not.toContain("Guests");
    expect((await guest.get("/badges")).status).toBe(200);
    expect((await guest.get("/badges/999999")).status).toBe(404);
    const about = (await guest.get("/about")).text;
    expect(about).toContain("Our staff");
    expect(about).toContain('href="/u/alice"');
    await forum.sql("UPDATE users SET profile_visibility = 'nobody' WHERE username = 'bob'");
    expect((await guest.get("/u")).text).not.toContain('href="/u/bob"');
    await forum.sql("UPDATE users SET profile_visibility = 'everyone' WHERE username = 'bob'");
  });
  it("suggests existing tags by prefix", async () => {
    const r = await alice.get("/api/v1/autocomplete?kind=tag&q=she");
    expect(JSON.parse(r.text).map((t: { label: string }) => t.label)).toContain("sheet-tag");
  });
  it("feeds the header dropdowns, for members only", async () => {
    for (const path of ["/api/v1/me/notifications", "/api/v1/me/messages"]) {
      const r = await alice.get(path);
      expect(r.status).toBe(200);
      expect(Array.isArray(JSON.parse(r.text))).toBe(true);
      expect((await guest.get(path)).status).toBe(403);
    }
  });
});

describe("calendar", () => {
  it("the Add event button is for members only", async () => {
    expect((await alice.get("/calendar")).text).toContain('data-dialog="new-event"');
    expect((await guest.get("/calendar")).text).not.toContain('data-dialog="new-event"');
    expect((await guest.get(`/calendar/new?forum=${FORUM}`)).status).toBe(403);
  });
  it("leads to the composer with the calendar part open", async () => {
    const r = await alice.get(`/calendar/new?forum=${FORUM}&date=2031-05-17`);
    expect(r.status).toBe(303);
    expect(r.location).toContain(`?event=1&date=2031-05-17#new-topic`);
    const page = await alice.get(r.location.replace(forum.base, ""));
    expect(page.text).toContain('name="event_date" value="2031-05-17"');
    expect((await alice.get("/calendar/new?forum=999999")).status).toBe(404);
  });
  it("a topic made with an event lands on the calendar, a bad date makes no topic", async () => {
    const bad = await alice.form(`/c/${FORUM}/new`, { title: "Event with a bad date", body: "Body of an event topic.", event_date: "2031-13-45" });
    expect(bad.status).toBe(422);
    expect((await alice.get("/latest")).text).not.toContain("Event with a bad date");
    const ok = await alice.form(`/c/${FORUM}/new`, {
      title: "Spring meetup",
      body: "Body of an event topic.",
      event_date: "2031-05-17",
      event_start: "18:30",
      event_timezone: "Europe/Warsaw",
      event_location: "Cafe",
    });
    expect(ok.status).toBe(303);
    const month = await guest.get("/calendar?month=2031-05");
    expect(month.text).toContain("Spring meetup");
    expect(month.text).not.toContain("Event with a bad date");
  });
  it("the calendar widget lists the same event", async () => {
    expect((await alice.api("/api/v1/admin/widgets", { area: "home_top", kind: "calendar", title: "", cache_seconds: 0, config: {} })).status).toBe(201);
    expect((await guest.get("/")).text).toContain("bf-minical");
  });
});

describe("editor limits", () => {
  it("members get the post length limit for the editor's counter, guests do not", async () => {
    expect((await alice.get("/")).text).toMatch(/<meta name="bf-post" content="\d+"/);
    expect((await guest.get("/")).text).not.toContain('name="bf-post"');
  });
});
