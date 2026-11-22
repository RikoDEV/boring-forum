// Shared by the exporters: writes the importer's JSON one entry at a time, so a large forum never
// has to fit in memory (the importer reads it the same way). Sections must be written in the order
// the importer expects: users, forums, topics, posts, then tags, groups, polls, reactions, messages.
import { createWriteStream } from "node:fs";

export class Out {
  constructor(path) {
    this.s = createWriteStream(path);
    this.sections = 0;
    this.s.write("{");
  }
  async write(text) {
    if (!this.s.write(text)) await new Promise((r) => this.s.once("drain", r));
  }
  /** `rows` is an array or an (async) iterable; returns how many entries were written. */
  async section(name, rows) {
    await this.write(`${this.sections++ ? "," : ""}"${name}":[`);
    let n = 0;
    for await (const row of rows) await this.write(`${n++ ? "," : ""}${JSON.stringify(row)}`);
    await this.write("]");
    return n;
  }
  /** A section that is one object, such as the site settings; returns how many entries it has. */
  async object(name, value) {
    await this.write(`${this.sections++ ? "," : ""}"${name}":${JSON.stringify(value)}`);
    return Object.keys(value).length;
  }
  async end() {
    await this.write("}");
    await new Promise((r) => this.s.end(r));
  }
}

/** Runs `sql(offset, limit)` in pages until a page is short; yields every row. */
export async function* pages(query, size = 5000) {
  for (let offset = 0; ; offset += size) {
    const rows = await query(offset, size);
    yield* rows;
    if (rows.length < size) return;
  }
}

/** A section that may not exist in this version of the software: a failed query gives no rows. */
export async function optional(what, run) {
  try {
    return await run();
  } catch (e) {
    console.error(`skipped ${what}: ${e.message}`);
    return [];
  }
}

export const iso = (s) => (s ? new Date(typeof s === "number" ? s * 1000 : s).toISOString() : null);
