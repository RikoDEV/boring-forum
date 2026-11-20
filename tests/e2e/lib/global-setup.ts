// Builds the server once and migrates a template database that every test file clones.
import { execFileSync, spawn } from "node:child_process";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import pg from "pg";
import { REPO, TEMPLATE_DB, adminUrl, dbUrl, freePort, waitHealthy } from "./env.ts";

export default async function setup({ provide }: { provide: (k: string, v: string) => void }) {
  // BF_E2E_BIN: a server binary built elsewhere (the CI browser job downloads one), so a run
  // does not compile it again.
  let bin = process.env.BF_E2E_BIN ?? "";
  if (!bin) {
    execFileSync("cargo", ["build", "-q", "-p", "bf-server", "--bin", "boring-forum"], { cwd: REPO, stdio: "inherit" });
    const meta = JSON.parse(execFileSync("cargo", ["metadata", "--format-version", "1", "--no-deps"], { cwd: REPO, encoding: "utf8", maxBuffer: 1 << 26 }));
    bin = join(meta.target_directory, "debug", process.platform === "win32" ? "boring-forum.exe" : "boring-forum");
  }
  provide("bin", bin);

  const db = new pg.Client({ connectionString: adminUrl() });
  await db.connect();
  const old = await db.query("SELECT datname FROM pg_database WHERE datname LIKE 'bf_e2e_%'");
  for (const { datname } of old.rows) await db.query(`DROP DATABASE "${datname}" WITH (FORCE)`);
  await db.query(`DROP DATABASE IF EXISTS ${TEMPLATE_DB} WITH (FORCE)`);
  await db.query(`CREATE DATABASE ${TEMPLATE_DB}`);

  // The server applies migrations when it starts.
  const port = await freePort();
  const server = spawn(bin, [], {
    env: { ...process.env, BF_BIND: `127.0.0.1:${port}`, BF_DATABASE_URL: dbUrl(TEMPLATE_DB), BF_UPLOAD_DIR: mkdtempSync(join(tmpdir(), "bf-")), BF_MAIL_DIR: mkdtempSync(join(tmpdir(), "bf-")), RUST_LOG: "error" },
    stdio: "ignore",
  });
  await waitHealthy(`http://127.0.0.1:${port}`);
  await new Promise<void>((ok) => { server.once("exit", () => ok()); server.kill(); });
  await db.query(`SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '${TEMPLATE_DB}'`);
  await db.end();
}
