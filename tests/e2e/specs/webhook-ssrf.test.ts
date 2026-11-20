// Webhook signing (against a local receiver) and the SSRF guard on every endpoint that fetches
// an address an admin or member typed. The strict forum runs without BF_WEBHOOK_ALLOW_LOOPBACK,
// as production does; the lax one allows loopback so a local receiver can be reached.
import { createHmac } from "node:crypto";
import { createServer as http, type IncomingMessage, type Server as HttpServer } from "node:http";
import { createServer as tcp, type Server } from "node:net";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { sleep, until, useForum } from "../lib/forum.ts";
import { seed, topic, type Seed } from "../lib/seed.ts";

const lax = useForum();
const strict = useForum({ BF_WEBHOOK_ALLOW_LOOPBACK: "0" });

type Got = { headers: IncomingMessage["headers"]; body: string };
const received: Got[] = [];
let receiver: HttpServer, receiverPort = 0;
let trap: Server, trapPort = 0, trapHits = 0;
let L: Seed, S: Seed;

const listen = (srv: Server | HttpServer) => new Promise<number>((ok) => srv.listen(0, "127.0.0.1", () => ok((srv.address() as { port: number }).port)));

beforeAll(async () => {
  receiver = http((req, res) => {
    const chunks: Buffer[] = [];
    req.on("data", (c) => chunks.push(c));
    req.on("end", () => {
      received.push({ headers: req.headers, body: Buffer.concat(chunks).toString() });
      res.writeHead(204).end();
    });
  });
  receiverPort = await listen(receiver);
  // Anything that connects here was let through the SSRF guard; it never answers.
  trap = tcp((sock) => {
    trapHits++;
    sock.destroy();
  });
  trapPort = await listen(trap);
  L = await seed(lax);
  S = await seed(strict);
});
afterAll(() => {
  receiver?.close();
  trap?.close();
});

const sign = (secret: string, t: string, body: string) => `t=${t},sha256=${createHmac("sha256", secret).update(`${t}.${body}`).digest("hex")}`;

describe("webhook signing", () => {
  let id: number, secret: string;

  it("only an administrator can create or test one", async () => {
    const url = `http://127.0.0.1:${receiverPort}/hook`;
    expect((await L.bob.api("/api/v1/admin/webhooks", { url, events: ["*"] })).status).toBe(403);
    expect((await L.guest.api("/api/v1/admin/webhooks", { url, events: ["*"] }, { headers: { "x-csrf-token": L.bob.csrf } })).status).toBe(403);
    expect((await L.bob.api("/api/v1/admin/webhooks/1/test")).status).toBe(403);
  });

  it("a created webhook returns its secret once", async () => {
    const r = await L.alice.api("/api/v1/admin/webhooks", { url: `http://127.0.0.1:${receiverPort}/hook`, events: ["*"] });
    expect(r.status).toBe(201);
    ({ id, secret } = r.json());
    expect(secret.length).toBeGreaterThanOrEqual(32);
    expect((await L.alice.get("/api/v1/admin/webhooks")).text).not.toContain(secret);
  });

  it("a test delivery carries a valid HMAC over timestamp.body", async () => {
    const r = await L.alice.api(`/api/v1/admin/webhooks/${id}/test`);
    expect(r.status).toBe(200);
    expect(r.json()).toMatchObject({ ok: true, status: 204 });
    const got = received.at(-1)!;
    const sig = String(got.headers["x-bf-signature"]);
    const t = /^t=(\d+),sha256=[0-9a-f]{64}$/.exec(sig)?.[1];
    expect(t, `bad signature header ${sig}`).toBeTruthy();
    expect(sig).toBe(sign(secret, t!, got.body));
    expect(got.headers["x-bf-timestamp"]).toBe(t);
    expect(got.headers["x-bf-event"]).toBe("webhook.test");
    expect(Math.abs(Date.now() / 1000 - Number(t))).toBeLessThan(60);
    expect(JSON.parse(got.body).event).toBe("webhook.test");
  });

  it("the signature does not verify for a tampered body, another secret or another timestamp", () => {
    const got = received.at(-1)!;
    const sig = String(got.headers["x-bf-signature"]);
    const t = String(got.headers["x-bf-timestamp"]);
    expect(sig).not.toBe(sign(secret, t, got.body + " "));
    expect(sig).not.toBe(sign("another-secret", t, got.body));
    expect(sig).not.toBe(sign(secret, String(Number(t) + 1), got.body));
  });

  it("a real event is queued, delivered and signed with the same secret", async () => {
    const before = received.length;
    await topic(L.alice, L.FORUM, "Webhook event topic");
    const hit = await until(async () => received.slice(before).find((g) => g.headers["x-bf-event"] === "topic.created"), (g) => !!g, 15_000);
    expect(hit, "topic.created was not delivered").toBeDefined();
    const sig = String(hit!.headers["x-bf-signature"]);
    const t = /^t=(\d+),/.exec(sig)![1];
    expect(sig).toBe(sign(secret, t, hit!.body));
  });

  it("a deleted webhook gets nothing more", async () => {
    expect((await L.alice.api(`/api/v1/admin/webhooks/${id}`, undefined, { method: "DELETE" })).status).toBe(204);
    await sleep(500);
    const n = received.length;
    await topic(L.alice, L.FORUM, "After the webhook was removed");
    await sleep(4000);
    expect(received.length).toBe(n);
  });
});

describe("SSRF: addresses the server must never fetch", () => {
  const targets = () => [
    `https://127.0.0.1:${trapPort}/x`,
    `https://localhost:${trapPort}/x`,
    `https://[::1]:${trapPort}/x`,
    `https://[::ffff:127.0.0.1]:${trapPort}/x`,
    `https://0.0.0.0:${trapPort}/x`,
    `https://2130706433:${trapPort}/x`, // 127.0.0.1 written as one integer
    "https://169.254.169.254/latest/meta-data/",
    "https://10.0.0.5/x",
    "https://192.168.1.10/x",
    "https://172.16.0.1/x",
  ];

  it("webhook creation takes only https without credentials", async () => {
    for (const url of [`http://127.0.0.1:${trapPort}/x`, "http://example.com/x", "ftp://example.com/x", "https://user:pw@example.com/x", "file:///etc/passwd", "gopher://127.0.0.1/x"]) {
      const r = await S.alice.api("/api/v1/admin/webhooks", { url, events: ["*"] });
      expect(r.status, url).toBe(422);
    }
  });

  it("a webhook pointed at a private or loopback address is never contacted", async () => {
    for (const url of targets()) {
      const made = await S.alice.api("/api/v1/admin/webhooks", { url, events: ["*"] });
      if (made.status !== 201) {
        expect(made.status, url).toBe(422);
        continue;
      }
      const test = await S.alice.api(`/api/v1/admin/webhooks/${made.json().id}/test`);
      expect(test.status, url).toBe(200);
      expect(test.json().ok, `${url} was delivered to`).toBe(false);
      expect(test.json().status, url).toBeNull();
    }
    expect(trapHits, "the guard let a connection through").toBe(0);
  });

  it("queued deliveries are guarded too, not just the test button", async () => {
    const made = await S.alice.api("/api/v1/admin/webhooks", { url: `https://127.0.0.1:${trapPort}/queued`, events: ["topic.created"] });
    expect(made.status).toBe(201);
    await topic(S.alice, S.FORUM, "Event for a forbidden webhook");
    await sleep(6000);
    expect(trapHits).toBe(0);
  });

  it("theme and plugin install-by-URL refuse them, and are admin-only", async () => {
    for (const path of ["/api/v1/admin/themes/install-url", "/api/v1/admin/plugins/install-url"]) {
      for (const url of [...targets(), `http://127.0.0.1:${trapPort}/x.zip`]) {
        const r = await S.alice.api(path, { url });
        expect(r.status, `${path} ${url}`).toBe(422);
      }
      expect((await S.bob.api(path, { url: "https://example.com/x.zip" })).status, path).toBe(403);
    }
    expect(trapHits).toBe(0);
  });

  it("link previews never fetch a private address, posted or not", async () => {
    for (const url of targets().slice(0, 3)) {
      await topic(S.bob, S.FORUM, `Preview of ${Math.random()}`, `A link on its own line:\n\n${url}\n`);
      expect((await S.guest.get(`/api/v1/onebox?url=${encodeURIComponent(url)}`)).status, url).toBe(404);
    }
    // An address nobody posted is not fetched at all, so the preview cannot be used as a proxy.
    expect((await S.guest.get(`/api/v1/onebox?url=${encodeURIComponent(`https://127.0.0.1:${trapPort}/never-posted`)}`)).status).toBe(404);
    await sleep(1500);
    expect(trapHits).toBe(0);
  });

  it("the comment-embed endpoint does not fetch them either", async () => {
    for (const url of targets().slice(0, 3)) {
      const r = await S.guest.get(`/embed/comments?url=${encodeURIComponent(url)}`);
      expect(r.status, url).toBeGreaterThanOrEqual(400);
    }
    expect(trapHits).toBe(0);
  });
});
