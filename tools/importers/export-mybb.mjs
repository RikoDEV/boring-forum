// Writes a MyBB 1.8 forum to the importer's JSON format:
//   npm i mysql2
//   MYBB_DB="mysql://user:pass@host/dbname" node tools/importers/export-mybb.mjs out.json
//   BF_IMPORT_FILES=/path/to/mybb boring-forum import out.json
// Reads only. Set MYBB_PREFIX if the tables do not use mybb_. Give the importer the MyBB folder (the one with
// `uploads/`) as BF_IMPORT_FILES: attachments and uploaded avatars are copied from there, and the pictures in
// posts show. Without it they are skipped.
// Exports members and their avatars, forums (in the board's order, with MyBB's permission inheritance applied: those
// guests and members cannot read are left out, and so are their topics), topics (views, sticky, closed), posts,
// attachments, custom groups, polls (who voted what, whether voters are shown) and private messages. MyBB has
// no tags or reactions in core.
// Posts: MyBB's BBCode (quotes with author, lists, colours, attachments) is converted to Markdown here.
// Passwords: MyBB's `md5(md5(salt) . md5(password))` is the same scheme as Invision 3 and is carried over as
// `ips3$<salt>$<hash>`; it is checked at the member's next sign-in, when it is replaced.
import { createRequire } from "node:module";
import { Out, iso, optional, pages } from "./lib.mjs";
const require = createRequire(`${process.cwd()}/`);
const mysql = require("mysql2/promise");
const db = await mysql.createConnection(process.env.MYBB_DB);
const p = process.env.MYBB_PREFIX ?? "mybb_";
const q = async (sql, args = []) => (await db.query(sql, args))[0];
const out = new Out(process.argv[2]);
const counts = {};

// ---- post text ------------------------------------------------------------------------------------------------------
const COLORS = { red: "red", orange: "orange", yellow: "yellow", green: "green", blue: "blue", purple: "purple", violet: "purple", gray: "gray", grey: "gray" };
const IMAGE = /\.(png|jpe?g|gif|webp)$/i;
const unquote = (s) => (s ?? "").replace(/^['"]|['"]$/g, "");

/** MyBB BBCode as Markdown. Each `[attachment=<id>]` becomes a link to that file (`upload://mb<id>`, which the importer
 *  swaps for the stored copy); the post's files not placed in the text follow it, as MyBB lists them. */
function toMarkdown(source, attachments = []) {
  const byId = new Map(attachments.map((a) => [String(a.aid), a]));
  const used = new Set();
  const link = (a) => `${IMAGE.test(a.filename) ? "!" : ""}[${a.filename.replace(/[[\]]/g, " ")}](upload://mb${a.aid})`;
  const held = [];
  let t = source.replace(/\r\n?/g, "\n");
  // Code is set aside first, so nothing below touches what is inside it.
  t = t.replace(/\[(code|php)\]([\s\S]*?)\[\/\1\]/gi, (_, kind, body) => {
    held.push(`\n\`\`\`\n${body.replace(/^\n|\n$/g, "")}\n\`\`\`\n`);
    return `\u0000${held.length - 1}\u0000`;
  });
  // Quotes, innermost first: [quote='name' pid='1' dateline='2'], [quote=name] and plain [quote].
  const inner = /\[quote(?:=('[^']*'|"[^"]*"|[^\]\s]*)(?:\s[^\]]*)?)?\]((?:(?!\[quote)[\s\S])*?)\[\/quote\]/i;
  for (let guard = 0; inner.test(t) && guard < 200; guard++) {
    t = t.replace(inner, (_, name, body) => {
      const lines = body.trim().split("\n").map((l) => `> ${l}`.trimEnd());
      return `\n${name ? `> **${unquote(name)}:**\n` : ""}${lines.join("\n")}\n\n`;
    });
  }
  t = t.replace(/\[list(?:=([^\]]*))?\]([\s\S]*?)\[\/list\]/gi, (_, kind, body) => {
    let n = 0;
    const items = body.split(/\[\*\]/).slice(1).map((i) => `${kind ? `${++n}.` : "-"} ${i.trim()}`);
    return `\n${items.join("\n")}\n\n`;
  });
  for (const [tag, mark] of [["b", "**"], ["i", "*"], ["u", "++"], ["s", "~~"]]) {
    t = t.replace(new RegExp(`\\[${tag}\\]([\\s\\S]*?)\\[\\/${tag}\\]`, "gi"), `${mark}$1${mark}`);
  }
  t = t
    .replace(/\[color=([^\]]+)\]([\s\S]*?)\[\/color\]/gi, (_, c, body) => (COLORS[c.toLowerCase()] ? `[${body}]{${COLORS[c.toLowerCase()]}}` : body))
    .replace(/\[(size|font|align)=[^\]]*\]([\s\S]*?)\[\/\1\]/gi, "$2")
    .replace(/\[url\]([\s\S]*?)\[\/url\]/gi, "$1")
    .replace(/\[url=([^\]]+)\]([\s\S]*?)\[\/url\]/gi, (_, u, text) => `[${text}](${unquote(u)})`)
    .replace(/\[email\]([\s\S]*?)\[\/email\]/gi, "[$1](mailto:$1)")
    .replace(/\[email=([^\]]+)\]([\s\S]*?)\[\/email\]/gi, "[$2](mailto:$1)")
    .replace(/\[img(?:=[^\]]*|\s[^\]]*)?\]([\s\S]*?)\[\/img\]/gi, "![]($1)")
    .replace(/\[video=[^\]]*\]([\s\S]*?)\[\/video\]/gi, "$1")
    .replace(/\[hr\]/gi, "\n---\n")
    .replace(/\[attachment=(\d+)\]/gi, (_, id) => {
      const a = byId.get(id);
      if (!a) return "";
      used.add(id);
      return link(a);
    });
  t = t.replace(/\u0000(\d+)\u0000/g, (_, i) => held[Number(i)]);
  // MyBB shows every line break: outside code a single newline becomes a hard break.
  t = t
    .split(/(\n```\n[\s\S]*?\n```\n)/)
    .map((part, i) => (i % 2 ? part : part.replace(/([^\n])\n(?!\n)/g, "$1  \n")))
    .join("");
  t = t.replace(/\n{3,}/g, "\n\n").trim();
  for (const a of attachments) if (!used.has(String(a.aid))) t += `\n\n${link(a)}`;
  return t;
}

// ---- who can read what: MyBB's permissions are per group, set on a forum or inherited from the ones above ------------
const groups = new Map((await q(`SELECT gid, canview, canviewthreads FROM ${p}usergroups`)).map((g) => [g.gid, g]));
const forumRows = await q(`SELECT fid, pid, name, type, disporder, parentlist FROM ${p}forums WHERE active = 1`);
const perms = await optional("permissions", () => q(`SELECT fid, gid, canview, canviewthreads FROM ${p}forumpermissions`));
/** Whether the group may see the forum and read its threads: the nearest forum up the tree with a rule for the group decides, else the group's own default. */
const canRead = (gid, forum) => {
  for (const fid of forum.parentlist.split(",").reverse().map(Number)) {
    const rule = perms.find((r) => r.fid === fid && r.gid === gid);
    if (rule) return rule.canview === 1 && rule.canviewthreads === 1;
  }
  const g = groups.get(gid);
  return !!g && g.canview === 1 && g.canviewthreads === 1;
};
const [GUESTS, REGISTERED] = [1, 2];
const byFid = new Map(forumRows.map((f) => [f.fid, f]));
const children = (id) => forumRows.filter((f) => f.pid === id);
const readable = (f) => (f.type === "c" ? children(f.fid).some(readable) : canRead(REGISTERED, f) || canRead(GUESTS, f));
const shownForums = forumRows.filter((f) => readable(f) && (!f.pid || byFid.has(f.pid)));
const shownIds = new Set(shownForums.map((f) => f.fid));

// ---- members and files ----------------------------------------------------------------------------------------------
const users = await q(`SELECT uid, username, email, password, salt, regdate, avatar, avatartype, usergroup, additionalgroups FROM ${p}users WHERE email <> ''`);
const known = new Set(users.map((u) => u.uid));
counts.members = await out.section("users", users.map((u) => ({ id: u.uid, username: u.username, email: u.email, password_hash: /^[0-9a-f]{32}$/i.test(u.password) && u.salt ? `ips3$${u.salt}$${u.password}` : null, joined_at: iso(u.regdate) })));

const attachmentRows = await optional("attachments", () => q(`SELECT aid, pid, uid, filename, attachname FROM ${p}attachments WHERE visible = 1 AND pid > 0 ORDER BY aid`));
const byPost = new Map();
for (const a of attachmentRows) {
  if (!byPost.has(a.pid)) byPost.set(a.pid, []);
  byPost.get(a.pid).push(a);
}
// Paths are relative to the MyBB folder, which is what BF_IMPORT_FILES points at.
const avatars = users
  .filter((u) => u.avatar && u.avatartype === "upload")
  .map((u) => ({ id: 1_000_000 + u.uid, refs: [], path: u.avatar.replace(/^\.\//, "").split("?")[0], name: "avatar", user_id: u.uid, avatar_of: u.uid }));
counts.files = await out.section("uploads", [
  ...avatars,
  ...attachmentRows.map((a) => ({ id: a.aid, refs: [`mb${a.aid}`], path: `uploads/${a.attachname}`, name: a.filename, user_id: known.has(a.uid) ? a.uid : null, avatar_of: null })),
]);

// ---- forums, topics, posts ------------------------------------------------------------------------------------------
counts.forums = await out.section(
  "forums",
  shownForums.map((f) => ({ id: f.fid, parent_id: f.pid || null, title: f.name, kind: f.type === "c" ? "category" : "forum", public: f.type === "c" ? true : canRead(GUESTS, f), position: f.disporder })),
);
const topics = (await q(`SELECT tid, fid, subject, views, sticky, closed FROM ${p}threads WHERE visible = 1 AND closed NOT LIKE 'moved|%'`)).filter((t) => shownIds.has(t.fid));
const shown = new Set(topics.map((t) => t.tid));
counts.topics = await out.section("topics", topics.map((t) => ({ id: t.tid, forum_id: t.fid, title: t.subject, url: `/showthread.php?tid=${t.tid}`, views: t.views, pinned: t.sticky === 1, locked: t.closed === "1" })));

async function* posts() {
  const rows = pages((offset, size) => q(`SELECT pid, tid, uid, message, dateline FROM ${p}posts WHERE visible = 1 ORDER BY tid, dateline, pid LIMIT ? OFFSET ?`, [size, offset]));
  for await (const x of rows) {
    if (shown.has(x.tid)) yield { id: x.pid, topic_id: x.tid, author_id: known.has(x.uid) ? x.uid : null, body: toMarkdown(x.message, byPost.get(x.pid) ?? []), format: "markdown", created_at: iso(x.dateline) };
  }
}
counts.posts = await out.section("posts", posts());

// ---- groups, polls, private messages --------------------------------------------------------------------------------
// Custom groups (type 2); a member is in one by their main group or an additional one.
const custom = await optional("groups", async () => {
  const gs = await q(`SELECT gid, title FROM ${p}usergroups WHERE type = 2`);
  const result = [];
  for (const g of gs) {
    const members = await q(`SELECT uid FROM ${p}users WHERE usergroup = ? OR FIND_IN_SET(?, additionalgroups)`, [g.gid, g.gid]);
    result.push({ id: g.gid, name: g.title, members: members.map((m) => m.uid) });
  }
  return result;
});
counts.groups = await out.section("groups", custom);

const polls = await optional("polls", async () => {
  const rows = await q(`SELECT pid, tid, question, dateline, options, numoptions, timeout, closed, multiple, public, maxoptions FROM ${p}polls`);
  const result = [];
  for (const pl of rows) {
    if (!shown.has(pl.tid)) continue;
    const options = pl.options.split("||~|~||");
    if (options.length < 2) continue;
    const votes = (await q(`SELECT uid, voteoption FROM ${p}pollvotes WHERE pid = ?`, [pl.pid])).filter((v) => v.voteoption >= 1 && v.voteoption <= options.length).map((v) => ({ user_id: v.uid, option: v.voteoption - 1 }));
    result.push({ id: pl.pid, topic_id: pl.tid, question: pl.question, options, max_choices: pl.multiple ? pl.maxoptions || options.length : 1, closes_at: pl.timeout > 0 ? iso(pl.dateline + pl.timeout * 86400) : pl.closed ? iso(Date.now() / 1000) : null, public: pl.public === 1, final_votes: true, votes });
  }
  return result;
});
counts.polls = await out.section("polls", polls);

// MyBB keeps a copy of each message in every folder it is in; the pair of people and the subject (without "RE:") make a conversation.
const conversations = await optional("messages", async () => {
  const rows = await q(`SELECT pmid, fromid, toid, subject, message, dateline FROM ${p}privatemessages ORDER BY dateline, pmid`);
  const seen = new Set();
  const threads = new Map();
  for (const m of rows) {
    const key = `${m.fromid}|${m.toid}|${m.dateline}|${m.subject}`;
    if (seen.has(key)) continue;
    seen.add(key);
    const id = `${[m.fromid, m.toid].sort().join("-")}|${m.subject.replace(/^(re|fw|fwd):\s*/i, "").toLowerCase()}`;
    if (!threads.has(id)) threads.set(id, { id: m.pmid, subject: m.subject.replace(/^(re|fw|fwd):\s*/i, ""), people: new Set(), messages: [] });
    const t = threads.get(id);
    t.people.add(m.fromid);
    t.people.add(m.toid);
    t.messages.push({ author_id: m.fromid, body: toMarkdown(m.message), format: "markdown", created_at: iso(m.dateline) });
  }
  return [...threads.values()].map((t) => ({ id: t.id, subject: t.subject, participants: [...t.people], messages: t.messages }));
});
counts.messages = await out.section("messages", conversations);

await out.end();
await db.end();
console.log(`Wrote ${Object.entries(counts).map(([k, n]) => `${n} ${k}`).join(", ")}.`);
