// imgproxy: pictures from other sites go through the admin's own imgproxy, signed; this site's
// pictures and GIF providers' media do not.
import { beforeAll, describe, expect, it } from "vitest";
import { Client, useForum } from "../lib/forum.ts";
import { seed, setting, topic, type Seed } from "../lib/seed.ts";

const forum = useForum();
let s: Seed, alice: Client, bob: Client, guest: Client;
let url = "";

const b64 = (text: string) => Buffer.from(text).toString("base64url");
const PIC = "https://pics.example.org/cat.png";
const GIF = "https://pics.example.org/dance.gif";
const KLIPY = "https://static.klipy.com/ii/a/b/c/party.webp";
const imgs = (html: string) => [...html.matchAll(/<img[^>]*\ssrc="([^"]+)"/g)].map((m) => m[1]).filter((u) => u.startsWith("http"));

beforeAll(async () => {
  s = await seed(forum);
  ({ alice, bob, guest } = s);
  const t = await topic(bob, s.FORUM, "A post with pictures", `Look:\n\n![cat](${PIC})\n\n![dance](${GIF})\n\n![party](${KLIPY})`);
  url = t.url;
});

describe("imgproxy", () => {
  it("is off until set up: pictures come straight from their sites", async () => {
    expect(imgs((await guest.get(url)).text)).toEqual(expect.arrayContaining([PIC, GIF, KLIPY]));
    // Switched on without a working address, key and salt: still nothing changes.
    expect((await setting(alice, "imgproxy_enabled", true)).status).toBe(204);
    expect(imgs((await guest.get(url)).text)).toContain(PIC);
    for (const [k, v] of [["imgproxy_url", "https://img.example.com"], ["imgproxy_key", "not hex"], ["imgproxy_salt", "68656c6c6f"]] as const)
      expect((await setting(alice, k, v)).status).toBe(204);
    expect(imgs((await guest.get(url)).text)).toContain(PIC);
  });

  it("sends other sites' pictures through it, signed, and leaves GIFs and GIF providers alone", async () => {
    expect((await setting(alice, "imgproxy_key", "736563726574")).status).toBe(204);
    for (const c of [guest, bob, alice]) {
      const found = imgs((await c.get(url)).text);
      const swapped = found.find((u) => u.startsWith("https://img.example.com/"))!;
      expect(swapped).toBeDefined();
      const [sig, ...rest] = swapped.replace("https://img.example.com/", "").split("/");
      expect(sig).toMatch(/^[A-Za-z0-9_-]{43}$/);
      expect(rest.join("/")).toBe(`rs:fit:1280:0:0/${b64(PIC)}`);
      expect(found).not.toContain(PIC);
      expect(found).toEqual(expect.arrayContaining([GIF, KLIPY]));
    }
  });

  it("follows the settings: width, format, GIFs, and switching it off", async () => {
    expect((await setting(alice, "imgproxy_width", 800)).status).toBe(204);
    expect((await setting(alice, "imgproxy_format", "webp")).status).toBe(204);
    expect((await setting(alice, "imgproxy_gifs", true)).status).toBe(204);
    const found = imgs((await guest.get(url)).text);
    expect(found.find((u) => u.endsWith(`${b64(PIC)}.webp`))).toContain("/rs:fit:800:0:0/");
    expect(found.some((u) => u.endsWith(`${b64(GIF)}.webp`))).toBe(true);
    expect(found).toContain(KLIPY);
    expect((await setting(alice, "imgproxy_enabled", false)).status).toBe(204);
    expect(imgs((await guest.get(url)).text)).toEqual(expect.arrayContaining([PIC, GIF, KLIPY]));
  });

  it("changes a page's validator with its settings, so nothing stale is kept", async () => {
    expect((await setting(alice, "imgproxy_enabled", true)).status).toBe(204);
    const on = await guest.get(url);
    const tag = on.headers.get("etag");
    if (tag) {
      expect(tag).toMatch(/-p[0-9a-f]{8}"$/);
      expect((await guest.get(url, { headers: { "if-none-match": tag.replace(/-p[0-9a-f]{8}"$/, '-pdeadbeef"') } })).status).toBe(200);
    }
    expect((await setting(alice, "imgproxy_enabled", false)).status).toBe(204);
  });
});
