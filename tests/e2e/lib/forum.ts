// A private forum for one test file: cloned database, own server, own mock identity provider.
import { spawn, type ChildProcess } from "node:child_process";
import { generateKeyPairSync, randomBytes } from "node:crypto";
import { mkdtempSync, readFileSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import pg from "pg";
import { afterAll, beforeAll, inject } from "vitest";
import { REPO, TEMPLATE_DB, adminUrl, dbUrl, freePort, waitHealthy } from "./env.ts";

declare module "vitest" {
  export interface ProvidedContext { bin: string }
}

const TIME_SCALE = Number(process.env.BF_TIME_SCALE_PERCENT ?? 10);
export const PASSWORD = "another long passphrase";

export class Res {
  constructor(readonly status: number, readonly text: string, readonly location: string, readonly headers: Headers) {}
  json() { return JSON.parse(this.text); }
}

export type Fields = Record<string, string | number | boolean | (string | number)[]>;
type Opts = { headers?: Record<string, string>; method?: string; follow?: boolean; noCsrf?: boolean };

/** `fetch`, tried once more when the pooled connection it picked had just been closed by the server (a
 *  refused oversized upload closes it): the request never left, so sending it again is safe. */
async function sendOnce(url: string, init: RequestInit): Promise<Response> {
  try {
    return await fetch(url, init);
  } catch (e) {
    const code = (e as { cause?: { code?: string } }).cause?.code;
    if (code !== "EPIPE" && code !== "ECONNRESET") throw e;
    return await fetch(url, init);
  }
}

/** One browser: a cookie jar plus helpers. Forms and API calls send the CSRF token by default. */
export class Client {
  readonly jar = new Map<string, string>();
  constructor(readonly base: string) {}

  get csrf() { return this.jar.get("__Host-bf_csrf") ?? ""; }

  async send(path: string, init: RequestInit & { follow?: boolean } = {}): Promise<Res> {
    let url = path.startsWith("http") ? path : this.base + path;
    let method = init.method ?? "GET";
    let body = init.body;
    for (let hops = 0; ; hops++) {
      const headers = new Headers(init.headers);
      if (this.jar.size) headers.set("cookie", [...this.jar].map(([k, v]) => `${k}=${v}`).join("; "));
      const r = await sendOnce(url, { ...init, method, body, headers, redirect: "manual" });
      for (const c of r.headers.getSetCookie()) {
        const [pair, ...attrs] = c.split(";");
        const eq = pair.indexOf("=");
        const name = pair.slice(0, eq).trim();
        const gone = attrs.some((a) => /^\s*max-age=0\s*$/i.test(a)) || pair.slice(eq + 1) === "";
        if (gone) this.jar.delete(name);
        else this.jar.set(name, pair.slice(eq + 1));
      }
      const raw = r.headers.get("location");
      const location = raw ? new URL(raw, url).href : "";
      if (init.follow && location && r.status >= 300 && r.status < 400 && hops < 10) {
        await r.arrayBuffer();
        if (r.status === 303 || r.status === 301 || r.status === 302) { method = "GET"; body = undefined; }
        url = location;
        continue;
      }
      return new Res(r.status, await r.text(), location, r.headers);
    }
  }

  get(path: string, o: Opts = {}) { return this.send(path, { headers: o.headers, follow: o.follow, method: o.method }); }

  /** POST urlencoded. Array values repeat the key. */
  form(path: string, fields: Fields = {}, o: Opts = {}) {
    const q = new URLSearchParams();
    if (!o.noCsrf && !("csrf" in fields)) q.set("csrf", this.csrf);
    for (const [k, v] of Object.entries(fields)) for (const x of Array.isArray(v) ? v : [v]) q.append(k, String(x));
    return this.send(path, { method: o.method ?? "POST", body: q, headers: { "content-type": "application/x-www-form-urlencoded", ...o.headers }, follow: o.follow });
  }

  /** JSON API call; POST by default. */
  api(path: string, body?: unknown, o: Opts = {}) {
    const headers: Record<string, string> = { ...(o.noCsrf ? {} : { "x-csrf-token": this.csrf }), ...o.headers };
    if (body !== undefined) headers["content-type"] = "application/json";
    return this.send(path, { method: o.method ?? "POST", body: body === undefined ? undefined : JSON.stringify(body), headers, follow: o.follow });
  }

  /** POST multipart. `file` values are {name, data}. */
  multipart(path: string, parts: Record<string, string | { name: string; data: Uint8Array | string }>, o: Opts = {}) {
    const fd = new FormData();
    for (const [k, v] of Object.entries(parts)) {
      if (typeof v === "string") fd.append(k, v);
      else fd.append(k, new Blob([v.data as BlobPart]), v.name);
    }
    return this.send(path, { method: "POST", body: fd, headers: { ...(o.noCsrf ? {} : { "x-csrf-token": this.csrf }), ...o.headers } });
  }

  async register(username: string, email = `${username}@example.com`, password = PASSWORD) {
    await this.get("/register");
    return this.form("/register", { username, email, password });
  }

  async login(login: string, password = PASSWORD) {
    await this.get("/login");
    return this.form("/login", { login, password });
  }
}

export class Forum {
  base = "";
  oidc = "";
  mailDir = "";
  uploadDir = "";
  db!: pg.Client;
  server?: ChildProcess;
  mock?: ChildProcess;
  dbName = "";

  client() { return new Client(this.base); }

  /** A new signed-in member. */
  async member(name: string) {
    const c = this.client();
    const r = await c.register(name);
    if (r.status !== 303) throw new Error(`register ${name}: ${r.status} ${r.text.slice(0, 200)}`);
    return c;
  }

  sql(text: string, values?: unknown[]) { return this.db.query(text, values as never[]); }

  mails(): string[] {
    return readdirSync(this.mailDir).filter((f) => f.endsWith(".eml")).sort().map((f) => readFileSync(join(this.mailDir, f), "utf8"));
  }

  /** Waits for a mail matching the test, returning it. */
  async mail(test: (m: string) => boolean, tries = 150) {
    for (let i = 0; i < tries; i++) {
      const hit = this.mails().reverse().find(test);
      if (hit) return hit;
      await sleep(100);
    }
    throw new Error("mail did not arrive");
  }
}

export const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Waits out a server timer of `seconds`, which test runs shorten. */
export const settle = (seconds: number) => sleep((seconds * 1000 * TIME_SCALE) / 100);

/** Retries until `get` satisfies `ok`, and returns the last value. */
export async function until<T>(get: () => Promise<T>, ok: (v: T) => boolean, ms = 10_000): Promise<T> {
  const end = Date.now() + ms;
  for (;;) {
    const v = await get();
    if (ok(v) || Date.now() > end) return v;
    await sleep(50);
  }
}

/** Declares the forum for this test file. */
export function useForum(extraEnv: Record<string, string> = {}) {
  const f = new Forum();
  beforeAll(async () => {
    f.dbName = `bf_e2e_${randomBytes(6).toString("hex")}`;
    const admin = new pg.Client({ connectionString: adminUrl() });
    await admin.connect();
    await admin.query(`CREATE DATABASE ${f.dbName} TEMPLATE ${TEMPLATE_DB}`);
    await admin.end();
    f.db = new pg.Client({ connectionString: dbUrl(f.dbName) });
    await f.db.connect();

    const [port, mockPort] = [await freePort(), await freePort()];
    f.base = `http://127.0.0.1:${port}`;
    f.oidc = `http://127.0.0.1:${mockPort}`;
    f.mailDir = mkdtempSync(join(tmpdir(), "bf-mail-"));
    f.uploadDir = mkdtempSync(join(tmpdir(), "bf-up-"));
    f.mock = spawn(process.execPath, [join(REPO, "tests/support/mock-oidc.mjs"), String(mockPort)], { stdio: "ignore" });
    const apple = generateKeyPairSync("ec", { namedCurve: "P-256" }).privateKey.export({ type: "pkcs8", format: "pem" }) as string;
    f.server = spawn(inject("bin"), [], {
      stdio: "ignore",
      env: {
        ...process.env,
        BF_BIND: `127.0.0.1:${port}`,
        BF_DATABASE_URL: dbUrl(f.dbName),
        BF_ADMIN_DIR: join(REPO, "web/admin/build"),
        BF_MAIL_DIR: f.mailDir,
        BF_UPLOAD_DIR: f.uploadDir,
        BF_TIME_SCALE_PERCENT: String(TIME_SCALE),
        BF_RATE_LIMIT_PER_MIN: "1000000",
        BF_REGISTER_LIMIT_PER_HOUR: "1000000",
        BF_GEO_HEADER: "X-Test-Country",
        BF_ASN_HEADER: "X-Test-ASN",
        BF_BOUNCE_TOKEN: "0123456789abcdef0123456789abcdef",
        BF_VAPID_PRIVATE: "yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw",
        BF_TICK_SECONDS: "1",
        BF_DELETE_GRACE_DAYS: "0",
        BF_REPLY_DOMAIN: "mail.example",
        BF_INBOUND_TOKEN: "abcdef0123456789abcdef0123456789",
        BF_WEBHOOK_ALLOW_LOOPBACK: "1",
        BF_SSO_URL: `${f.oidc}/sso`,
        BF_SSO_SECRET: "0123456789abcdef0123456789abcdef",
        BF_CAPTCHA_VERIFY_URL: `${f.oidc}/siteverify`,
        BF_OAUTH_APPLE_ID: "com.example.forum",
        BF_OAUTH_APPLE_TEAM_ID: "TEAM",
        BF_OAUTH_APPLE_KEY_ID: "KID",
        BF_OAUTH_APPLE_BASE: f.oidc,
        BF_OAUTH_APPLE_KEY: apple,
        BF_OIDC_NAME: "Test SSO",
        BF_OIDC_CLIENT_ID: "test-client",
        BF_OIDC_CLIENT_SECRET: "test-secret",
        BF_OIDC_AUTHORIZE_URL: `${f.oidc}/authorize`,
        BF_OIDC_TOKEN_URL: `${f.oidc}/token`,
        BF_OIDC_USERINFO_URL: `${f.oidc}/userinfo`,
        RUST_LOG: "error",
        NO_COLOR: "1",
        ...extraEnv,
      },
    });
    await waitHealthy(f.base);
  });
  afterAll(async () => {
    f.server?.kill();
    f.mock?.kill();
    await f.db?.end();
    const admin = new pg.Client({ connectionString: adminUrl() });
    await admin.connect();
    await admin.query(`DROP DATABASE IF EXISTS ${f.dbName} WITH (FORCE)`);
    await admin.end();
  });
  return f;
}

/** Numbers in the first match of a pattern, for pulling ids out of pages. */
export function idFrom(text: string, re: RegExp): string {
  const m = text.match(re);
  if (!m) throw new Error(`no match for ${re}`);
  return m[1];
}
