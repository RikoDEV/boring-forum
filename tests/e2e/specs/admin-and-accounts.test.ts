import { createHash } from "node:crypto";
// Admin API, account security, likes, legal, approval, search filters, cooking, OIDC.
import { beforeAll, describe, expect, it } from "vitest";
import { Client, idFrom, useForum } from "../lib/forum.ts";
import { mailLink, seed, setting, topic, totp, type Seed } from "../lib/seed.ts";

const forum = useForum();
let s: Seed, alice: Client, bob: Client, guest: Client;
let SRC: string, srcUrl: string;
const patch = (body: unknown, path: string) => alice.api(path, body, { method: "PATCH" });
const put = (body: unknown, path: string) => alice.api(path, body, { method: "PUT" });

beforeAll(async () => {
  s = await seed(forum);
  ({ alice, bob, guest } = s);
  const t = await topic(alice, s.FORUM, "Split source topic", "The original first post text");
  SRC = t.id;
  srcUrl = t.url;
  await bob.form(`/t/${SRC}/reply`, { body: "A reply alice can like" });
});

describe("admin API", () => {
  it("is protected", async () => {
    expect((await alice.get("/api/v1/admin/stats")).text).toContain('"users":2');
    expect((await alice.api("/api/v1/admin/groups", { name: "x" }, { noCsrf: true })).status).toBe(400);
    expect((await bob.get("/api/v1/admin/stats")).status).toBe(403);
    expect((await alice.get("/api/v1/admin/users?q=bo")).text).toContain('"username":"bob"');
  });
  it("groups", async () => {
    expect((await alice.api("/api/v1/admin/groups", { name: "Helpers" })).status).toBe(201);
    expect((await alice.api("/api/v1/admin/groups", { name: "Helpers" })).status).toBe(422);
    expect((await patch({ primary_group_id: 2 }, "/api/v1/admin/users/1")).status).toBe(422);
  });
  it("silence", async () => {
    expect((await patch({ silenced_until: "2099-01-01T00:00:00Z" }, "/api/v1/admin/users/2")).status).toBe(204);
    expect((await bob.form(`/c/${s.FORUM}/new`, { title: "Silenced topic title", body: "Silenced body text here" })).status).toBe(403);
    expect((await patch({ silenced_until: null }, "/api/v1/admin/users/2")).status).toBe(204);
  });
  it("forum permissions and rename", async () => {
    const OFF = s.OFF;
    expect((await alice.get("/api/v1/admin/tree")).text).toContain('"title":"Offtopic"');
    expect((await put({ 2: 31, 3: 63, 4: 63 }, `/api/v1/admin/nodes/${OFF}/permissions`)).status).toBe(204);
    expect((await guest.get(`/c/offtopic/${OFF}`)).status).toBe(404);
    expect((await bob.get(`/c/offtopic/${OFF}`)).status).toBe(200);
    expect((await patch({ title: "Off Topic" }, `/api/v1/admin/nodes/${OFF}`)).status).toBe(204);
    expect((await bob.get(`/c/offtopic/${OFF}`)).location).toBe(`${forum.base}/c/off-topic/${OFF}`);
    expect((await patch({ parent_id: OFF }, "/api/v1/admin/nodes/1")).status).toBe(422);
  });
  it("scoped moderators", async () => {
    const url = `/api/v1/admin/nodes/${s.OFF}/moderators`;
    expect((await bob.get("/review")).status).toBe(403);
    expect((await put({ usernames: ["bob"] }, url)).status).toBe(204);
    expect((await bob.get("/review")).status).toBe(200);
    expect((await bob.form(`/t/${SRC}/mod`, { action: "pin" })).status).toBe(403);
    expect((await put({ usernames: [] }, url)).status).toBe(204);
    expect((await put({ usernames: ["nobody"] }, url)).status).toBe(422);
    expect((await alice.get("/api/v1/admin/audit?action=node.")).text).toContain('"action":"node.moderators"');
  });
  it("suspension signs out", async () => {
    expect((await patch({ suspended_until: "2099-01-01T00:00:00Z" }, "/api/v1/admin/users/2")).status).toBe(204);
    expect((await bob.get("/notifications")).status).toBe(403);
    expect((await patch({ suspended_until: null }, "/api/v1/admin/users/2")).status).toBe(204);
  });
  it("admin SPA serves client routes", async () => {
    // `admin_gate`: anyone but an administrator gets the site's 404 page, not the panel and not a login redirect.
    const res = await guest.get("/admin/users/");
    expect(res.status).toBe(404);
    expect(res.text).not.toContain("acp");
  });
});

describe("account security", () => {
  const NEW_PW = "brand new passphrase";
  let g: Client, l: Client, secret: string;
  it("email verification is single use", async () => {
    const v = await mailLink(forum, "alice@example.com", "/verify/");
    // The link counts only in a session of the account it was sent for: anyone else is asked to sign in
    // and the token is not used up.
    expect((await guest.get(v)).status).toBe(403);
    expect((await bob.get(v)).status).toBe(403);
    // GET only shows a button (mail scanners follow links); the POST consumes the token.
    const page = await alice.get(v);
    expect(page.status).toBe(200);
    expect(page.text).toContain("Confirm email address");
    expect((await alice.form(v)).text).toContain("Email verified");
    expect((await alice.get(v)).status).toBe(410);
    expect((await alice.form(v)).status).toBe(410);
    expect((await alice.get("/settings")).text).toContain("a•••@e•••.com — verified");
    expect((await alice.get("/settings")).text).not.toContain("alice@example.com");
  });
  it("password reset", async () => {
    g = forum.client();
    await g.get("/forgot");
    expect((await g.form("/forgot", { email: "nobody@example.com" })).text).toContain("If an account uses that address");
    expect((await g.form("/forgot", { email: "BOB@example.com" })).text).toContain("If an account uses that address");
    const r = await mailLink(forum, "bob@example.com", "/reset/");
    expect((await g.get(r)).status).toBe(200);
    expect((await g.form(r, { password: "short" })).status).toBe(422);
    expect((await g.form(r, { password: NEW_PW })).status).toBe(303);
    expect((await g.get(r)).status).toBe(410);
    expect((await g.get("/settings")).text).toContain("b•••@e•••.com");
    expect((await bob.get("/notifications")).status).toBe(403);
  });
  it("login uses the new password", async () => {
    l = forum.client();
    await l.get("/login");
    expect((await l.form("/login", { login: "bob", password: "another long passphrase" })).status).toBe(422);
    expect((await l.form("/login", { login: "bob", password: NEW_PW })).status).toBe(303);
    expect((await l.form("/settings/password", { current: "nope-nope-nope", new: "yet another passphrase" })).status).toBe(422);
  });
  it("sessions can be revoked", async () => {
    const page = (await l.get("/settings")).text;
    expect(page.split("bf-session-icon").length - 1).toBe(2);
    const other = idFrom(page, /name="id" value="([0-9a-f]{64})"/);
    expect((await l.form("/settings/sessions/revoke", { id: other })).status).toBe(303);
    expect((await g.get("/notifications")).status).toBe(403);
  });
  it("two-factor login", async () => {
    secret = idFrom((await l.get("/settings?setup=2fa")).text, /name="secret" value="([A-Z2-7]{20,})"/);
    expect((await l.form("/settings/2fa/enable", { secret, code: "000000" })).status).toBe(422);
    expect((await l.form("/settings/2fa/enable", { secret, code: totp(secret) })).text).toContain("Two-factor authentication is on");
    const m = forum.client();
    await m.get("/login");
    expect((await m.form("/login", { login: "bob", password: NEW_PW })).location).toBe(`${forum.base}/login/2fa`);
    expect((await m.get("/notifications")).status).toBe(403);
    expect((await m.form("/login/2fa", { code: "123456" })).status).toBe(422);
    // The enable step used the current 30 s step; the next one is accepted (±1 skew).
    expect((await m.form("/login/2fa", { code: totp(secret, 30) })).status).toBe(303);
    expect((await m.get("/notifications")).status).toBe(200);
    bob = m;
  });
});

describe("likes, listing, edit grace", () => {
  let WT: string, wtUrl: string, QID: string;
  it("likes", async () => {
    const page = (await alice.get(srcUrl)).text;
    const like = idFrom(page, /action="\/post\/(\d+)\/like"/);
    expect((await alice.form(`/post/${like}/like`)).status).toBe(303);
    expect((await alice.get(srcUrl)).text).toContain('bf-react-pill is-mine">❤️ 1');
    const un = await alice.form(`/post/${like}/like`, {}, { headers: { accept: "application/json" } });
    expect(un.text).toContain('"liked":false');
    expect(un.text).toContain('"count":0');
    const own = idFrom(page, /data-edit-post="(\d+)"/);
    expect((await alice.form(`/post/${own}/like`)).status).toBe(422);
  });
  it("top pages", async () => {
    expect((await guest.get("/top?period=all")).text).toContain("Split source topic");
    expect((await guest.get("/top?period=week")).text).toContain('class="current" aria-current="true">This week');
  });
  it("unlisted topics leave lists", async () => {
    const t = await topic(alice, s.FORUM, "Watched forum topic", "Hello watchers of this forum");
    WT = t.id;
    wtUrl = t.url;
    expect((await alice.form(`/t/${WT}/mod`, { action: "unlist" })).status).toBe(303);
    expect((await guest.get("/latest")).text).not.toContain("Watched forum topic");
    expect((await guest.get(wtUrl)).text).toContain('name="robots" content="noindex"');
    expect((await guest.get("/sitemap.xml")).text).not.toContain(`/t/watched-forum-topic/${WT}`);
    expect((await alice.form(`/t/${WT}/mod`, { action: "list" })).status).toBe(303);
  });
  it("edits in the grace period keep no revision", async () => {
    await alice.form(`/t/${WT}/reply`, { body: "Quick reply with a typo" });
    QID = [...(await alice.get(wtUrl)).text.matchAll(/data-edit-post="(\d+)"/g)].at(-1)![1];
    const base = async () => JSON.parse((await alice.get(`/api/v1/posts/${QID}/source`)).text).base;
    expect((await alice.form(`/post/${QID}/edit`, { body: "Quick reply without a typo", base: await base() })).status).toBe(303);
    expect((await alice.get(`/post/${QID}/history`)).text).not.toContain("<h2>Edit ");
    expect((await alice.form(`/post/${QID}/edit`, { body: "Quick reply, edited again", reason: "more", base: await base() })).status).toBe(303);
    expect((await alice.get(`/post/${QID}/history`)).text).toContain("<h2>Edit 1</h2>");
  });
  it("legal documents must be accepted", async () => {
    expect((await put({ body: "Be **kind**." }, "/api/v1/admin/legal/terms")).status).toBe(201);
    expect((await put({ body: "We keep little data." }, "/api/v1/admin/legal/privacy")).status).toBe(201);
    expect((await guest.get("/legal/terms")).text).toContain("<strong>kind</strong>");
    expect((await alice.form(`/t/${WT}/reply`, { body: "Posting before accepting" })).status).toBe(422);
    const docs = idFrom((await alice.get("/legal/accept")).text, /name="docs" value="([0-9,]+)"/);
    expect((await alice.form("/legal/accept", { docs })).status).toBe(303);
    expect((await alice.form(`/t/${WT}/reply`, { body: "Posting after accepting" })).status).toBe(303);
  });
  it("offers anonymous posting only when the site allows it", async () => {
    const url = /href="(\/t\/[^"#/]+\/\d+)"/.exec((await alice.get("/latest")).text)![1];
    const box = 'name="anonymous"';
    expect((await bob.get(url)).text).not.toContain(box);
    expect((await setting(alice, "anonymous_posting", true)).status).toBe(204);
    expect((await bob.get(url)).text).toContain(box);
    await setting(alice, "anonymous_posting", false);
  });
  it("refuses temporary email addresses when told to", async () => {
    const reg = { password: "a long enough passphrase", accept: 1 };
    const tryEmail = async (username: string, email: string) => {
      const c = forum.client();
      await c.get("/register");
      return c.form("/register", { ...reg, username, email });
    };
    expect((await setting(alice, "block_disposable_emails", true)).status).toBe(204);
    const refused = await tryEmail("tempa", "x@mailinator.com");
    expect(refused.status).toBe(422);
    expect(refused.text).toContain("cannot be used");
    expect((await tryEmail("tempb", "x@sub.yopmail.com")).status).toBe(422);
    expect((await tryEmail("tempc", "tempc@example.com")).status).toBe(303);
    await setting(alice, "block_disposable_emails", false);
    expect((await tryEmail("tempd", "tempd@mailinator.com")).status).toBe(303);
  });
  it("renames the site, but not to nothing", async () => {
    expect((await setting(alice, "site_name", "   ")).status).toBe(422);
    expect((await setting(alice, "site_name", "Renamed Place")).status).toBe(204);
    expect((await bob.get("/settings")).text).toContain('class="brand" href="/">Renamed Place<');
    expect((await setting(alice, "site_name", "Boring Forum")).status).toBe(204);
  });
  it("ALTCHA stops sign-ups without a fresh solution", async () => {
    expect((await forum.client().get("/api/v1/captcha/altcha")).status).toBe(404);
    expect((await setting(alice, "captcha_provider", "altcha")).status).toBe(204);
    expect((await forum.client().get("/api/v1/captcha/altcha")).status, "no secret, no challenge").toBe(404);
    expect((await setting(alice, "captcha_secret", "e2e-captcha-secret")).status).toBe(204);
    const solve = async () => {
      const c = JSON.parse((await forum.client().get("/api/v1/captcha/altcha")).text);
      for (let number = 0; number <= c.maxnumber; number++) {
        if (createHash("sha256").update(c.salt + number).digest("hex") === c.challenge) {
          return Buffer.from(JSON.stringify({ algorithm: c.algorithm, challenge: c.challenge, number, salt: c.salt, signature: c.signature })).toString("base64");
        }
      }
      throw new Error("unsolved");
    };
    const register = async (username: string, altcha?: string) => {
      const c = forum.client();
      expect((await c.get("/register")).text).toContain('data-island="altcha"');
      return c.form("/register", { username, email: `${username}@example.com`, password: "a long enough passphrase", accept: 1, ...(altcha ? { altcha } : {}) });
    };
    expect((await register("altchaa")).status).toBe(422);
    expect((await register("altchab", "bm90IGEgc29sdXRpb24=")).status).toBe(422);
    const solved = await solve();
    expect((await register("altchac", solved)).status).toBe(303);
    expect((await register("altchad", solved)).status, "a solution works once").toBe(422);
    await setting(alice, "captcha_provider", "");
  });
  it("opens the content policy to a challenge provider on the sign-up form only", async () => {
    await setting(alice, "captcha_site_key", "site-key");
    await setting(alice, "captcha_secret", "s");
    expect((await setting(alice, "captcha_provider", "recaptcha")).status).toBe(204);
    const form = await forum.client().get("/register");
    expect(form.text).toContain('class="g-recaptcha" data-sitekey="site-key"');
    const policy = (r: typeof form) => r.headers.get("content-security-policy") ?? "";
    expect(policy(form)).toContain("https://www.google.com/recaptcha/");
    expect(policy(form)).not.toContain("require-trusted-types-for");
    const elsewhere = await forum.client().get("/login");
    expect(policy(elsewhere)).not.toContain("google.com");
    expect(policy(elsewhere)).toContain("require-trusted-types-for 'script'");
    await setting(alice, "captcha_provider", "");
    await setting(alice, "captcha_site_key", "");
  });
  it("age gate and account approval", async () => {
    expect((await setting(alice, "minimum_age", 16)).status).toBe(204);
    expect((await setting(alice, "require_account_approval", true)).status).toBe(204);
    const c = forum.client();
    const page = (await c.get("/register")).text;
    expect(page).toContain('href="/legal/terms"');
    const reg = { username: "carol", email: "carol@example.com", password: "carol long passphrase" };
    expect((await c.form("/register", { ...reg, birth_date: "1990-01-01" })).status).toBe(422);
    expect((await c.form("/register", { ...reg, birth_date: `${new Date().getFullYear()}-01-01`, accept: 1 })).status).toBe(422);
    expect((await c.form("/register", { ...reg, birth_date: "1990-01-01", accept: 1 })).status).toBe(303);
    const post = { title: "Carol first topic here", body: "Hello everyone from Carol" };
    // An account awaiting approval is read-only (viewer.rs: silenced || !approved): refused, not validated.
    expect((await c.form(`/c/${s.FORUM}/new`, post)).status).toBe(403);
    const found = (await alice.get("/api/v1/admin/users?q=carol")).text;
    expect(found).toContain('"approved":false');
    const cid = idFrom(found, /"id":(\d+)/);
    expect((await patch({ approved: true }, `/api/v1/admin/users/${cid}`)).status).toBe(204);
    expect((await c.form(`/c/${s.FORUM}/new`, post)).status).toBe(303);
    await setting(alice, "minimum_age", 0);
    await setting(alice, "require_account_approval", false);
  });
  it("search filters", async () => {
    const byAlice = (await alice.get("/search?q=topic&author=alice")).text;
    expect(byAlice).toContain("Watched forum <mark>topic</mark>");
    expect(byAlice).not.toContain("Carol first topic");
    expect((await alice.get("/search?q=watched&in=topics")).text).toContain("<mark>Watched</mark> forum topic");
    expect((await alice.get("/search?q=watched&after=2999-01-01")).text).not.toContain('<a href="/topic');
    expect((await alice.get(`/search?q=reply&forum=${s.FORUM}`)).text).toContain('<ol class="results bf-feed">');
    expect((await guest.get("/api/v1/search?q=watched")).text).toContain('"title":"Watched forum topic"');
    expect((await guest.get(`/api/v1/topics/${WT}/state`)).text).toContain('"highest_post_number":');
  });
  it("hidden posts", async () => {
    const hid = [...(await alice.get(wtUrl)).text.matchAll(/action="\/post\/(\d+)\/hide"/g)].at(-1)![1];
    expect((await alice.form(`/post/${hid}/hide`, { hidden: 1, reason: "off topic" })).status).toBe(303);
    expect((await alice.get(wtUrl)).text).toContain("Hidden from members: off topic");
    expect((await guest.get(wtUrl)).text).not.toContain("Hidden from members");
    expect((await alice.form(`/post/${hid}/hide`, { hidden: 0 })).status).toBe(303);
  });
  it("link forums", async () => {
    expect((await alice.api("/api/v1/admin/nodes", { kind: "link", title: "Docs", parent_id: 1 })).status).toBe(422);
    expect((await alice.api("/api/v1/admin/nodes", { kind: "link", title: "Docs", parent_id: 1, url: "https://docs.example.com/" })).status).toBe(201);
    expect((await guest.get("/")).text).toContain('/go"');
  });
  it("cooking and quoting", async () => {
    const body = "Code and video:\n\n```rust\nfn main() { let x = 1; }\n```\n\nhttps://www.youtube.com/watch?v=dQw4w9WgXcQ\n";
    await alice.form(`/t/${WT}/reply`, { body });
    const tp = (await guest.get(wtUrl)).text;
    expect(tp).toContain('class="hl-');
    expect(tp).toContain('src="https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg"');
    expect(tp).not.toContain("<iframe");
    const qp = idFrom((await alice.get(wtUrl)).text, /data-quote="(\d+)"/);
    expect((await alice.get(`/api/v1/posts/${qp}/raw`)).text).toContain('"raw":"Hello watchers of this forum"');
    expect((await guest.get("/api/v1/posts/999999/raw")).status).toBe(404);
    const mjs = tp.match(/\/assets\/members\.[0-9a-f]+\.js/)![0];
    expect((await guest.get(mjs)).status).toBe(200);
  });
});

describe("OIDC", () => {
  const nextUser = (u: unknown) => fetch(`${forum.oidc}/next-user`, { method: "POST", body: JSON.stringify(u) });
  // These sign-ins start with a GET, which `auth_require_interaction` (on by default) turns into a page with a button.
  const sso = (c: Client) => c.get("/auth/oidc", { follow: true });
  beforeAll(async () => {
    await setting(alice, "auth_require_interaction", false);
  });
  it("creates and reuses an account", async () => {
    expect((await guest.get("/login")).text).toContain('action="/auth/oidc"');
    await nextUser({ sub: "ext-1", email: "dave@example.com", email_verified: true, preferred_username: "dave" });
    const d = forum.client();
    expect((await sso(d)).status).toBe(200);
    expect((await d.get("/settings")).text).toContain("d•••@e•••.com");
    const d2 = forum.client();
    expect((await sso(d2)).status).toBe(200);
    expect((await d2.get("/")).text).toContain('<strong>dave</strong></div>');
  });
  it("a sign-in begun while signed in links nothing: only Connected logins may", async () => {
    await nextUser({ sub: "ext-2", email: "someone-else@example.com", email_verified: true, preferred_username: "whoever" });
    expect((await sso(alice)).status).toBe(422);
    await nextUser({ sub: "ext-2", email: "someone-else@example.com", email_verified: true, preferred_username: "whoever" });
    const d = forum.client();
    await sso(d);
    const home = (await d.get("/")).text;
    expect(home).toContain('<strong>whoever</strong></div>');
    expect(home).not.toContain('<strong>alice</strong></div>');
  });
  it("an unverified email cannot take over an account", async () => {
    await nextUser({ sub: "ext-3", email: "carol@example.com", email_verified: false, preferred_username: "carol" });
    const d = forum.client();
    expect((await sso(d)).status).toBe(422);
    expect((await d.get("/notifications")).status).toBe(403);
  });
  it("refuses forged callbacks", async () => {
    const d = forum.client();
    await d.get("/auth/oidc");
    expect((await d.get("/auth/oidc/callback?code=x&state=forged")).status).toBe(400);
    expect((await guest.get("/auth/oidc/callback?code=x&state=y")).status).toBe(400);
    expect((await guest.get("/auth/myspace")).status).toBe(404);
  });
});
