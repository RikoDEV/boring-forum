import { createServer } from "node:net";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const REPO = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");
export const TEMPLATE_DB = "bf_template";
const HOST = process.env.BF_TEST_DB_HOST ?? "127.0.0.1";

export const dbUrl = (name: string) => `postgres://forum:forum@${HOST}:5432/${name}?sslmode=disable`;
export const adminUrl = () => `postgres://forum:forum@${HOST}:5432/forum`;

export function freePort(): Promise<number> {
  return new Promise((ok, fail) => {
    const s = createServer();
    s.once("error", fail);
    s.listen(0, "127.0.0.1", () => {
      const { port } = s.address() as { port: number };
      s.close(() => ok(port));
    });
  });
}

export async function waitHealthy(base: string, path = "/healthz") {
  for (let i = 0; i < 300; i++) {
    try {
      if ((await fetch(base + path)).ok) return;
    } catch {}
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error(`${base} did not start`);
}
