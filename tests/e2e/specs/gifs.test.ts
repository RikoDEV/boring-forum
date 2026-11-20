// The GIF picker's settings reach the editor, the page policy opens only the chosen provider, and
// a GIF is an ordinary picture in a post.
import { beforeAll, describe, expect, it } from "vitest";
import { Client, useForum } from "../lib/forum.ts";
import { seed, setting, topic, type Seed } from "../lib/seed.ts";

const forum = useForum();
let s: Seed, alice: Client, bob: Client, guest: Client;

const csp = (r: { headers: Headers }) => r.headers.get("content-security-policy") ?? "";
const meta = (html: string) => html.match(/<meta name="bf-gif"[^>]*>/)?.[0];

beforeAll(async () => {
  s = await seed(forum);
  ({ alice, bob, guest } = s);
});

describe("GIFs", () => {
  it("are off until enabled with a key, and the page says nothing of them", async () => {
    const r = await bob.get("/");
    expect(meta(r.text)).toBeUndefined();
    expect(csp(r)).not.toContain("klipy");
    // On, but without a key: still nothing.
    expect((await setting(alice, "gifs_enabled", true)).status).toBe(204);
    expect(meta((await bob.get("/")).text)).toBeUndefined();
  });

  it("give the editor the provider's settings, and the policy only that provider's hosts", async () => {
    for (const [k, v] of [["gif_api_key", "test-key_1"], ["gif_format", "gif"], ["gif_content_filter", "medium"], ["gif_country", "pl"], ["gif_limit_scroll", true], ["gif_max_results", 96]] as const)
      expect((await setting(alice, k, v)).status).toBe(204);
    const r = await bob.get("/");
    const m = meta(r.text)!;
    expect(m).toContain('content="test-key_1"');
    expect(m).toContain('data-provider="klipy"');
    expect(m).toContain('data-format="gif"');
    expect(m).toContain('data-filter="medium"');
    expect(m).toContain('data-country="PL"');
    expect(m).toContain('data-limit="96"');
    expect(csp(r)).toContain("connect-src 'self'");
    expect(csp(r)).toContain("https://api.klipy.com");
    expect(csp(r)).not.toContain("giphy");

    expect((await setting(alice, "gif_provider", "giphy")).status).toBe(204);
    const g = await bob.get("/");
    expect(meta(g.text)).toContain('data-provider="giphy"');
    expect(csp(g)).toContain("https://api.giphy.com");
    expect(csp(g)).not.toContain("klipy");
  });

  it("are for those who can write: guests get no picker", async () => {
    expect(meta((await guest.get("/")).text)).toBeUndefined();
  });

  it("refuse a key that is not plain text and a provider that does not exist", async () => {
    expect((await setting(alice, "gif_api_key", 'x"><script>')).status).toBe(204);
    expect(meta((await bob.get("/")).text)).toBeUndefined();
    expect((await setting(alice, "gif_api_key", "test-key_1")).status).toBe(204);
    expect((await setting(alice, "gif_provider", "nonsense")).status).toBe(204);
    const r = await bob.get("/");
    expect(meta(r.text)).toBeUndefined();
    expect(csp(r)).not.toContain("klipy");
  });

  it("are an ordinary picture in a post", async () => {
    const url = "https://static.klipy.com/ii/abc/def/ghi/cat.webp";
    const t = await topic(bob, s.FORUM, "A post with a GIF", `Look at this:\n\n![Cat typing](${url})`);
    const page = await guest.get(t.url);
    expect(page.text).toContain(`src="${url}"`);
    expect(page.text).toContain('alt="Cat typing"');
  });
});
