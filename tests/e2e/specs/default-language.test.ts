// The site's default language: browser-based, or one language for everyone who has not picked one.
import { beforeAll, describe, expect, it } from "vitest";
import { Client, useForum } from "../lib/forum.ts";
import { seed, setting, type Seed } from "../lib/seed.ts";

const forum = useForum();
let s: Seed, alice: Client;

const lang = async (headers: Record<string, string>) => {
  const html = (await forum.client().get("/", { headers })).text;
  return html.match(/<html[^>]*\slang="([a-z]+)"/)?.[1];
};

beforeAll(async () => {
  s = await seed(forum);
  alice = s.alice;
});

describe("default language", () => {
  it("is browser-based unless set: the visitor's browser decides, English if it asks for none of ours", async () => {
    expect(await lang({ "accept-language": "pl-PL,pl;q=0.9" })).toBe("pl");
    expect(await lang({ "accept-language": "ar" })).toBe("ar");
    expect(await lang({ "accept-language": "de, fr;q=0.8" })).toBe("en");
    expect(await lang({})).toBe("en");
  });

  it("can be one language for everyone, whatever their browser says", async () => {
    expect((await setting(alice, "default_locale", "pl")).status).toBe(204);
    expect(await lang({ "accept-language": "en-US,en;q=0.9" })).toBe("pl");
    expect(await lang({})).toBe("pl");
    // A visitor's own choice still wins.
    expect(await lang({ "accept-language": "pl", cookie: "bf_lang=en" })).toBe("en");
    expect((await setting(alice, "default_locale", "browser")).status).toBe(204);
    expect(await lang({ "accept-language": "ar" })).toBe("ar");
  });

  it("keeps a language that was saved before: English stays the fallback, not a fixed language", async () => {
    // Anything that is not one of our languages reads as browser-based, so no stored value breaks.
    expect((await setting(alice, "default_locale", "xx")).status).toBe(204);
    expect(await lang({ "accept-language": "pl" })).toBe("pl");
    expect(await lang({ "accept-language": "de" })).toBe("en");
    expect((await setting(alice, "default_locale", "browser")).status).toBe(204);
  });
});
