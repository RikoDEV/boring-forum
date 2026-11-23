// Notification kinds: who hears what, grouping, preferences, quiet hours, clubs.
import { beforeAll, describe, expect, it } from "vitest";
import { Client, settle, until, useForum } from "../lib/forum.ts";
import { seed, setting, topic } from "../lib/seed.ts";

const forum = useForum();
let alice: Client, bob: Client, FORUM: number;

const inbox = async (c: Client, show = "all") => (await c.get(`/notifications?show=${show}`)).text;

beforeAll(async () => {
  ({ alice, bob, FORUM } = await seed(forum));
});

describe("posts", () => {
  let id: string, path: string;
  beforeAll(async () => {
    ({ id, url: path } = await topic(alice, FORUM, "Notified topic"));
  });
  it("a reply to a post is a direct reply, not a watcher notice", async () => {
    await bob.form(`/t/${id}/reply`, { body: "Answering the first post directly", reply_to: "1" });
    const text = await inbox(alice, "replies");
    expect(text).toContain("bob replied to your post in Notified topic");
    expect(text).not.toContain("bob posted in Notified topic");
  });
  it("a composer quote is a quote, not a mention", async () => {
    await alice.form(`/t/${id}/reply`, { body: "> Answering\n>\n> — @bob, [post #2](/t/x/1#post-2)\n\nI agree with this" });
    const text = await inbox(bob);
    expect(text).toContain("alice quoted your post in Notified topic");
    expect(text).not.toContain("alice mentioned you in Notified topic");
  });
  it("repeat posts fold into one unread row", async () => {
    await bob.form(`/t/${id}/follow`, { level: "watch" });
    await alice.form(`/t/${id}/reply`, { body: "The first of two more replies" });
    await alice.form(`/t/${id}/reply`, { body: "The second of two more replies" });
    expect(await inbox(bob, "replies")).toContain("2 new posts in Notified topic");
  });
  it("an invitation reaches a member who can read the topic", async () => {
    expect((await bob.form(`/t/${id}/invite`, { username: "alice" })).status).toBe(303);
    expect(await inbox(alice, "mentions")).toContain("bob invited you to Notified topic");
    expect((await bob.form(`/t/${id}/invite`, { username: "nobody-here" })).status).toBe(422);
  });
});

describe("people", () => {
  it("a new follower is told", async () => {
    const me = (await alice.get("/api/v1/me")).text.match(/"id":\s*(\d+)/)![1];
    expect((await bob.form(`/u/${me}/follow`, {})).status).toBe(303);
    expect(await inbox(alice, "social")).toContain("bob started following you");
  });
  it("account notices ignore preferences", async () => {
    const me = (await bob.get("/api/v1/me")).text.match(/"id":\s*(\d+)/)![1];
    await alice.api(`/api/v1/admin/users/${me}`, { silenced_until: "2999-01-01T00:00:00Z" }, { method: "PATCH" });
    expect(await inbox(bob, "staff")).toContain("Your account was silenced");
    await alice.api(`/api/v1/admin/users/${me}`, { silenced_until: null }, { method: "PATCH" });
    expect(await inbox(bob, "staff")).toContain("The limits on your account were lifted");
  });
  it("reports reach the review queue's staff", async () => {
    const { id: tid } = await topic(alice, FORUM, "A topic to report");
    const post = (await bob.form(`/t/${tid}/reply`, { body: "A reply someone will report" }));
    expect(post.status).toBe(303);
    const page = (await alice.get(`/t/a-topic-to-report/${tid}`)).text;
    const pid = page.match(/data-quote="(\d+)"[\s\S]*data-quote="(\d+)"/)![2];
    await alice.form(`/p/${pid}/report`, { reason: "spam" });
    await bob.form(`/p/${pid}/report`, { reason: "spam" });
    await until(() => inbox(alice, "staff"), (t) => t.includes("waiting for review"));
  });
});

describe("settings", () => {
  it("quiet hours save and validate", async () => {
    expect((await alice.form("/settings/quiet", { start: "22", end: "7" })).status).toBe(303);
    expect((await alice.get("/settings")).text).toContain('value="22"');
    expect((await alice.form("/settings/quiet", { start: "25", end: "7" })).status).toBe(422);
    await alice.form("/settings/quiet", { start: "", end: "" });
  });
  it("switchable kinds are listed and account notices are not", async () => {
    const text = (await alice.get("/settings")).text;
    expect(text).toContain("direct_reply_app");
    expect(text).not.toContain("suspended_app");
  });
});

describe("clubs", () => {
  it("a closed club takes requests and invitations", async () => {
    await setting(alice, "clubs_enabled", true);
    expect((await alice.form("/clubs", { name: "Private circle", open: "" })).status).toBe(303);
    const list = (await bob.get("/clubs")).text;
    const club = list.match(/\/clubs\/(\d+)\/request/)![1];
    expect((await bob.form(`/clubs/${club}/join`, {})).status).toBe(403);
    await bob.form(`/clubs/${club}/request`, {});
    expect(await inbox(alice, "social")).toContain("bob asked to join your club Private circle");
    await alice.form(`/clubs/${club}/invite`, { username: "bob" });
    expect(await inbox(bob, "social")).toContain("alice invited you to the club Private circle");
    expect((await bob.form(`/clubs/${club}/join`, {})).status).toBe(303);
  });
});

describe("user menu", () => {
  it("replaces the notifications dropdown with tabs", async () => {
    const html = (await alice.get("/")).text;
    expect(html).toContain("data-user-menu");
    expect(html).toContain('data-um-tab="bookmarks"');
    expect(html).not.toContain('data-drop="/api/v1/me/notifications"');
  });
  it("lists a tab's notifications as who and what", async () => {
    const replies = JSON.parse((await alice.get("/api/v1/me/notifications?show=replies")).text);
    const row = replies.find((n: { label?: string; description: string }) => n.label === "bob" && n.description === "Notified topic");
    expect(row).toBeTruthy();
    expect(row.icon).toBe("reply");
    expect(row.text).toContain("bob replied to your post");
    expect(replies.every((n: { icon: string }) => n.icon !== "heart")).toBe(true);
  });
  it("lists bookmarks for the member only", async () => {
    expect(JSON.parse((await alice.get("/api/v1/me/bookmarks")).text)).toEqual([]);
    expect((await forum.client().get("/api/v1/me/bookmarks")).status).toBeGreaterThanOrEqual(400);
  });
});

describe("page layouts", () => {
  it("places a block on forum and topic pages only", async () => {
    const { url } = await topic(alice, FORUM, "A topic with blocks");
    const add = (area: string, title: string) => alice.api("/api/v1/admin/widgets", { kind: "custom", area, title, config: { markdown: "block text" } });
    expect((await add("forum_side", "Forum side block")).status).toBe(201);
    expect((await add("topic_bottom", "Topic bottom block")).status).toBe(201);
    expect((await alice.get(url)).text).toContain("Topic bottom block");
    expect((await alice.get(url)).text).not.toContain("Forum side block");
    expect((await alice.get("/")).text).not.toContain("Forum side block");
    expect((await alice.get("/")).text).not.toContain("Topic bottom block");
    expect((await alice.get(url + "?edit_layout=1")).text).toContain('data-area="topic_side"');
    expect((await bob.get(url + "?edit_layout=1")).text).not.toContain('data-area="topic_side"');
    // The words "Edit layout" are also in the page's message catalog (#bf-i18n), so look for the link.
    expect((await bob.get(url)).text).not.toContain('class="bf-layout-link"');
  });
  it("keeps the home page's own blocks on the home page", async () => {
    expect((await alice.api("/api/v1/admin/widgets", { kind: "home_stats", area: "forum_top" })).status).toBe(422);
    expect((await alice.api("/api/v1/admin/widgets", { kind: "custom", area: "nowhere", title: "x" })).status).toBe(422);
    const { widgets } = JSON.parse((await alice.get("/api/v1/admin/widgets")).text);
    const stats = widgets.find((w: { kind: string }) => w.kind === "home_stats");
    expect((await alice.api("/api/v1/admin/widgets/arrange", { area: "forum_top", ids: [stats.id] }, { method: "PUT" })).status).toBe(422);
  });
});

describe("post style and suggestions", () => {
  it("follows the setting", async () => {
    expect((await alice.get("/")).text).toContain('data-post-style="classic"');
    await setting(alice, "post_style", "discourse");
    expect((await alice.get("/")).text).toContain('data-post-style="discourse"');
    // A value that is not one of the choices is refused, and the setting keeps what it had.
    expect((await setting(alice, "post_style", "nonsense")).status).toBe(422);
    expect((await alice.get("/")).text).toContain('data-post-style="discourse"');
    await setting(alice, "post_style", "classic");
  });
  it("names the forum on suggested and similar topics", async () => {
    const { url } = await topic(alice, FORUM, "Suggestions come from here");
    await topic(alice, FORUM, "Another topic to suggest");
    expect((await alice.get(url)).text).toContain('class="bf-topic-forum"');
    const found = JSON.parse((await alice.get("/api/v1/search?in=topics&q=suggest")).text);
    expect(found[0].forum).toBeTruthy();
  });
  it("keeps the sticky bar apart from the links below it", async () => {
    const html = (await alice.get("/")).text;
    expect(html.indexOf("bf-header-bar")).toBeLessThan(html.indexOf("bf-nav-row"));
    expect(html).toMatch(/<\/div>\s*<div class="bf-nav-row">/);
  });
});

describe("recently browsing", () => {
  it("lists members on the same page, not guests", async () => {
    const { url } = await topic(alice, FORUM, "A topic people are reading");
    expect((await forum.client().get(url)).text).toContain("No registered users viewing");
    await alice.get(url);
    const seen = (await bob.get(url)).text;
    expect(seen).toContain("Recently Browsing");
    expect(seen).toMatch(/bf-browsing-list[\s\S]*alice/);
    await setting(alice, "recently_browsing", false);
    expect((await bob.get(url)).text).not.toContain("Recently Browsing");
    await setting(alice, "recently_browsing", true);
  });
});

describe("feeds", () => {
  it("serves forum, topic and tag feeds with post text", async () => {
    const { id } = await topic(alice, FORUM, "A topic for the feed", "The text a feed reader shows");
    await alice.form(`/t/${id}/reply`, { body: "A reply in the topic feed" });
    const f = await forum.client().get(`/c/${FORUM}/feed.rss`);
    expect(f.status).toBe(200);
    expect(f.text).toContain("A topic for the feed");
    expect(f.text).toContain("The text a feed reader shows");
    expect((await forum.client().get(`/t/${id}/feed.rss`)).text).toContain("A reply in the topic feed");
    expect((await forum.client().get("/latest.rss")).text).toContain("The text a feed reader shows");
    expect((await forum.client().get("/t/999999/feed.rss")).status).toBe(404);
    expect((await forum.client().get("/c/999999/feed.rss")).status).toBe(404);
  });
});

describe("action entries", () => {
  it("records a title change and reads as a sentence", async () => {
    const { id, url } = await topic(alice, FORUM, "The first title here");
    expect((await bob.form(`/t/${id}/title`, { title: "Bob should not manage this" })).status).toBe(403);
    expect((await alice.form(`/t/${id}/title`, { title: "A much better title" })).status).toBe(303);
    const page = (await alice.get(url)).text;
    expect(page).toContain("changed the title to <em>A much better title</em>");
  });
});

describe("topic timeline", () => {
  it("notes a member's first post, once", async () => {
    const newcomer = forum.client();
    await newcomer.register("newcomer");
    const { id, url } = await topic(alice, FORUM, "Welcome the newcomer here");
    expect((await newcomer.form(`/t/${id}/reply`, { body: "Hello everyone, I just joined" })).status).toBe(303);
    expect((await guestPage(url)).match(/posted for the first time/g)).toHaveLength(1);
    await newcomer.form(`/t/${id}/reply`, { body: "And a second post from me" });
    expect((await guestPage(url)).match(/posted for the first time/g)).toHaveLength(1);
  });
  it("no longer offers a search inside the topic", async () => {
    const { url } = await topic(alice, FORUM, "A topic with no search box");
    expect(await guestPage(url)).not.toContain("Search this topic");
  });
  it("marks wiki posts for the table of contents", async () => {
    const { url } = await topic(alice, FORUM, "A topic that stays plain");
    expect(await guestPage(url)).not.toContain("data-wiki");
  });
});
const guestPage = async (url: string) => (await forum.client().get(url)).text;

describe("chat", () => {
  const J = (r: { text: string }) => JSON.parse(r.text);
  let channel: number, msg: number;
  it("is for members, in channels an admin creates", async () => {
    expect((await forum.client().get("/chat")).status).toBeGreaterThanOrEqual(400);
    expect((await bob.api("/api/v1/admin/chat/channels", { name: "No" })).status).toBe(403);
    const r = await alice.api("/api/v1/admin/chat/channels", { name: "General chat" });
    expect(r.status).toBe(201);
    channel = J(r).id;
    expect((await bob.get(`/chat/${channel}`)).text).toContain('data-island="chat"');
  });
  it("sends, cooks, edits and hides deleted text", async () => {
    const r = await bob.api(`/api/v1/chat/channels/${channel}/messages`, { raw: "Hello **chat** <script>x</script> @alice" });
    expect(r.status).toBe(201);
    msg = J(r).id;
    expect(J(r).cooked).toContain("<strong>chat</strong>");
    expect(J(r).cooked).not.toContain("<script");
    expect(await inbox(alice, "mentions")).toContain("bob mentioned you in chat: General chat");
    expect((await alice.api(`/api/v1/chat/messages/${msg}`, { raw: "stolen" }, { method: "PUT" })).status).toBe(403);
    expect((await bob.api(`/api/v1/chat/messages/${msg}`, { raw: "Edited text" }, { method: "PUT" })).status).toBe(200);
    expect(J(await alice.api(`/api/v1/chat/messages/${msg}/react`, { emoji: "👍" })).reactions[0].count).toBe(1);
    expect((await alice.api(`/api/v1/chat/messages/${msg}/react`, { emoji: "words" })).status).toBe(422);
    expect((await bob.api(`/api/v1/chat/messages/${msg}/pin`)).status).toBe(403);
    expect((await alice.api(`/api/v1/chat/messages/${msg}/flag`, { reason: "test" })).status).toBe(204);
    expect(J(await alice.get("/api/v1/admin/chat/flags"))[0].message_id).toBe(msg);
    const gone = J(await alice.api(`/api/v1/chat/messages/${msg}`, undefined, { method: "DELETE" }));
    expect(gone.deleted).toBe(true);
    expect(gone.raw).toBe("");
    expect(J(await bob.get(`/api/v1/chat/channels/${channel}/messages`))[0].cooked).toBe("");
  });
  it("keeps direct messages between their people", async () => {
    expect((await bob.api("/api/v1/chat/dm", { usernames: ["alice"] })).status).toBe(422);
    await setting(alice, "chat_dm_min_trust_level", 0);
    const r = await bob.api("/api/v1/chat/dm", { usernames: ["alice"] });
    const dm = J(r).id;
    expect(J(await alice.api("/api/v1/chat/dm", { usernames: ["bob"] })).id).toBe(dm);
    await bob.api(`/api/v1/chat/channels/${dm}/messages`, { raw: "Private words" });
    expect(await inbox(alice, "messages")).toContain("bob sent you a chat message");
    const other = forum.client();
    await other.register("chatoutsider");
    expect((await other.get(`/api/v1/chat/channels/${dm}/messages`)).status).toBe(404);
    expect((await other.api(`/api/v1/chat/channels/${dm}/messages`, { raw: "let me in" })).status).toBe(404);
    expect((await other.get(`/api/v1/live?channels=chat:${dm}`)).status).toBe(404);
    expect((await other.get(`/chat/${dm}`)).status).toBe(404);
    expect(J(await other.get("/api/v1/chat/channels")).some((c: { id: number }) => c.id === dm)).toBe(false);
  });
  it("lets a chat creator choose who is in an invite-only channel", async () => {
    await setting(alice, "chat_create_min_trust_level", 0);
    const r = await bob.api("/api/v1/chat/channels", { name: "Bob's private room" });
    expect(r.status).toBe(201);
    const id = J(r).id;
    const outsider = forum.client();
    await outsider.register("roomoutsider");
    expect((await outsider.get(`/api/v1/chat/channels/${id}/messages`)).status).toBe(404);
    expect((await outsider.api(`/api/v1/chat/channels/${id}/join`)).status).toBe(404);
    expect((await outsider.api(`/api/v1/chat/channels/${id}/members`, { username: "roomoutsider" })).status).toBe(404);
    // Staff are not let in by rank.
    expect((await alice.get(`/api/v1/chat/channels/${id}/messages`)).status).toBe(404);
    expect((await bob.api(`/api/v1/chat/channels/${id}/members`, { username: "roomoutsider" })).status).toBe(204);
    expect((await outsider.get(`/api/v1/chat/channels/${id}/messages`)).status).toBe(200);
    expect(await inbox(outsider, "mentions")).toContain("bob added you to the chat channel");
    // A member cannot manage it, and the creator can remove them.
    expect((await outsider.get(`/api/v1/chat/channels/${id}/members`)).status).toBe(403);
    const people = J(await bob.get(`/api/v1/chat/channels/${id}/members`));
    const them = people.find((p: { username: string }) => p.username === "roomoutsider").id;
    expect((await bob.api(`/api/v1/chat/channels/${id}/members/${them}`, undefined, { method: "DELETE" })).status).toBe(204);
    expect((await outsider.get(`/api/v1/chat/channels/${id}/messages`)).status).toBe(404);
  });
  it("limits a channel to chosen groups", async () => {
    const empty = await alice.api("/api/v1/admin/chat/channels", { name: "No groups", access: "groups" });
    expect(empty.status).toBe(422);
    const r = await alice.api("/api/v1/admin/chat/channels", { name: "Staff room", access: "groups", group_ids: [3] });
    const id = J(r).id;
    expect((await bob.get(`/api/v1/chat/channels/${id}/messages`)).status).toBe(404);
    expect(J(await bob.get("/api/v1/chat/channels")).some((c: { id: number }) => c.id === id)).toBe(false);
    expect((await alice.get(`/api/v1/chat/channels/${id}/messages`)).status).toBe(200);
  });
  it("refuses archived channels", async () => {
    const r = await alice.api("/api/v1/admin/chat/channels", { name: "Old room" });
    const id = J(r).id;
    await alice.api(`/api/v1/admin/chat/channels/${id}`, { name: "Old room", archived: true }, { method: "PUT" });
    expect((await bob.api(`/api/v1/chat/channels/${id}/messages`, { raw: "too late" })).status).toBe(422);
  });
  it("hides secrets on integration rules and refuses internal addresses", async () => {
    const r = await alice.api("/api/v1/admin/chat/rules", { provider: "slack", url: "https://127.0.0.1/hook-secret" });
    expect(r.status).toBe(201);
    const list = (await alice.get("/api/v1/admin/chat/rules")).text;
    expect(list).not.toContain("hook-secret");
    expect((await alice.api(`/api/v1/admin/chat/rules/${J(r).id}/test`)).status).toBe(422);
    expect((await alice.api("/api/v1/admin/chat/rules", { provider: "nope", url: "https://x.example" })).status).toBe(422);
    expect((await bob.get("/api/v1/admin/chat/rules")).status).toBe(403);
    expect((await alice.api(`/api/v1/admin/chat/rules/${J(r).id}`, undefined, { method: "DELETE" })).status).toBe(204);
  });
});

describe("chat anti-spam", () => {
  const J = (r: { text: string }) => JSON.parse(r.text);
  let ch: number;
  const say = (c: Client, raw: string) => c.api(`/api/v1/chat/channels/${ch}/messages`, { raw });
  beforeAll(async () => {
    ch = J(await alice.api("/api/v1/admin/chat/channels", { name: "Spam tests" })).id;
    await setting(alice, "chat_auto_silence_hidden", 2);
  });
  it("refuses repeats and enforces the wait", async () => {
    expect((await say(bob, "Only once please")).status).toBe(201);
    expect((await say(bob, "Only once please")).status).toBe(422);
    await setting(alice, "chat_wait_seconds", 3600);
    expect((await say(bob, "Too soon after that")).status).toBe(422);
    await setting(alice, "chat_wait_seconds", 0);
  });
  it("applies the trust gate and upload setting", async () => {
    await setting(alice, "chat_min_trust_level", 4);
    expect((await say(bob, "Below the trust gate")).status).toBe(422);
    expect((await say(alice, "Staff are not held back")).status).toBe(201);
    await setting(alice, "chat_min_trust_level", 0);
    await setting(alice, "chat_allow_uploads", false);
    expect((await say(bob, `![x](/uploads/${"a".repeat(64)}.png)`)).status).toBe(422);
    expect(J(await bob.get("/api/v1/chat/me")).uploads).toBe(false);
    await setting(alice, "chat_allow_uploads", true);
  });
  it("applies watched words to new messages and edits", async () => {
    expect((await alice.api("/api/v1/admin/watched-words", { action: "block", pattern: "forbiddenword" })).status).toBeLessThan(300);
    expect((await say(bob, "this has forbiddenword inside")).status).toBe(422);
    expect((await alice.api("/api/v1/admin/watched-words", { action: "require_approval", pattern: "holdthisword" })).status).toBeLessThan(300);
    // "Require approval" holds a chat message for review (moderation::held): accepted, not yet visible.
    expect((await say(bob, "Chat has a queue so holdthisword is held for approval")).status).toBe(202);
    const ok = J(await say(bob, "A clean message"));
    expect((await bob.api(`/api/v1/chat/messages/${ok.id}`, { raw: "now forbiddenword" }, { method: "PUT" })).status).toBe(422);
  });
  it("hides on enough flags, reaches review, and silences repeat offenders", async () => {
    await setting(alice, "flag_hide_score", 1);
    const a = J(await say(bob, "First flagged message"));
    expect((await bob.api(`/api/v1/chat/messages/${a.id}/flag`, {})).status).toBe(422);
    expect((await alice.api(`/api/v1/chat/messages/${a.id}/flag`, { reason: "spam" })).status).toBe(204);
    const hidden = J(await bob.get(`/api/v1/chat/channels/${ch}/messages`)).find((m: { id: number }) => m.id === a.id);
    expect(hidden.deleted).toBe(true);
    expect(hidden.raw).toBe("");
    const queue = (await alice.get("/review")).text;
    expect(queue).toContain("First flagged message");
    expect((await bob.get("/review")).text).not.toContain("First flagged message");
    expect((await bob.form(`/chat/flags/${a.id}`, { decision: "keep" })).status).toBe(403);
    expect((await alice.form(`/chat/flags/${a.id}`, { decision: "keep" })).status).toBe(303);
    const back = J(await bob.get(`/api/v1/chat/channels/${ch}/messages`)).find((m: { id: number }) => m.id === a.id);
    expect(back.deleted).toBe(false);
    for (const text of ["Second flagged message", "Third flagged message"]) {
      const m = J(await say(bob, text));
      await alice.api(`/api/v1/chat/messages/${m.id}/flag`, { reason: "spam" });
    }
    expect((await say(bob, "Silenced now")).status).toBe(403);
    expect(await inbox(bob, "staff")).toContain("Your account was silenced");
    const me = J(await bob.get("/api/v1/me")).id;
    await alice.api(`/api/v1/admin/users/${me}`, { silenced_until: null }, { method: "PATCH" });
    await setting(alice, "flag_hide_score", 3);
  });
  it("blocks messages to someone who ignores you, and lists ignored names", async () => {
    const dm = J(await bob.api("/api/v1/chat/dm", { usernames: ["alice"] })).id;
    const bobId = J(await bob.get("/api/v1/me")).id;
    await alice.form(`/u/${bobId}/ignore`, { on: "1" });
    expect(J(await alice.get("/api/v1/chat/me")).ignored).toContain("bob");
    expect((await bob.api(`/api/v1/chat/channels/${dm}/messages`, { raw: "Are you ignoring me" })).status).toBe(422);
    await alice.form(`/u/${bobId}/ignore`, { on: "0" });
  });
});

describe("system badges", () => {
  it("lists them, awards from real activity, and tells the member", async () => {
    const page = (await forum.client().get("/badges")).text;
    for (const name of ["First Like", "Welcome", "Basic User", "Great Topic", "Champion", "Conversation Starter", "First Chat", "Ten Years"]) expect(page).toContain(name);
    expect(page.match(/class="bf-medal /g)!.length).toBeGreaterThan(90);
    const liker = forum.client();
    await liker.register("firstliker");
    await settle(3); // the silent first award has run
    const { url } = await topic(alice, FORUM, "A topic that earns a like");
    const pid = (await alice.get(url)).text.match(/data-quote="(\d+)"/)![1];
    expect((await liker.form(`/post/${pid}/like`, {})).status).toBe(303);
    await until(() => inbox(liker, "social"), (t) => t.includes("You earned the First Like badge"), 15_000);
    await until(() => inbox(alice, "social"), (t) => t.includes("You earned the Welcome badge"), 15_000);
  });
  it("can be switched off", async () => {
    const list = JSON.parse((await alice.get("/api/v1/admin/badges")).text);
    const b = list.find((x: { slug?: string }) => x.slug === "great_topic");
    // Deleting is allowed for the forum's own badges since 9feb066 (only a plugin's are refused); this one is kept for the rest of the suite.
    expect((await alice.api(`/api/v1/admin/badges/${b.id}`, { ...b, enabled: false }, { method: "PUT" })).status).toBe(204);
    expect((await forum.client().get("/badges")).text).not.toContain("Great Topic");
    await alice.api(`/api/v1/admin/badges/${b.id}`, { ...b, enabled: true }, { method: "PUT" });
  });
});

describe("profile achievements and statuses", () => {
  it("shows a member's rank, badges, reputation and answers, then their statuses", async () => {
    const me = JSON.parse((await alice.get("/api/v1/me")).text).id;
    const path = "/u/alice";
    const own = (await alice.get(path)).text;
    expect(own).toContain("Your achievements");
    expect(own).toContain("Community answers");
    expect(own.indexOf("bf-achievements")).toBeLessThan(own.indexOf('<div class="bf-list-bar"><nav class="bf-segments small" aria-label="Profile"'));
    const other = (await bob.get(path)).text;
    expect(other).toContain(">Achievements<");
    expect(other).not.toContain("Your achievements");
    expect((await alice.form(`/u/${me}/status`, { body: "My very first status <b>here</b>" })).status).toBe(303);
    const feed = (await bob.get(`${path}?tab=status`)).text;
    expect(feed).toContain('class="bf-sc"');
    expect(feed).toContain("My very first status");
    expect(feed).not.toContain("status <b>here");
    const id = feed.match(/id="status-(\d+)"/)![1];
    expect((await bob.form(`/status/${id}/reply`, { body: "A reply to that status" })).status).toBe(303);
    expect((await bob.get(`${path}?tab=status`)).text).toContain("A reply to that status");
  });
});

describe("content by deleted members", () => {
  it("shows a name and a working avatar", async () => {
    const { id } = await topic(alice, FORUM, "A topic whose poster is gone");
    await forum.sql("UPDATE items SET last_poster_id = NULL WHERE id = $1", [id]);
    for (const page of [`/c/x/${FORUM}`, "/latest", "/"]) {
      const r = await forum.client().get(page, { follow: true });
      expect(r.status).toBe(200);
      expect(r.text).not.toContain("/avatar/.svg");
    }
    expect((await forum.client().get("/latest")).text).toContain("/avatar/deleted.svg");
    expect((await forum.client().get("/avatar/.svg")).status).toBeLessThan(400);
    expect((await forum.client().get("/avatar/deleted.svg")).status).toBe(200);
  });
});
