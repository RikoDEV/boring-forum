// Writes a FluxBB 1.5 (or PunBB 1.4, which has the same tables) forum to the importer's JSON format:
//   npm i mysql2
//   FLUXBB_DB="mysql://user:pass@host/dbname" node tools/importers/export-fluxbb.mjs out.json
//   boring-forum import out.json
// Reads only. Set FLUXBB_PREFIX if the tables have a prefix (FluxBB's default is none, a common choice is `fluxbb_`).
// Exports members, categories (as category forums) with their forums, topics (views, sticky, closed), posts, and custom
// groups. FluxBB has no polls, private messages, tags or reactions in core.
// Written without a real install to test against; these are the assumptions:
//  - Passwords: FluxBB keeps sha1, salted sha1 or sha256 (PunBB: sha1), which the importer does not check, so every member
//    resets the password at first sign-in.
//  - Members: everyone in the table except the guest (id 1) and unverified accounts (group 0) with an email address.
//  - Who can read what: a forum is exported when members (group 4) may read it: its `forum_perms` row for the group if
//    there is one, else the group's `g_read_board`. Guests (group 3) are decided the same way and give the forum's
//    `public`. Forums that are redirect links are left out, with the forums and topics members cannot read.
//  - Avatars are not exported: FluxBB stores them as `img/avatars/<id>.<gif|jpg|png>` and the database does not say which.
//  - Posts: FluxBB's BBCode (`[quote=Name]`, `[code]`, `[list]` with `[*]`, `[img=alt]`, `[h]`, `[color]`/`[colour]`, `[del]`,
//    `[ins]`) is converted to Markdown here. Nested lists lose their nesting; smileys stay as the text they were typed as.
//  - Groups: custom groups only (id above 4); a member is in the group set as their main group.
import { createRequire } from "node:module";
import { Out, iso, optional, pages } from "./lib.mjs";
const require = createRequire(`${process.cwd()}/`);
const mysql = require("mysql2/promise");
const db = await mysql.createConnection(process.env.FLUXBB_DB);
const p = process.env.FLUXBB_PREFIX ?? "";
const q = async (sql, args = []) => (await db.query(sql, args))[0];
const out = new Out(process.argv[2]);
const counts = {};

// ---- post text ------------------------------------------------------------------------------------------------------
const COLORS = { red: "red", orange: "orange", yellow: "yellow", green: "green", blue: "blue", purple: "purple", violet: "purple", gray: "gray", grey: "gray" };
const unquote = (s) => (s ?? "").replace(/^['"]|['"]$/g, "");

/** FluxBB BBCode as Markdown. The text is stored as typed (no HTML entities), so nothing is decoded. */
function toMarkdown(source) {
  const held = [];
  let t = source.replace(/\r\n?/g, "\n");
  // Code is set aside first, so nothing below touches what is inside it.
  t = t.replace(/\[code\]([\s\S]*?)\[\/code\]/gi, (_, body) => {
    held.push(`\n\`\`\`\n${body.replace(/^\n|\n$/g, "")}\n\`\`\`\n`);
    return `\u0000${held.length - 1}\u0000`;
  });
  // Quotes, innermost first: [quote=Name] (or ["Name"], or quotes) and plain [quote].
  const inner = /\[quote(?:=('[^']*'|"[^"]*"|[^\]]*))?\]((?:(?!\[quote)[\s\S])*?)\[\/quote\]/i;
  for (let guard = 0; inner.test(t) && guard < 200; guard++) {
    t = t.replace(inner, (_, name, body) => {
      const lines = body.trim().split("\n").map((l) => `> ${l}`.trimEnd());
      return `\n${name ? `> **${unquote(name.trim())}:**\n` : ""}${lines.join("\n")}\n\n`;
    });
  }
  // Lists: `[list=1|a|*]` with `[*]item[/*]` (older boards leave the item open).
  t = t.replace(/\[list(?:=([^\]]*))?\]([\s\S]*?)\[\/list\]/gi, (_, kind, body) => {
    const numbered = kind && kind.trim() !== "*";
    const closed = [...body.matchAll(/\[\*\]([\s\S]*?)\[\/\*\]/g)].map((m) => m[1]);
    const items = (closed.length ? closed : body.split(/\[\*\]/).slice(1)).map((i) => i.trim());
    let n = 0;
    return `\n${items.map((i) => `${numbered ? `${++n}.` : "-"} ${i}`).join("\n")}\n\n`;
  });
  for (const [tag, mark] of [["b", "**"], ["strong", "**"], ["i", "*"], ["em", "*"], ["u", "++"], ["ins", "++"], ["del", "~~"], ["s", "~~"]]) {
    t = t.replace(new RegExp(`\\[${tag}\\]([\\s\\S]*?)\\[\\/${tag}\\]`, "gi"), `${mark}$1${mark}`);
  }
  t = t
    .replace(/\[h\]([\s\S]*?)\[\/h\]/gi, "\n#### $1\n")
    .replace(/\[colou?r=([^\]]+)\]([\s\S]*?)\[\/colou?r\]/gi, (_, c, body) => (COLORS[c.toLowerCase()] ? `[${body}]{${COLORS[c.toLowerCase()]}}` : body))
    .replace(/\[url\]([\s\S]*?)\[\/url\]/gi, "$1")
    .replace(/\[url=([^\]]+)\]([\s\S]*?)\[\/url\]/gi, (_, u, text) => `[${text}](${unquote(u)})`)
    .replace(/\[email\]([\s\S]*?)\[\/email\]/gi, "[$1](mailto:$1)")
    .replace(/\[email=([^\]]+)\]([\s\S]*?)\[\/email\]/gi, "[$2](mailto:$1)")
    .replace(/\[img=([^\]]*)\]([\s\S]*?)\[\/img\]/gi, (_, alt, url) => `![${alt.replace(/[[\]]/g, " ")}](${url})`)
    .replace(/\[img\]([\s\S]*?)\[\/img\]/gi, "![]($1)");
  t = t.replace(/\u0000(\d+)\u0000/g, (_, i) => held[Number(i)]);
  // FluxBB shows every line break: outside code a single newline becomes a hard break.
  t = t
    .split(/(\n```\n[\s\S]*?\n```\n)/)
    .map((part, i) => (i % 2 ? part : part.replace(/([^\n])\n(?!\n)/g, "$1  \n")))
    .join("");
  return t.replace(/\n{3,}/g, "\n\n").trim();
}

// ---- who can read what: a group's default, with a row per forum where it differs --------------------------------------
const GUESTS = 3;
const MEMBERS = 4;
const groups = new Map((await q(`SELECT g_id, g_title, g_read_board FROM ${p}groups`)).map((g) => [g.g_id, g]));
const perms = await optional("permissions", () => q(`SELECT group_id, forum_id, read_forum FROM ${p}forum_perms`));
const canRead = (gid, forumId) => {
  const rule = perms.find((r) => r.group_id === gid && r.forum_id === forumId);
  if (rule) return rule.read_forum === 1;
  return groups.get(gid)?.g_read_board === 1;
};
const catRows = await q(`SELECT id, cat_name, disp_position FROM ${p}categories ORDER BY disp_position, id`);
const forumRows = (await q(`SELECT id, cat_id, forum_name, disp_position, redirect_url FROM ${p}forums ORDER BY disp_position, id`)).filter((f) => !f.redirect_url && canRead(MEMBERS, f.id));
const shownIds = new Set(forumRows.map((f) => f.id));
const shownCats = catRows.filter((c) => forumRows.some((f) => f.cat_id === c.id));
const CAT = 1_000_000;

// ---- members --------------------------------------------------------------------------------------------------------
const users = await q(`SELECT id, group_id, username, email, registered FROM ${p}users WHERE id > 1 AND group_id <> 0 AND email <> ''`);
const known = new Set(users.map((u) => u.id));
// When each member was last on the old site (unix time or a date); a table or column this version lacks gives none.
const seen = new Map((await optional("last visits", () => q(`SELECT id, last_visit AS t FROM ${p}users`))).map((r) => [r.id, r.t]));
counts.members = await out.section("users", users.map((u) => ({ id: u.id, username: u.username, email: u.email, password_hash: null, joined_at: iso(u.registered), last_seen_at: iso(seen.get(u.id) || null) })));

// ---- forums, topics, posts ------------------------------------------------------------------------------------------
counts.forums = await out.section("forums", [
  ...shownCats.map((c) => ({ id: CAT + c.id, parent_id: null, title: c.cat_name, kind: "category", public: true, position: c.disp_position })),
  ...forumRows.map((f) => ({ id: f.id, parent_id: CAT + f.cat_id, title: f.forum_name, kind: "forum", public: canRead(GUESTS, f.id), position: f.disp_position })),
]);
// A moved topic leaves a stub with `moved_to` set in its old forum; the real one is in the new forum.
const topics = (await q(`SELECT id, forum_id, subject, num_views, sticky, closed FROM ${p}topics WHERE moved_to IS NULL`)).filter((t) => shownIds.has(t.forum_id));
const shown = new Set(topics.map((t) => t.id));
counts.topics = await out.section("topics", topics.map((t) => ({ id: t.id, forum_id: t.forum_id, title: t.subject, url: `/viewtopic.php?id=${t.id}`, views: t.num_views, pinned: t.sticky === 1, locked: t.closed === 1 })));

async function* posts() {
  const rows = pages((offset, size) => q(`SELECT id, topic_id, poster_id, message, posted FROM ${p}posts ORDER BY topic_id, posted, id LIMIT ? OFFSET ?`, [size, offset]));
  for await (const x of rows) {
    if (shown.has(x.topic_id)) yield { id: x.id, topic_id: x.topic_id, author_id: known.has(x.poster_id) ? x.poster_id : null, body: toMarkdown(x.message), format: "markdown", created_at: iso(x.posted) };
  }
}
counts.posts = await out.section("posts", posts());

// ---- groups ---------------------------------------------------------------------------------------------------------
counts.groups = await out.section(
  "groups",
  [...groups.values()].filter((g) => g.g_id > 4).map((g) => ({ id: g.g_id, name: g.g_title, members: users.filter((u) => u.group_id === g.g_id).map((u) => u.id) })),
);

await out.end();
await db.end();
console.log(`Wrote ${Object.entries(counts).map(([k, n]) => `${n} ${k}`).join(", ")}.`);
