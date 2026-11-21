// Login lockout and rate limiting: an attacker is shut out, the account's owner is not.
// The server believes X-Forwarded-For from a trusted proxy, so one test process can act as
// several addresses (BF_TRUSTED_PROXIES lists the loopback peer).
import { describe, expect, it } from "vitest";
import { Client, PASSWORD, useForum } from "../lib/forum.ts";

const forum = useForum({ BF_TRUSTED_PROXIES: "127.0.0.1" });
const limited = useForum({ BF_TRUSTED_PROXIES: "127.0.0.1", BF_RATE_LIMIT_PER_MIN: "40" });

const SESSION = "__Host-bf_session";
const from = (ip: string) => ({ "x-forwarded-for": ip });

/** One sign-in attempt from `ip` by a browser that has no session. */
async function attempt(f: { client(): Client }, ip: string, login: string, password: string) {
  const c = f.client();
  await c.get("/login", { headers: from(ip) });
  const r = await c.form("/login", { login, password }, { headers: from(ip) });
  return { r, c, signedIn: c.jar.has(SESSION), locked: r.text.includes("Too many failed attempts") };
}

async function signUp(f: { client(): Client }, ip: string, name: string) {
  const c = f.client();
  await c.get("/register", { headers: from(ip) });
  const r = await c.form("/register", { username: name, email: `${name}@example.com`, password: PASSWORD }, { headers: from(ip) });
  expect(r.status).toBe(303);
}

describe("per-address lockout", () => {
  const OWNER = "198.51.100.20";
  const ATTACKER = "203.0.113.7";

  it("registers the account to defend", async () => {
    await signUp(forum, OWNER, "lockvictim");
  });

  it("a wrong password is refused without locking at first", async () => {
    const a = await attempt(forum, ATTACKER, "lockvictim", "wrong password one");
    expect(a.signedIn).toBe(false);
    expect(a.locked).toBe(false);
    expect(a.r.text).toContain("Wrong username or password");
  });

  it("the fifth failure from one address locks that address out, even with the right password", async () => {
    for (let i = 2; i <= 5; i++) {
      const a = await attempt(forum, ATTACKER, "lockvictim", `wrong password ${i}`);
      expect(a.signedIn).toBe(false);
    }
    const right = await attempt(forum, ATTACKER, "lockvictim", PASSWORD);
    expect(right.signedIn, "the locked address got a session").toBe(false);
    expect(right.locked).toBe(true);
  });

  it("the owner, from another address, still signs in", async () => {
    const owner = await attempt(forum, OWNER, "lockvictim", PASSWORD);
    expect(owner.r.status).toBe(303);
    expect(owner.signedIn).toBe(true);
  });

  it("the lock reads the same for a name that does not exist (accounts cannot be enumerated)", async () => {
    const ghost = "203.0.113.8";
    let last = await attempt(forum, ghost, "nobody-by-this-name", "wrong password");
    for (let i = 0; i < 5; i++) last = await attempt(forum, ghost, "nobody-by-this-name", "wrong password");
    expect(last.locked).toBe(true);
    expect(last.signedIn).toBe(false);
  });

  it("the attacker's lock does not touch other accounts or addresses", async () => {
    await signUp(forum, "198.51.100.30", "lockother");
    expect((await attempt(forum, "198.51.100.30", "lockother", PASSWORD)).signedIn).toBe(true);
  });

  it("a forged left-hand X-Forwarded-For does not hide the attacker's real address", async () => {
    // Read from the right: the address nearest to us is the one that counts. (The owner's own
    // successful sign-in above cleared the failures on record, by design, so lock it again.)
    for (let i = 1; i <= 5; i++) await attempt(forum, ATTACKER, "lockvictim", `wrong again ${i}`);
    const chained = { "x-forwarded-for": "198.51.100.20, 203.0.113.7" };
    const c = forum.client();
    await c.get("/login", { headers: chained });
    const r = await c.form("/login", { login: "lockvictim", password: PASSWORD }, { headers: chained });
    expect(c.jar.has(SESSION), "the locked address signed in behind a spoofed left-hand address").toBe(false);
    expect(r.text).toContain("Too many failed attempts");
  });

  it("an address trying many accounts is refused outright after 50 failures", async () => {
    const sprayer = "203.0.113.50";
    for (let i = 0; i < 50; i++) await attempt(forum, sprayer, `spray-${i}`, "wrong password");
    const c = forum.client();
    await c.get("/login", { headers: from(sprayer) });
    const r = await c.form("/login", { login: "lockvictim", password: PASSWORD }, { headers: from(sprayer) });
    expect(r.status).toBe(429);
    expect(c.jar.has(SESSION)).toBe(false);
    // Everybody else is unaffected.
    expect((await attempt(forum, OWNER, "lockvictim", PASSWORD)).signedIn).toBe(true);
    // Fifty logins that must each be checked slowly can take more than the default 30 s on a shared runner.
  }, 180_000);
});

describe("account-wide lock from many addresses", () => {
  it("30 failures from different addresses lock the account for strangers, not for the owner at a known address", async () => {
    await signUp(forum, "198.51.100.99", "lockwide");
    for (let i = 0; i < 30; i++) await attempt(forum, `192.0.2.${i + 1}`, "lockwide", "wrong password");
    const row = await forum.sql("SELECT locked_until > now() AS locked FROM users WHERE username_lower = 'lockwide'");
    expect(row.rows[0].locked).toBe(true);
    // An address the account never signed in from is refused, a correct password included.
    const stranger = await attempt(forum, "198.51.100.150", "lockwide", PASSWORD);
    expect(stranger.signedIn).toBe(false);
    expect(stranger.locked).toBe(true);
    // The lock is what many addresses together can do to someone, so it does not shut the owner out of
    // the address they registered from.
    const owner = await attempt(forum, "198.51.100.99", "lockwide", PASSWORD);
    expect(owner.signedIn).toBe(true);
  });
});

describe("request rate limit", () => {
  it("an address over its limit gets 429 with Retry-After; other addresses are not affected", async () => {
    const c = limited.client();
    const flood = from("203.0.113.200");
    let blocked: Awaited<ReturnType<Client["get"]>> | undefined;
    for (let i = 0; i < 80 && !blocked; i++) {
      const r = await c.get("/login", { headers: flood });
      if (r.status === 429) blocked = r;
    }
    expect(blocked, "80 requests in a row were never limited").toBeDefined();
    expect(blocked!.headers.get("retry-after")).toMatch(/^\d+$/);
    expect(blocked!.headers.get("ratelimit-policy")).toBeTruthy();
    expect((await limited.client().get("/login", { headers: from("198.51.100.77") })).status).toBe(200);
    // Probes are never limited, so an orchestrator is not taken out of rotation.
    expect((await c.get("/healthz", { headers: flood })).status).toBe(200);
  });

  it("inventing a forwarded address on the left does not dodge the limit", async () => {
    const c = limited.client();
    let blocked = false;
    for (let i = 0; i < 80 && !blocked; i++) {
      const r = await c.get("/login", { headers: { "x-forwarded-for": `10.9.${i % 250}.1, 203.0.113.201` } });
      blocked = r.status === 429;
    }
    expect(blocked, "rotating the left-hand address defeated the rate limit").toBe(true);
  });
});
