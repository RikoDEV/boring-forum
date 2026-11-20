// Which links in posts carry nofollow: by setting, by the author's trust level, by site.
import { beforeAll, describe, expect, it } from "vitest";
import { Client, useForum } from "../lib/forum.ts";
import { seed, setting, topic, type Seed } from "../lib/seed.ts";

const forum = useForum();
let s: Seed, alice: Client, bob: Client, guest: Client;
let url = "";

const rel = (html: string, text: string) => {
  const m = html.match(new RegExp(`<a [^>]*rel="([^"]*)"[^>]*>${text}</a>`));
  return m?.[1];
};
const STOCK = "nofollow ugc noopener";

beforeAll(async () => {
  s = await seed(forum);
  ({ alice, bob, guest } = s);
  url = (await topic(bob, s.FORUM, "A post with several links", "[theirs](https://other.example/page) [mine](https://friend.example/page) [sub](https://blog.friend.example/a) [lookalike](https://notfriend.example/a) [home](/latest)")).url;
});

describe("nofollow on links in posts", () => {
  it("is on every link by default, even a link to this site", async () => {
    const html = (await guest.get(url)).text;
    for (const t of ["theirs", "mine", "sub", "lookalike", "home"]) expect(rel(html, t), t).toBe(STOCK);
  });

  it("can be limited to links to other sites", async () => {
    expect((await setting(alice, "nofollow_links", "external")).status).toBe(204);
    const html = (await guest.get(url)).text;
    expect(rel(html, "home")).toBe("noopener");
    expect(rel(html, "theirs")).toBe(STOCK);
    expect((await setting(alice, "nofollow_links", "none")).status).toBe(204);
    const none = (await guest.get(url)).text;
    for (const t of ["theirs", "mine", "home"]) expect(rel(none, t), t).toBe("noopener");
    expect((await setting(alice, "nofollow_links", "all")).status).toBe(204);
  });

  it("can leave out sites, and their subdomains, but not lookalikes", async () => {
    expect((await setting(alice, "nofollow_exempt_domains", "https://www.friend.example/ , nonsense")).status).toBe(204);
    const html = (await guest.get(url)).text;
    expect(rel(html, "mine")).toBe("noopener");
    expect(rel(html, "sub")).toBe("noopener");
    expect(rel(html, "lookalike")).toBe(STOCK);
    expect(rel(html, "theirs")).toBe(STOCK);
    expect((await setting(alice, "nofollow_exempt_domains", "")).status).toBe(204);
    expect(rel((await guest.get(url)).text, "mine")).toBe(STOCK);
  });

  it("can leave out members from a trust level up", async () => {
    // Bob is a new member (level 0); level 5 is above every level there is.
    expect((await setting(alice, "nofollow_exempt_trust_level", 1)).status).toBe(204);
    expect(rel((await guest.get(url)).text, "theirs")).toBe(STOCK);
    expect((await setting(alice, "nofollow_exempt_trust_level", 0)).status).toBe(204);
    expect(rel((await guest.get(url)).text, "theirs")).toBe("noopener");
    expect((await setting(alice, "nofollow_exempt_trust_level", 5)).status).toBe(204);
    expect(rel((await guest.get(url)).text, "theirs")).toBe(STOCK);
  });
});
