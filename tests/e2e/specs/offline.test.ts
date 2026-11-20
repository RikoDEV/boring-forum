// The offline worker saves only what a guest could read, so these check what it would be given.
import { beforeAll, describe, expect, it } from "vitest";
import { useForum } from "../lib/forum.ts";

const forum = useForum();

describe("offline reading", () => {
  let sw: string;
  beforeAll(async () => {
    const res = await forum.client().get("/sw.js");
    expect(res.status).toBe(200);
    sw = res.text;
  });
  it("serves a worker with its strings filled in", () => {
    expect(sw).not.toContain("__BF_STRINGS__");
    expect(sw).toContain("You are offline");
    expect(() => new Function(sw)).not.toThrow();
  });
  it("saves pages without cookies", () => {
    expect(sw).toContain('credentials: "omit"');
  });
  it("gives a guest a page that names no one and has no session", async () => {
    const guest = await forum.client().get("/");
    expect(guest.status).toBe(200);
    expect(guest.text).not.toContain("__Host-bf_session");
  });
});
