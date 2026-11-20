// Common starting data and small helpers for spec files.
import { createHmac } from "node:crypto";
import { Client, Forum, idFrom } from "./forum.ts";

export type Seed = { alice: Client; bob: Client; guest: Client; FORUM: number; OFF: number };

/** Admin alice, member bob, a guest, and the forums "Introductions" (2) and "Offtopic" (3). */
export async function seed(f: Forum): Promise<Seed> {
  const alice = f.client();
  await alice.register("alice", "alice@example.com", "correct horse battery");
  const bob = await f.member("bob");
  await alice.api("/api/v1/admin/nodes", { kind: "category", title: "General" });
  await alice.api("/api/v1/admin/nodes", { kind: "forum", title: "Introductions", parent_id: 1, description: "Say **hi**" });
  await alice.api("/api/v1/admin/nodes", { kind: "forum", title: "Offtopic", parent_id: 1 });
  return { alice, bob, guest: f.client(), FORUM: 2, OFF: 3 };
}

/** Creates a topic and returns its id and path. */
export async function topic(c: Client, forum: number, title: string, body = "A first post long enough to pass") {
  const r = await c.form(`/c/${forum}/new`, { title, body });
  if (r.status !== 303) throw new Error(`topic ${title}: ${r.status} ${r.text.slice(0, 300)}`);
  return { id: idFrom(r.location, /\/t\/[^/]+\/(\d+)/), url: new URL(r.location).pathname };
}

export const setting = (admin: Client, key: string, value: unknown) =>
  admin.api(`/api/v1/admin/settings/${key}`, { value }, { method: "PUT" });

/** The newest mail to `to` containing `path`: its first matching link. */
export async function mailLink(f: Forum, to: string, path: string) {
  const m = await f.mail((x) => x.includes(`To: ${to}`) && x.includes(path));
  return m.match(new RegExp(`https?://[^\\s"<>]*${path}[^\\s"<>]*`))![0];
}

/** TOTP code for a base32 secret, `offset` seconds from now. */
export function totp(secret: string, offset = 0) {
  const A = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
  const bits = [...secret].map((c) => A.indexOf(c).toString(2).padStart(5, "0")).join("");
  const key = Buffer.from(bits.match(/.{8}/g)!.map((b) => parseInt(b, 2)));
  const ctr = Buffer.alloc(8);
  ctr.writeBigUInt64BE(BigInt(Math.floor((Date.now() / 1000 + offset) / 30)));
  const h = createHmac("sha1", key).update(ctr).digest();
  return String((h.readUInt32BE(h[19] & 15) & 0x7fffffff) % 1e6).padStart(6, "0");
}
