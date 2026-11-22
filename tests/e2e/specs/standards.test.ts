// Discovery files, feeds, probes, API semantics, passwords, usernames, the EU DSA
// flows and ActivityPub federation against a mock remote server that signs its requests.
import { createHash, createSign, generateKeyPairSync } from "node:crypto";
import { createServer, type Server } from "node:http";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { freePort } from "../lib/env.ts";
import { Client, until, useForum } from "../lib/forum.ts";

const forum = useForum();
let admin: Client, bob: Client, guest: Client;
let FORUM: number;

const setting = (key: string, value: unknown) => admin.api(`/api/v1/admin/settings/${key}`, { value }, { method: "PUT" });

describe("setup", () => {
  it("admin, a public forum and a topic", async () => {
    admin = await forum.member("standards_admin");
    bob = await forum.member("standards_bob");
    guest = forum.client();
    expect((await admin.api("/api/v1/admin/nodes", { kind: "category", title: "Main" })).status).toBe(201);
    const r = await admin.api("/api/v1/admin/nodes", { kind: "forum", title: "Open Talk", parent_id: 1 });
    FORUM = Number(r.text.match(/\d+/)![0]);
    const t = await admin.form(`/c/${FORUM}/new`, { title: "Standards topic here", body: "The first post of the standards topic." });
    expect(t.status).toBe(303);
  });
});

describe("discovery and feeds", () => {
  it("security.txt only with a contact, with a rolling Expires", async () => {
    expect((await guest.get("/.well-known/security.txt")).status).toBe(404);
    expect((await setting("security_contact", "mailto:security@example.com")).status).toBe(204);
    const r = await guest.get("/.well-known/security.txt");
    expect(r.status).toBe(200);
    expect(r.text).toMatch(/^Contact: mailto:security@example.com\nExpires: \d{4}-/);
  });
  it("ads.txt is the configured text, and absent when empty", async () => {
    expect((await guest.get("/ads.txt")).status).toBe(404);
    const lines = "google.com, pub-0000000000000000, DIRECT, f08c47fec0942fa0\r\n\r\n# a note  \r\nexample.com, 1, RESELLER  \r\n";
    expect((await setting("ads_txt", lines)).status).toBe(204);
    const r = await guest.get("/ads.txt");
    expect(r.status).toBe(200);
    expect(r.headers.get("content-type")).toContain("text/plain");
    expect(r.text).toBe("google.com, pub-0000000000000000, DIRECT, f08c47fec0942fa0\n\n# a note\nexample.com, 1, RESELLER\n");
    expect((await setting("ads_txt", "")).status).toBe(204);
    expect((await guest.get("/ads.txt")).status).toBe(404);
  });
  it("the mail page can tell how mail is sent, and only admins may ask", async () => {
    expect((await guest.get("/api/v1/admin/mail/status")).status).toBe(403);
    const r = await admin.get("/api/v1/admin/mail/status");
    expect(r.status).toBe(200);
    // The test server writes mail to files; a missing or broken sender would say `error` or `log`.
    expect(r.json().kind).toBe("dir");
  });
  it("change-password and passkey endpoints", async () => {
    const r = await guest.get("/.well-known/change-password");
    expect(r.status).toBe(303);
    expect(r.location).toBe(`${forum.base}/settings#s-password`);
    expect((await guest.get("/.well-known/passkey-endpoints")).json().enroll).toContain("/settings");
  });
  it("Atom and JSON Feed beside RSS", async () => {
    const atom = await guest.get("/latest.atom");
    expect(atom.headers.get("content-type")).toContain("application/atom+xml");
    expect(atom.text).toContain("<feed xmlns=\"http://www.w3.org/2005/Atom\">");
    expect(atom.text).toContain("Standards topic here");
    const json = await guest.get(`/c/${FORUM}/feed.json`);
    expect(json.headers.get("content-type")).toContain("application/feed+json");
    expect(json.json().items[0].title).toBe("Standards topic here");
    expect((await guest.get("/")).text).toContain('type="application/atom+xml"');
  });
  it("oEmbed for a topic, discovered from its page", async () => {
    const page = await guest.get("/t/standards-topic-here/1");
    expect(page.text).toContain("application/json+oembed");
    const r = await guest.get(`/oembed?url=${encodeURIComponent(`${forum.base}/t/standards-topic-here/1`)}`);
    expect(r.status).toBe(200);
    expect(r.json()).toMatchObject({ version: "1.0", type: "rich", title: "Standards topic here" });
    expect(r.json().html).not.toContain("<script");
    expect((await guest.get("/oembed?url=https%3A%2F%2Felsewhere.example%2Ft%2Fx%2F1")).status).toBe(404);
  });
  it("llms.txt and AI crawler rules", async () => {
    expect((await guest.get("/llms.txt")).text).toContain("Open Talk");
    expect((await guest.get("/robots.txt")).text).not.toContain("GPTBot");
    await setting("ai_crawlers", "block");
    expect((await guest.get("/robots.txt")).text).toContain("User-agent: GPTBot");
  });
});

describe("assets", () => {
  it("every island chunk an entry loads is served, immutable", async () => {
    const page = (await guest.get("/")).text;
    const entries = [...page.matchAll(/import\("(\/assets\/(?:islands|members)\.[0-9a-f]+\.js)"\)/g)].map((m) => m[1]);
    expect(entries).toHaveLength(2);
    const chunks = new Set<string>();
    for (const e of entries) {
      const js = (await guest.get(e)).text;
      for (const m of js.matchAll(/["'`]\.\/([gm]-[\w-]+\.js)["'`]/g)) chunks.add(m[1]);
    }
    expect(chunks.size).toBeGreaterThan(3);
    for (const c of chunks) {
      const r = await guest.get(`/assets/${c}`);
      expect(r.status, c).toBe(200);
      expect(r.headers.get("content-type")).toContain("text/javascript");
      expect(r.headers.get("cache-control")).toContain("immutable");
    }
  });
  it("the stylesheets are bundled, layered and minified", async () => {
    const page = (await guest.get("/")).text;
    expect(page).toContain("<style>@layer tokens, base, app, a11y;");
    const href = page.match(/href="(\/assets\/app\.[0-9a-f]+\.css)"/)![1];
    const css = (await guest.get(href)).text;
    expect(css.startsWith("@layer app{")).toBe(true);
    expect(css).not.toContain("@import");
    expect(css).not.toContain("/*");
  });
});

describe("probes and headers", () => {
  it("livez, readyz, and metrics for loopback only without a token", async () => {
    expect((await guest.get("/livez")).status).toBe(200);
    expect((await guest.get("/readyz")).json()).toEqual({ config: true, db: true, draining: false, migrations: true, status: "ok" });
    const m = await guest.get("/metrics");
    expect(m.status).toBe(200);
    expect(m.text).toContain("bf_http_requests_total");
  });
  it("Reporting API endpoint beside report-uri", async () => {
    const r = await guest.get("/");
    expect(r.headers.get("reporting-endpoints")).toBe('csp="/api/v1/csp-report"');
    expect(r.headers.get("content-security-policy")).toContain("report-to csp");
    const rep = await guest.send("/api/v1/csp-report", {
      method: "POST",
      headers: { "content-type": "application/reports+json" },
      body: JSON.stringify([{ type: "csp-violation", url: forum.base, body: { effectiveDirective: "script-src-elem" } }]),
    });
    expect(rep.status).toBe(204);
  });
});

describe("API semantics", () => {
  it("an Idempotency-Key replays the first response", async () => {
    const key = { "idempotency-key": "draft-save-1" };
    const body = { body: "a draft that should only be saved once" };
    const first = await bob.api(`/api/v1/drafts?context=new-topic:${FORUM}`, body, { method: "PUT", headers: key });
    const again = await bob.api(`/api/v1/drafts?context=new-topic:${FORUM}`, body, { method: "PUT", headers: key });
    expect(again.status).toBe(first.status);
    expect(again.headers.get("idempotent-replayed")).toBe("true");
    const other = await bob.api(`/api/v1/drafts?context=new-topic:${FORUM}`, { body: "something else" }, { method: "PUT", headers: key });
    expect(other.status).toBe(422);
  });
  it("JSON reads have an ETag and answer If-None-Match with 304", async () => {
    const r = await guest.get("/api/v1/settings");
    const tag = r.headers.get("etag");
    expect(tag).toMatch(/^W\/"[0-9a-f]{32}"$/);
    expect((await guest.get("/api/v1/settings", { headers: { "if-none-match": tag! } })).status).toBe(304);
  });
});

describe("passwords and usernames", () => {
  it("common passwords are refused", async () => {
    const c = forum.client();
    const r = await c.register("pw_tester", "pw_tester@example.com", "Password123");
    expect(r.status).toBe(422);
    expect(r.text).toContain("most common");
  });
  it("non-Latin usernames only when the setting is on, and never confusable", async () => {
    expect((await forum.client().register("Michał", "michal@example.com")).status).toBe(422);
    await setting("unicode_usernames", true);
    expect((await forum.client().register("Michał", "michal@example.com")).status).toBe(303);
    const mixed = await forum.client().register("pаypal", "mixed@example.com");
    expect(mixed.text).toContain("one writing system");
    expect((await forum.client().register("popo", "popo@example.com")).status).toBe(303);
    await until(() => forum.sql("SELECT count(*)::int AS n FROM users WHERE username_skeleton IS NULL"), (r) => r.rows[0].n === 0);
    // All Cyrillic, so one script; only the skeleton gives it away.
    const lookalike = await forum.client().register("роро", "lookalike@example.com");
    expect(lookalike.text).toContain("looks too much like");
  });
});

describe("EU Digital Services Act", () => {
  let postId: number;
  it("anyone can send a notice about illegal content", async () => {
    const reply = await bob.form("/t/1/reply", { body: "A reply that someone will report." });
    expect(reply.status).toBe(303);
    postId = (await forum.sql("SELECT id FROM posts WHERE item_id = 1 AND post_number = 2")).rows[0].id;
    const g = forum.client();
    await g.get("/legal/notice");
    const r = await g.form("/legal/notice", {
      url: `${forum.base}/t/standards-topic-here/1#post-2`,
      category: "illegal_or_harmful_speech",
      explanation: "This reply defames a named person.",
      notifier_name: "Nora Notifier",
      notifier_email: "nora@example.com",
      good_faith: "1",
    });
    expect(r.status).toBe(200);
    expect(r.text).toContain("Your notice was received");
    await forum.mail((m) => m.includes("nora@example.com") && m.includes("We received your notice"));
  });
  it("staff decide it; the author gets a statement and can appeal", async () => {
    const review = await admin.get("/review/dsa");
    expect(review.text).toContain("defames a named person");
    const id = (await forum.sql("SELECT id FROM legal_notices ORDER BY id DESC LIMIT 1")).rows[0].id;
    expect((await admin.form(`/review/dsa/notice/${id}`, { decision: "remove", reason: "Defamation under national law." })).status).toBe(303);
    expect((await forum.sql("SELECT state FROM posts WHERE id = $1", [postId])).rows[0].state).toBe("hidden");
    const s = (await forum.sql("SELECT id FROM moderation_statements WHERE target_id = $1", [postId])).rows[0].id;
    const page = await bob.get(`/statements/${s}`);
    expect(page.text).toContain("Defamation under national law.");
    expect((await guest.get(`/statements/${s}`)).status).not.toBe(200);
    expect((await bob.form(`/statements/${s}/appeal`, { text: "It is true, so not defamation." })).status).toBe(303);
    // The moderator who decided may not decide the appeal.
    expect((await admin.form(`/review/dsa/appeal/${s}`, { outcome: "reversed" })).status).toBe(422);
    await forum.mail((m) => m.includes("nora@example.com") && m.includes("Defamation under national law."));
  });
  it("the transparency report counts it", async () => {
    const t = await guest.get("/transparency");
    expect(t.status).toBe(200);
    const figure = (label: string) => t.text.match(new RegExp(`<dt>${label}</dt>\\s*<dd>([^<]*)</dd>`))?.[1];
    expect(figure("Received")).toBe("1");
    expect(figure("Led to action")).toBe("1");
    expect(figure("Appeals")).toBe("1");
  });
});

describe("ActivityPub", () => {
  const { publicKey, privateKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
  const pem = publicKey.export({ type: "spki", format: "pem" }) as string;
  const received: any[] = [];
  let remote: Server;
  let R = "";
  let carol = "";

  beforeAll(async () => {
    const port = await freePort();
    R = `http://127.0.0.1:${port}`;
    carol = `${R}/users/carol`;
    remote = createServer((req, res) => {
      let body = "";
      req.on("data", (c) => (body += c));
      req.on("end", () => {
        if (req.method === "GET" && req.url === "/users/carol") {
          res.setHeader("content-type", "application/activity+json");
          res.end(JSON.stringify({
            "@context": ["https://www.w3.org/ns/activitystreams"], id: carol, type: "Person", preferredUsername: "carol",
            inbox: `${R}/inbox`, publicKey: { id: `${carol}#main-key`, owner: carol, publicKeyPem: pem },
          }));
        } else if (req.method === "POST" && req.url === "/inbox") {
          received.push(JSON.parse(body));
          res.statusCode = 202;
          res.end();
        } else {
          res.statusCode = 404;
          res.end();
        }
      });
    });
    await new Promise<void>((ok) => remote.listen(port, "127.0.0.1", ok));
  });
  afterAll(() => remote?.close());

  /** POSTs an activity signed like Mastodon does. */
  async function deliver(path: string, activity: unknown, sign = true) {
    const body = JSON.stringify(activity);
    const url = new URL(forum.base + path);
    const date = new Date().toUTCString();
    const digest = `SHA-256=${createHash("sha256").update(body).digest("base64")}`;
    const headers: Record<string, string> = { "content-type": "application/activity+json", date, digest };
    if (sign) {
      const str = [`(request-target): post ${url.pathname}`, `host: ${url.host}`, `date: ${date}`, `digest: ${digest}`, "content-type: application/activity+json"].join("\n");
      const sig = createSign("RSA-SHA256").update(str).sign(privateKey, "base64");
      headers.signature = `keyId="${carol}#main-key",algorithm="rsa-sha256",headers="(request-target) host date digest content-type",signature="${sig}"`;
    }
    return guest.send(path, { method: "POST", body, headers });
  }

  it("is off until enabled", async () => {
    expect((await guest.get("/.well-known/webfinger?resource=acct:standards_admin@x")).status).toBe(404);
    expect((await setting("federation_enabled", true)).status).toBe(204);
  });
  it("WebFinger, NodeInfo and actors", async () => {
    const host = new URL(forum.base).host;
    const wf = await guest.get(`/.well-known/webfinger?resource=acct:standards_admin@${host}`);
    expect(wf.status).toBe(200);
    const self = wf.json().links.find((l: any) => l.rel === "self").href;
    expect(self).toMatch(/\/ap\/u\/\d+$/);
    const actor = await guest.get(new URL(self).pathname);
    expect(actor.headers.get("content-type")).toContain("application/activity+json");
    expect(actor.json()).toMatchObject({ type: "Person", preferredUsername: "standards_admin" });
    expect(actor.json().publicKey.publicKeyPem).toContain("BEGIN PUBLIC KEY");
    const group = await guest.get(`/.well-known/webfinger?resource=acct:forum.open-talk@${host}`);
    expect(group.status).toBe(200);
    const ni = await guest.get("/nodeinfo/2.1");
    expect(ni.json().protocols).toEqual(["activitypub"]);
    const neg = await guest.get("/u/standards_admin", { headers: { accept: "application/activity+json" } });
    expect(neg.location).toBe(self);
  });
  it("refuses unsigned or tampered activities", async () => {
    expect((await deliver("/ap/inbox", { id: `${R}/a/1`, type: "Follow", actor: carol, object: "x" }, false)).status).toBe(401);
  });
  it("accepts a follow and answers with Accept", async () => {
    const uid = (await forum.sql("SELECT id FROM users WHERE username = 'standards_admin'")).rows[0].id;
    const me = `${forum.base}/ap/u/${uid}`;
    expect((await deliver(`/ap/u/${uid}/inbox`, { id: `${R}/follows/1`, type: "Follow", actor: carol, object: me })).status).toBe(202);
    const accept = await until(async () => received.find((a) => a.type === "Accept"), (a) => !!a);
    expect(accept?.object?.id).toBe(`${R}/follows/1`);
  });
  it("a reply from the fediverse becomes a post, and local posts go out", async () => {
    const first = (await forum.sql("SELECT id FROM posts WHERE item_id = 1 AND post_number = 1")).rows[0].id;
    const note = {
      id: `${R}/notes/1`, type: "Note", attributedTo: carol, inReplyTo: `${forum.base}/ap/p/${first}`,
      to: ["https://www.w3.org/ns/activitystreams#Public"], content: "<p>Hello from <a href=\"https://elsewhere.example/x\">elsewhere</a>!</p><script>bad()</script>",
    };
    expect((await deliver("/ap/inbox", { id: `${R}/creates/1`, type: "Create", actor: carol, object: note })).status).toBe(202);
    const topic = await until(() => guest.get("/t/standards-topic-here/1"), (r) => r.text.includes("Hello from"));
    expect(topic.text).toContain("carol@127.0.0.1");
    expect(topic.text).not.toContain("bad()");
    // The follower hears about the admin's next post.
    await admin.form("/t/1/reply", { body: "A local reply that should federate." });
    const create = await until(async () => received.find((a) => a.type === "Create"), (a) => !!a);
    expect(create?.object?.content).toContain("A local reply that should federate.");
    expect(create?.object?.inReplyTo).toBeTruthy();
  });
  it("likes from the fediverse count as reactions", async () => {
    const first = (await forum.sql("SELECT id FROM posts WHERE item_id = 1 AND post_number = 1")).rows[0].id;
    await deliver("/ap/inbox", { id: `${R}/likes/1`, type: "Like", actor: carol, object: `${forum.base}/ap/p/${first}` });
    const n = await until(() => forum.sql("SELECT like_count FROM posts WHERE id = $1", [first]), (r) => r.rows[0].like_count === 1);
    expect(n.rows[0].like_count).toBe(1);
  });
});
