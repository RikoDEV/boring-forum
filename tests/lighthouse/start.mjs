// Boots the forum for Lighthouse CI the way tests/e2e does (a migrated Postgres, the server
// binary, a little content), prints READY and stays up until it is killed.
//
//   BF_BIN             the server binary            (default target/debug/boring-forum)
//   BF_DATABASE_URL    an empty, reachable database (default the compose db's `forum`)
//   LH_PORT            port to listen on            (default 8099)
import { spawn } from "node:child_process";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const port = process.env.LH_PORT ?? "8099";
const base = `http://127.0.0.1:${port}`;
const bin = process.env.BF_BIN ?? join(repo, "target/debug", process.platform === "win32" ? "boring-forum.exe" : "boring-forum");

const server = spawn(bin, [], {
  stdio: "inherit",
  env: {
    ...process.env,
    BF_BIND: `127.0.0.1:${port}`,
    BF_DATABASE_URL: process.env.BF_DATABASE_URL ?? "postgres://forum:forum@127.0.0.1:5432/forum?sslmode=disable",
    BF_UPLOAD_DIR: mkdtempSync(join(tmpdir(), "bf-lh-up-")),
    BF_MAIL_DIR: mkdtempSync(join(tmpdir(), "bf-lh-mail-")),
    BF_ADMIN_DIR: join(repo, "web/admin/build"),
    BF_RATE_LIMIT_PER_MIN: "1000000",
    RUST_LOG: process.env.LH_LOG ?? "error",
  },
});
const stop = () => server.kill();
process.on("SIGTERM", stop);
process.on("SIGINT", stop);
process.on("exit", stop);
server.on("exit", (code) => process.exit(code ?? 1));

for (let i = 0; ; i++) {
  try {
    if ((await fetch(`${base}/healthz`)).ok) break;
  } catch {}
  if (i > 600) throw new Error("the server did not start");
  await new Promise((r) => setTimeout(r, 100));
}

// A browser's cookie jar, as little as this needs.
const jar = new Map();
async function send(path, init = {}) {
  const headers = new Headers(init.headers);
  if (jar.size) headers.set("cookie", [...jar].map(([k, v]) => `${k}=${v}`).join("; "));
  const r = await fetch(base + path, { ...init, headers, redirect: "manual" });
  for (const c of r.headers.getSetCookie()) {
    const [pair] = c.split(";");
    const eq = pair.indexOf("=");
    jar.set(pair.slice(0, eq).trim(), pair.slice(eq + 1));
  }
  return r;
}
const csrf = () => jar.get("__Host-bf_csrf") ?? "";
const form = (path, fields) =>
  send(path, {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({ csrf: csrf(), ...fields }),
  });
const api = (path, body) =>
  send(path, { method: "POST", headers: { "content-type": "application/json", "x-csrf-token": csrf() }, body: JSON.stringify(body) });
const must = async (r, what) => {
  if (r.status >= 400) throw new Error(`${what}: ${r.status} ${(await r.text()).slice(0, 300)}`);
  return r;
};

// The first account is the administrator.
await send("/register");
await must(await form("/register", { username: "alice", email: "alice@example.com", password: "another long passphrase" }), "register");
await must(await api("/api/v1/admin/nodes", { kind: "category", title: "General" }), "category");
await must(await api("/api/v1/admin/nodes", { kind: "forum", title: "Introductions", parent_id: 1, description: "Say **hi**" }), "forum");
await must(
  await form("/c/2/new", {
    title: "Welcome to the forum",
    body: "A first post, long enough to read as a real one, with a [link](https://example.com/) and some **formatting**.",
  }),
  "topic",
);
console.log("READY");
