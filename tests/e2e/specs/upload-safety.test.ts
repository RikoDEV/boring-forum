// What the upload pipeline refuses, and how it serves what it keeps.
import { beforeAll, describe, expect, it } from "vitest";
import { Client, useForum } from "../lib/forum.ts";
import { seed, setting, topic, type Seed } from "../lib/seed.ts";

const forum = useForum();
let s: Seed, alice: Client, bob: Client, carol: Client, guest: Client;

const PNG = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==", "base64");
const nav = { "sec-fetch-mode": "navigate", "sec-fetch-dest": "document", "sec-fetch-site": "none" };
const up = (c: Client, name: string, data: string | Uint8Array) => c.multipart("/api/v1/uploads", { file: { name, data } });
const stored = async (r: Awaited<ReturnType<typeof up>>) => {
  expect(r.status, r.text.slice(0, 200)).toBe(201);
  return r.json() as { url: string; markdown: string };
};
/** Text that is unique, so the content-addressed file is not already there from an earlier test. */
const uniq = (tag: string, kb = 0) => `${tag} ${Math.random()}\n` + "x".repeat(kb * 1024);

beforeAll(async () => {
  s = await seed(forum);
  ({ alice, bob, guest } = s);
  carol = await forum.member("carol");
});

describe("types that are refused", () => {
  const refused: [string, string, string | Uint8Array][] = [
    ["a Windows executable", "setup.exe", "MZ\x90\x00\x03\x00\x00\x00 not really a program"],
    ["an ELF binary", "run.bin", "\x7fELF\x02\x01\x01\x00 binary"],
    ["an HTML page named .html", "page.html", "<html><script>alert(1)</script></html>"],
    ["an HTML page renamed .png", "photo.png", "<html><script>alert(1)</script></html>"],
    ["an SVG with a handler", "x.svg", '<svg xmlns="http://www.w3.org/2000/svg" onload="alert(1)"/>'],
    ["an SVG renamed .png", "x.png", '<svg xmlns="http://www.w3.org/2000/svg" onload="alert(1)"/>'],
    ["a script named .js", "a.js", "alert(document.cookie)"],
    ["a PHP shell named .php", "shell.php", "<?php system($_GET['c']); ?>"],
    ["a binary blob named .txt", "blob.txt", new Uint8Array([0x00, 0xff, 0xfe, 0x01, 0x02])],
    ["plain text with no .txt name and no magic", "notes", "plain text but nothing says so"],
    ["an empty file", "empty.txt", ""],
  ];
  for (const [label, name, data] of refused) {
    it(label, async () => {
      const r = await up(bob, name, data);
      expect(r.status, `${label} was accepted: ${r.text.slice(0, 120)}`).toBe(422);
    });
  }

  it("a request with no file at all", async () => {
    const r = await bob.multipart("/api/v1/uploads", { note: "no file here" });
    expect(r.status).toBe(422);
  });

  it("a guest, a missing CSRF token and a cross-origin request", async () => {
    const file = { name: "a.png", data: PNG };
    expect((await guest.multipart("/api/v1/uploads", { file }, { headers: { "x-csrf-token": bob.csrf } })).status).toBe(403);
    expect((await bob.multipart("/api/v1/uploads", { file }, { noCsrf: true })).status).toBe(400);
    expect((await bob.multipart("/api/v1/uploads", { file }, { headers: { origin: "https://evil.example" } })).status).toBe(403);
  });
});

describe("the name never decides the type: the bytes do", () => {
  it("PNG bytes named .html are kept as a PNG, never served as HTML", async () => {
    const f = await stored(await up(bob, "evil.html", PNG));
    expect(f.url).toMatch(/\/uploads\/[0-9a-f]{64}\.png/);
    const r = await guest.get(f.url.split("#")[0]);
    expect(r.headers.get("content-type")).toBe("image/png");
    expect(r.headers.get("x-content-type-options")).toBe("nosniff");
  });

  it("HTML in a .txt is just text: served as an attachment with nosniff and a sandbox", async () => {
    const f = await stored(await up(bob, "page.txt", uniq("<html><script>alert(1)</script></html>")));
    const r = await guest.get(f.url, { headers: nav });
    expect(r.status).toBe(200);
    expect(r.headers.get("content-type")).toMatch(/^text\/plain/);
    expect(r.headers.get("x-content-type-options")).toBe("nosniff");
    expect(r.headers.get("content-disposition")).toMatch(/^attachment/);
    expect(r.headers.get("content-security-policy")).toContain("sandbox");
  });

  it("an HTML document appended to PNG bytes is refused, or served as an image only", async () => {
    const poly = Buffer.concat([PNG, Buffer.from(`<html><script>alert(${Math.random()})</script></html>`)]);
    const r = await up(bob, "poly.png", poly);
    if (r.status === 201) {
      const got = await guest.get((r.json() as { url: string }).url.split("#")[0]);
      expect(got.headers.get("content-type")).toBe("image/png");
      expect(got.headers.get("x-content-type-options")).toBe("nosniff");
    } else {
      expect(r.status).toBe(422);
    }
  });

  it("a ZIP and a PDF come back as attachments, with nosniff", async () => {
    const pdf = await stored(await up(bob, "doc.pdf", `%PDF-1.4\n% ${Math.random()}\n`));
    const zip = await stored(await up(bob, "pack.zip", `PK\x03\x04${Math.random()}`));
    for (const f of [pdf, zip]) {
      const r = await guest.get(f.url, { headers: nav });
      expect(r.status).toBe(200);
      expect(r.headers.get("x-content-type-options")).toBe("nosniff");
      expect(r.headers.get("content-disposition")).toMatch(/^attachment/);
    }
  });

  it("a hostile file name cannot break out of Content-Disposition or inject a header", async () => {
    const f = await stored(await up(bob, `..\\..\\ev"il<script>.txt`, uniq("hostile name")));
    const r = await guest.get(f.url, { headers: nav });
    const cd = r.headers.get("content-disposition") ?? "";
    expect(cd).toMatch(/^attachment/);
    expect(cd).not.toMatch(/[\r\n"<>]|\.\.[\\/]/);
    expect(r.headers.get("x-injected")).toBeNull();
  });

  it("only known extensions are served, and nothing outside the store", async () => {
    for (const p of [`/uploads/${"a".repeat(64)}.html`, `/uploads/${"a".repeat(64)}.svg`, `/uploads/${"A".repeat(64)}.png`, "/uploads/../Cargo.toml", "/uploads/%2e%2e%2f%2e%2e%2fetc%2fpasswd", "/uploads/short.png"]) {
      expect((await guest.get(p)).status, p).toBe(404);
    }
  });
});

describe("size and quota", () => {
  it("a file over the site's limit is refused, one just under it is kept", async () => {
    expect((await setting(alice, "max_upload_kb", 64)).status).toBe(204);
    try {
      // The server may answer and close before the whole body is sent, which a client sees as a dropped
      // connection: that is a refusal too.
      const refused = (name: string, kb: number) =>
        up(bob, name, uniq("too big", kb)).then(
          (r) => r.status,
          () => 413,
        );
      expect([413, 422]).toContain(await refused("big.txt", 65));
      expect([413, 422]).toContain(await refused("huge.txt", 2000));
      // The refused request may have left a pooled connection the server closed: one retry is enough.
      const ok = uniq("fits", 60);
      const kept = await up(bob, "ok.txt", ok).catch(() => up(bob, "ok.txt", ok));
      expect(kept.status).toBe(201);
    } finally {
      // Whatever happened above, the next tests get the usual limit back.
      expect((await setting(alice, "max_upload_kb", 4096)).status).toBe(204);
    }
  });

  it("a member's daily allowance runs out, and nobody else's does", async () => {
    expect((await setting(alice, "upload_daily_mb", 1)).status).toBe(204);
    expect((await up(bob, "q1.txt", uniq("quota one", 400))).status).toBe(201);
    expect((await up(bob, "q2.txt", uniq("quota two", 400))).status).toBe(201);
    const over = await up(bob, "q3.txt", uniq("quota three", 400));
    expect(over.status).toBe(422);
    expect(over.text).toContain("today's upload limit");
    // Re-sending bytes already stored still counts against the allowance.
    expect((await up(bob, "q2.txt", uniq("quota two", 400))).status).toBe(422);
    expect((await up(carol, "c1.txt", uniq("carol", 400))).status).toBe(201);
    expect((await setting(alice, "upload_daily_mb", 50)).status).toBe(204);
  });
});

describe("files in posts stay safe to open", () => {
  it("a post that links an uploaded text file serves it only as an attachment", async () => {
    const f = await stored(await up(alice, "linked.txt", uniq("<img src=x onerror=alert(1)>")));
    await topic(alice, s.FORUM, "A topic with a file", `File: ${f.markdown}`);
    const r = await guest.get(f.url, { headers: nav });
    expect(r.headers.get("content-disposition")).toMatch(/^attachment/);
    expect(r.headers.get("x-content-type-options")).toBe("nosniff");
  });
});
