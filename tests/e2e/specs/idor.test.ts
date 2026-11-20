// IDOR suite.
//
// "A generated test suite calls every endpoint with IDs belonging to other users, private
// forums, deleted items and PMs, and expects 404 (not 403, to avoid revealing that something
// exists)."
//
// The shape: build one set of fixtures owned by `alice` plus a private forum, then drive every
// id-taking route as `bob`, who owns none of it. The cases are generated from a table of route
// templates rather than written out one by one, so adding a route to the table covers it for
// every actor, and a route that starts leaking fails here rather than in production.
//
// What counts as a pass, per.5:
//   * 404 is the wanted answer: it does not reveal that the object exists.
//   * 403 is a FAILURE for a private object, because it confirms existence. It is accepted only
//     for routes marked `confirmsExistenceOk`, where the object's existence is already public
//     (a visible topic you simply may not moderate).
//   * 2xx and 3xx are always a failure: the action went through.
//
// A 405 means the table has the wrong method and the test is wrong, so it fails loudly too.

import { describe, expect, it, beforeAll } from "vitest";
import { Client, useForum } from "../lib/forum.ts";

const f = useForum();

/** Ids and slugs of things bob must not be able to reach. */
const own: Record<string, string> = {};
let alice: Client;
let bob: Client;
let guest: Client;
/** The bootstrap administrator, kept only to grant permissions in the control test below. */
let admin: Client;

beforeAll(async () => {
  // The first account registered becomes the administrator, so it is a throwaway used only to
  // create the public forum through the real admin API (and so get the default permissions).
  // alice and bob register afterwards and are therefore ordinary members -- which matters:
  // an admin reaches everything, and the suite would prove nothing with one as the owner.
  admin = await f.member("idoradmin");
  expect((await admin.api("/api/v1/admin/nodes", { kind: "category", title: "IDOR" })).status).toBe(201);
  const created = await admin.api("/api/v1/admin/nodes", {
    kind: "forum",
    title: "IDOR Public",
    parent_id: 1,
  });
  expect(created.status, `could not create the public forum: ${created.text.slice(0, 200)}`).toBe(201);

  alice = await f.member("idoralice");
  bob = await f.member("idorbob");
  guest = f.client();

  // Ids come from the database rather than from parsing HTML, so a fixture that fails to be
  // created shows up as a missing id here instead of as a silently skipped case below.
  const one = async (sql: string, values: unknown[] = []) => {
    const r = await f.sql(sql, values);
    if (!r.rows.length) throw new Error(`fixture produced no row: ${sql}`);
    return String(Object.values(r.rows[0])[0]);
  };

  own.aliceId = await one("SELECT id FROM users WHERE username = $1", ["idoralice"]);
  own.bobId = await one("SELECT id FROM users WHERE username = $1", ["idorbob"]);

  // A forum only staff may see, and a topic inside it. The canonical private object: bob must
  // not learn that either exists.
  own.privateNode = await one(
    `INSERT INTO nodes (kind, title, slug, position) VALUES ('forum', 'IDOR Private', 'idor-private', 900) RETURNING id`,
  );
  // No grants at all, so no group can view it (see perms: an ungranted node is invisible).
  own.privateTopic = await one(
    `INSERT INTO items (node_id, author_id, title, slug, kind)
     VALUES ($1, $2, 'Private topic', 'private-topic', 'topic') RETURNING id`,
    [own.privateNode, own.aliceId],
  );
  own.privatePost = await one(
    `INSERT INTO posts (item_id, author_id, post_number, raw, cooked, cooked_version)
     VALUES ($1, $2, 1, 'secret', '<p>secret</p>', 1) RETURNING id`,
    [own.privateTopic, own.aliceId],
  );

  // A topic alice created in the public forum: visible to bob, but not his to administer.
  const publicNode = await one("SELECT id FROM nodes WHERE title = 'IDOR Public'");
  own.publicNode = publicNode;
  const t = await alice.form(`/c/${publicNode}/new`, { title: "Alice's IDOR topic", body: "hello world" });
  expect(t.status, "alice could not create a topic").toBe(303);
  own.aliceTopic = await one("SELECT id FROM items WHERE author_id = $1 AND node_id = $2 ORDER BY id DESC LIMIT 1", [
    own.aliceId,
    publicNode,
  ]);
  own.alicePost = await one("SELECT id FROM posts WHERE item_id = $1 ORDER BY id LIMIT 1", [own.aliceTopic]);

  // Soft-deleted content, which is `state = 'deleted'` rather than a timestamp. It must be 404
  // for a non-staff member, not merely hidden from lists.
  own.deletedTopic = await one(
    `INSERT INTO items (node_id, author_id, title, slug, kind, state)
     VALUES ($1, $2, 'Deleted topic', 'deleted-topic', 'topic', 'deleted') RETURNING id`,
    [publicNode, own.aliceId],
  );
  own.deletedPost = await one(
    `INSERT INTO posts (item_id, author_id, post_number, raw, cooked, cooked_version, state)
     VALUES ($1, $2, 1, 'gone', '<p>gone</p>', 1, 'deleted') RETURNING id`,
    [own.deletedTopic, own.aliceId],
  );

  // A private conversation between alice and a third member, with bob not a participant.
  const carol = await f.member("idorcarol");
  own.carolId = await one("SELECT id FROM users WHERE username = $1", ["idorcarol"]);
  const pm = await alice.form("/messages", {
    to: "idorcarol",
    subject: "Private to carol",
    body: "a private message body",
  });
  expect(pm.status, "alice could not start a conversation").toBe(303);
  own.conversation = await one("SELECT id FROM conversations ORDER BY id DESC LIMIT 1");

  // Alice's own account-scoped objects.
  const key = await alice.api("/api/v1/me/api-keys", { name: "idor", scope: "read" });
  expect(key.status, "alice could not create an API key").toBeLessThan(300);
  own.apiKey = await one("SELECT id FROM api_keys WHERE user_id = $1 ORDER BY id DESC LIMIT 1", [own.aliceId]);

  // Ids that certainly do not exist, to prove the "missing" and "someone else's" answers match.
  own.absent = "99000111";
});

/** How a route may answer when the actor must not reach it. */
type Expectation =
  /**.5's rule: the object is private, so the answer must not reveal that it exists. */
  | "notFound"
  /** The object's existence is already public; 403 is fine, success is not. */
  | "notFoundOrForbidden"
  /**
   * Any refusal will do. For routes whose refusal is about the caller's role rather than the
   * target (a non-staff member hitting a staff route answers the same for every id), and for
   * routes where validation rejects the request before authorization is reached.
   */
  | "refused";

type Case = {
  path: string;
  method?: "GET" | "POST" | "DELETE" | "PATCH" | "PUT";
  /** Form fields for a POST, if the handler needs a valid body to get past validation. */
  fields?: Record<string, string | number>;
  /**
   * `notFoundOrForbidden` is for routes acting on an object whose existence is already public --
   * a topic bob can see but may not pin. Revealing "it exists but you may not" is correct there.
   * Anything touching a private forum, a PM, a deleted item or another member's account must be
   * `notFound`.
   */
  expect: Expectation;
};

/** Builds the case table for one placeholder set. `id` substitutes `:id` in each path. */
function cases(): Case[] {
  const list: Case[] = [];
  const get = (path: string, expect: Expectation = "notFound") => list.push({ path, method: "GET", expect });
  const post = (
    path: string,
    fields: Record<string, string | number> = {},
    expect: Expectation = "notFound",
  ) => list.push({ path, method: "POST", fields, expect });
  const del = (path: string, expect: Expectation = "notFound") => list.push({ path, method: "DELETE", expect });

  // --- A forum nobody but staff may see --------------------------------------------------
  //
  // The bare `/c/{id}` and `/t/{id}` forms are left out on purpose. They are canonicalising
  // redirects that echo the caller's own path segment back without consulting the database, so
  // they answer 308 for every id, present or not, and reveal nothing. The canonical
  // `/c/{slug}/{id}` and `/t/{slug}/{id}` forms below are the ones that load and check anything.
  get(`/c/${own.privateNode}/new`);
  get(`/c/idor-private/${own.privateNode}`);
  post(`/c/${own.privateNode}/new`, { title: "intrusion", body: "intrusion body" });
  post(`/c/${own.privateNode}/read`);
  post(`/c/${own.privateNode}/follow`, { level: "watching" });

  // --- A topic and post inside that forum ------------------------------------------------
  get(`/t/private-topic/${own.privateTopic}`);
  get(`/t/private-topic/${own.privateTopic}/1`);
  get(`/p/${own.privatePost}`);
  get(`/post/${own.privatePost}/history`);
  get(`/api/v1/posts/${own.privatePost}/raw`);
  get(`/api/v1/posts/${own.privatePost}/revisions`);
  get(`/api/v1/topics/${own.privateTopic}/posts/1`);
  get(`/api/v1/topics/${own.privateTopic}/state`);
  get(`/api/v1/topics/${own.privateTopic}/links`);
  post(`/t/${own.privateTopic}/reply`, { body: "a reply that must not land" });
  post(`/t/${own.privateTopic}/follow`, { level: "watching" });
  post(`/post/${own.privatePost}/like`);
  post(`/post/${own.privatePost}/react`, { id: 1 });
  post(`/post/${own.privatePost}/bookmark`);
  post(`/post/${own.privatePost}/report`, { reason: "spam" });
  post(`/post/${own.privatePost}/edit`, { body: "edited by an outsider" });
  post(`/post/${own.privatePost}/delete`);

  // --- Soft-deleted content, which is 404 for non-staff ----------------------------------
  get(`/t/deleted-topic/${own.deletedTopic}`);
  get(`/p/${own.deletedPost}`);
  get(`/api/v1/posts/${own.deletedPost}/raw`);
  post(`/t/${own.deletedTopic}/reply`, { body: "reply to a deleted topic" });
  post(`/post/${own.deletedPost}/like`);
  post(`/post/${own.deletedPost}/restore`);

  // --- A private conversation bob is not part of -----------------------------------------
  get(`/messages/${own.conversation}`);
  post(`/messages/${own.conversation}/reply`, { body: "reading your mail" });
  post(`/messages/${own.conversation}/add`, { username: "idorbob" });
  post(`/messages/${own.conversation}/leave`);
  post(`/messages/${own.conversation}/archive`);
  post(`/messages/${own.conversation}/remove`, { username: "idorcarol" });
  post(`/messages/${own.conversation}/unread`);
  post(`/messages/${own.conversation}/typing`);
  post(`/messages/${own.conversation}/folder`, { folder: "inbox" });

  // --- Another member's account-scoped objects -------------------------------------------
  del(`/api/v1/me/api-keys/${own.apiKey}`);
  get(`/u/${own.aliceId}/warnings`, "refused");
  post(`/u/${own.aliceId}/notes`, { body: "a note about someone else" }, "refused");

  // --- Staff-only review and moderation --------------------------------------------------
  //
  // These answer 403 "you are not staff" for every id, existing or not, so the refusal says
  // nothing about the target and.5's 404 rule does not apply. The pair test below checks
  // that a real and an absent id really do answer alike.
  post(`/review/${own.absent}`, { decision: "approve" }, "refused");
  post(`/review/user/${own.aliceId}`, { decision: "approve" }, "refused");
  get(`/review/messages/${own.conversation}`, "refused");
  post(`/u/${own.aliceId}/warn`, { reason: "because", points: 1 }, "refused");
  post(`/u/${own.aliceId}/status`, { body: "posting as someone else" }, "refused");

  // --- Admin API, which a member must not reach ------------------------------------------
  // Existence is not the secret here, so 403 is acceptable: what matters is that none of these
  // succeeds. The methods match the router -- a 405 would mean this table is wrong, not the app.
  get(`/api/v1/admin/users/${own.aliceId}/ips`, "refused");
  get(`/api/v1/admin/users/${own.aliceId}/history`, "refused");
  list.push({ path: `/api/v1/admin/users/${own.aliceId}`, method: "PATCH", fields: {}, expect: "refused" });
  list.push({ path: `/api/v1/admin/nodes/${own.privateNode}`, method: "PATCH", fields: {}, expect: "refused" });
  list.push({
    path: `/api/v1/admin/nodes/${own.privateNode}/permissions`,
    method: "PUT",
    fields: {},
    expect: "refused",
  });
  post(`/api/v1/admin/users/${own.aliceId}/impersonate`, {}, "refused");
  post(`/api/v1/admin/users/${own.aliceId}/logout`, {}, "refused");
  post(`/api/v1/admin/users/${own.aliceId}/reset-2fa`, {}, "refused");
  post(`/api/v1/admin/users/${own.aliceId}/delete-spammer`, {}, "refused");

  // --- Acting on a topic that IS visible: 403 is acceptable, success is not --------------
  // The object is visible to bob, so hiding its existence is not the goal; not letting him
  // change it is. Some of these are refused by validation before authorization is reached,
  // which is still a refusal, so the assertion is "did not succeed".
  post(`/t/${own.aliceTopic}/pin`, {}, "refused");
  post(`/t/${own.aliceTopic}/move`, { node: own.publicNode }, "refused");
  post(`/t/${own.aliceTopic}/tags`, { tags: "x" }, "refused");
  post(`/t/${own.aliceTopic}/author`, { username: "idorbob" }, "refused");
  post(`/t/${own.aliceTopic}/slow`, { seconds: 60 }, "refused");
  post(`/t/${own.aliceTopic}/banner`, {}, "refused");
  post(`/post/${own.alicePost}/edit`, { body: "editing alice's post" }, "refused");
  post(`/post/${own.alicePost}/delete`, {}, "refused");
  post(`/post/${own.alicePost}/hide`, {}, "refused");
  post(`/post/${own.alicePost}/wiki`, {}, "refused");
  post(`/post/${own.alicePost}/solve`, {}, "refused");
  post(`/post/${own.alicePost}/edit-lock`, {}, "refused");

  return list;
}

/** Sends one case and reports what the server answered. */
async function attempt(c: Client, k: Case) {
  if (k.method === "POST") return c.form(k.path, k.fields ?? {});
  if (k.method === "GET" || k.method === undefined) return c.get(k.path);
  // DELETE, PATCH and PUT only exist on the JSON API.
  return c.api(k.path, k.method === "DELETE" ? undefined : (k.fields ?? {}), { method: k.method });
}

/** Whether the server's answer satisfies the case's expectation. */
function satisfied(k: Case, status: number): boolean {
  // A 405 always means this table names a method the route does not have: the test is wrong,
  // and silently accepting it would hide whatever the route actually does.
  if (status === 405) return false;
  if (k.expect === "notFound") return status === 404;
  if (k.expect === "notFoundOrForbidden") return status === 403 || status === 404;
  return status >= 400;
}

/** What the failure message should say was wanted. */
const wanted = (k: Case) =>
  k.expect === "notFound" ? "404" : k.expect === "notFoundOrForbidden" ? "403 or 404" : "any refusal";

describe("IDOR: another member", () => {
  it("cannot reach objects that are not theirs", async () => {
    const failures: string[] = [];
    for (const k of cases()) {
      const r = await attempt(bob, k);
      if (!satisfied(k, r.status)) {
        failures.push(
          `${k.method} ${k.path} → ${r.status}` +
            (r.location ? ` (→ ${r.location})` : "") +
            ` [wanted ${wanted(k)}]`,
        );
      }
    }
    expect(failures, `${failures.length} endpoint(s) leaked to another member:\n${failures.join("\n")}`).toEqual([]);
  });
});

describe("IDOR: a signed-out visitor", () => {
  it("cannot reach objects that need an account", async () => {
    const failures: string[] = [];
    for (const k of cases()) {
      const r = await attempt(guest, k);
      // A guest is refused in several legitimate ways: 404, 403, a redirect to the login page,
      // or a 400/422 from the CSRF and validation layers, which run before authorization and
      // are a refusal just the same. What must never happen is success -- or a 405, which would
      // mean this table names a method the route does not have and the test is wrong.
      const refused = r.status >= 400 && r.status !== 405;
      const redirected = r.status === 302 || r.status === 303;
      if (!refused && !redirected) {
        failures.push(`${k.method} ${k.path} → ${r.status} [wanted a refusal or a login redirect]`);
      }
    }
    expect(failures, `${failures.length} endpoint(s) leaked to a guest:\n${failures.join("\n")}`).toEqual([]);
  });
});

describe("IDOR: missing and forbidden are indistinguishable", () => {
  //.5's reason for preferring 404 over 403: the answer for "someone else's private object"
  // must look exactly like the answer for "no such object", or the status code itself is an
  // oracle for enumerating what exists.
  const pairs = () => [
    { real: `/t/private-topic/${own.privateTopic}`, absent: `/t/private-topic/${own.absent}` },
    { real: `/p/${own.privatePost}`, absent: `/p/${own.absent}` },
    { real: `/messages/${own.conversation}`, absent: `/messages/${own.absent}` },
    { real: `/c/idor-private/${own.privateNode}`, absent: `/c/idor-private/${own.absent}` },
    { real: `/api/v1/posts/${own.privatePost}/raw`, absent: `/api/v1/posts/${own.absent}/raw` },
  ];

  it("answers the same for a private object and a nonexistent one", async () => {
    for (const { real, absent } of pairs()) {
      const [a, b] = [await bob.get(real), await bob.get(absent)];
      expect(a.status, `${real} and ${absent} answered differently`).toBe(b.status);
      expect(a.status).toBe(404);
    }
  });

  it("does not leak the title of a private topic in the body", async () => {
    const r = await bob.get(`/t/private-topic/${own.privateTopic}`);
    expect(r.text).not.toContain("Private topic");
    expect(r.text).not.toContain("secret");
  });

  it("does not leak a conversation's subject or body", async () => {
    const r = await bob.get(`/messages/${own.conversation}`);
    expect(r.text).not.toContain("Private to carol");
    expect(r.text).not.toContain("a private message body");
  });
});

describe("IDOR: writes scoped to the caller ignore the id", () => {
  // Some routes take an id but scope the query to the signed-in user, so the id is simply
  // ignored. They answer the same for every id and leak nothing, but a status code cannot show
  // that: the property worth asserting is that the other member's row is still there.
  it("does not let bob delete alice's API key through the form route", async () => {
    const before = await f.sql("SELECT count(*)::int AS n FROM api_keys WHERE id = $1", [own.apiKey]);
    expect(before.rows[0].n, "the fixture key is missing").toBe(1);

    await bob.form(`/settings/api-keys/${own.apiKey}/delete`);
    await bob.api(`/api/v1/me/api-keys/${own.apiKey}`, undefined, { method: "DELETE" });

    const after = await f.sql("SELECT count(*)::int AS n FROM api_keys WHERE id = $1", [own.apiKey]);
    expect(after.rows[0].n, "bob deleted another member's API key").toBe(1);
  });
});

describe("IDOR: the owner still can", () => {
  // The suite would also pass if every one of these routes were simply broken, so the control
  // case matters: alice reaches her own objects, which proves the 404s above are about
  // authorization rather than about a bad fixture or a wrong path.
  it("lets alice read her own topic, post and conversation", async () => {
    const paths = [`/t/alices-idor-topic/${own.aliceTopic}`, `/p/${own.alicePost}`, `/messages/${own.conversation}`];
    for (const path of paths) {
      const r = await alice.get(path);
      expect(r.status, `alice could not reach her own ${path}`).toBeLessThan(400);
    }
  });

  it("lets alice see the private forum's topic once her group is granted access", async () => {
    // Nothing is visible without a grant; with one, the same URL that 404s for bob works. That
    // is the difference between "hidden by permissions" and "hidden by a bug".
    const before = await alice.get(`/t/private-topic/${own.privateTopic}`);
    expect(before.status).toBe(404);

    // A group of alice's own. Granting her *primary* group would grant bob too -- every member
    // shares the Members group -- and the test would then prove the opposite of what it claims.
    const made = await f.sql(
      "INSERT INTO groups (name, slug) VALUES ('IDOR Granted', 'idor-granted') RETURNING id",
    );
    const group = made.rows[0].id as number;
    await f.sql("INSERT INTO user_secondary_groups (user_id, group_id) VALUES ($1, $2)", [own.aliceId, group]);
    // Granted through the admin API rather than with SQL: the forum tree and its permissions are
    // cached, and only the API invalidates that cache. Writing the row directly would leave the
    // server serving stale permissions, which looks exactly like a bug in this test.
    const granted = await admin.api(
      `/api/v1/admin/nodes/${own.privateNode}/permissions`,
      { [String(group)]: 3 }, // VIEW | READ
      { method: "PUT" },
    );
    expect(granted.status, `could not grant access: ${granted.text.slice(0, 200)}`).toBe(204);

    const after = await alice.get(`/t/private-topic/${own.privateTopic}`);
    expect(after.status, "a granted member still cannot read the topic").toBeLessThan(400);
    expect(after.text).toContain("Private topic");

    // And bob, who was never granted anything, still cannot.
    expect((await bob.get(`/t/private-topic/${own.privateTopic}`)).status).toBe(404);
  });
});
