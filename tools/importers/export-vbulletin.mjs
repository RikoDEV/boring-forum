// Writes a vBulletin 4.x forum to the importer's JSON format:
//   npm i mysql2
//   VBULLETIN_DB="mysql://user:pass@host/dbname" node tools/importers/export-vbulletin.mjs out.json
//   BF_IMPORT_FILES=/path/to/vbulletin boring-forum import out.json
// Reads only. Set VBULLETIN_PREFIX if the tables have a prefix (the "tableprefix" of config.php, default none).
// Written from vBulletin 4.2's schema and not run against a live install: every part that varies between
// versions is read through `optional`, so a missing table only drops that section (the message says which).
// vBulletin 5 keeps its content in a different set of tables (`node`, `text`); this exporter stops with a message
// when it finds those and no `forum` table. An install upgraded from 4.x that still has the old tables works.
// Files: attachments and avatars kept in the file system are copied when BF_IMPORT_FILES is the folder the paths below
// are relative to. Attachments are read from `<VBULLETIN_ATTACH_DIR>/<digits of the uploader's id, one folder each>/
// <filedataid>.attach` (default dir `attachments`; the "attachpath" option of vBulletin, relative to the forum folder);
// avatars from `customavatars/avatar<userid>_<revision>.gif` (VBULLETIN_AVATAR_DIR). Files kept in the database
// are not exported: those posts keep their link without a file.
// Exports members (and avatars), forums (in the board's order; those inactive, password protected or links are left
// out, and so are those neither guests nor registered members may view, from forumpermission and the group defaults
// with inheritance down the tree; the bits are canview = 1 and canviewothers = 2, and a board where they cannot be
// read exports every forum), topics (views, sticky, closed), posts, attachments, tags, custom groups (ids above 8),
// polls (who voted what, whether voters are shown) and private messages. vBulletin has no reactions in core.
// Posts: vBulletin's BBCode (quotes with author, lists, [attach], images, links) is converted to Markdown here.
// Passwords: vBulletin keeps md5(md5(password) . salt), a scheme the importer does not take, so every member
// resets the password at the first sign-in.
import { createRequire } from "node:module";
import { Out, iso, optional, pages } from "./lib.mjs";
const require = createRequire(`${process.cwd()}/`);
const mysql = require("mysql2/promise");
const db = await mysql.createConnection(process.env.VBULLETIN_DB);
const p = process.env.VBULLETIN_PREFIX ?? "";
const ATTACH_DIR = (process.env.VBULLETIN_ATTACH_DIR ?? "attachments").replace(/\/+$/, "");
const AVATAR_DIR = (process.env.VBULLETIN_AVATAR_DIR ?? "customavatars").replace(/\/+$/, "");
const q = async (sql, args = []) => (await db.query(sql, args))[0];
const out = new Out(process.argv[2]);
const counts = {};

if (!(await q(`SHOW TABLES LIKE ?`, [`${p}forum`])).length) {
  const v5 = (await q(`SHOW TABLES LIKE ?`, [`${p}node`])).length > 0;
  console.error(v5 ? "This is a vBulletin 5 database (no `forum` table, content in `node`); only the 4.x tables are supported." : `No ${p}forum table: is VBULLETIN_PREFIX right?`);
  process.exit(1);
}

// ---- post text ------------------------------------------------------------------------------------------------------
const COLORS = { red: "red", orange: "orange", yellow: "yellow", green: "green", blue: "blue", purple: "purple", violet: "purple", gray: "gray", grey: "gray" };
const IMAGE = /\.(png|jpe?g|gif|webp)$/i;
const unquote = (s) => (s ?? "").trim().replace(/^['"]|['"]$/g, "");

/** vBulletin BBCode as Markdown. Each `[attach]<id>[/attach]` becomes a link to that file (`upload://vb<id>`, which the
 *  importer swaps for the stored copy); the post's files not placed in the text follow it. */
function toMarkdown(source, attachments = []) {
  const byId = new Map(attachments.map((a) => [String(a.attachmentid), a]));
  const used = new Set();
  const link = (a) => `${IMAGE.test(a.filename) ? "!" : ""}[${a.filename.replace(/[[\]]/g, " ")}](upload://vb${a.attachmentid})`;
  const held = [];
  let t = (source ?? "").replace(/\r\n?/g, "\n");
  // Code is set aside first, so nothing below touches what is inside it.
  t = t.replace(/\[(code|php|html)(?:=[^\]]*)?\]([\s\S]*?)\[\/\1\]/gi, (_, kind, body) => {
    held.push(`\n\`\`\`${kind.toLowerCase() === "code" ? "" : kind.toLowerCase()}\n${body.replace(/^\n|\n$/g, "")}\n\`\`\`\n`);
    return `\u0000${held.length - 1}\u0000`;
  });
  // Quotes, innermost first: [quote=Name;postid] (the name may hold spaces), [quote=Name] and plain [quote].
  const inner = /\[quote(?:=([^\]]*))?\]((?:(?!\[quote)[\s\S])*?)\[\/quote\]/i;
  for (let guard = 0; inner.test(t) && guard < 200; guard++) {
    t = t.replace(inner, (_, name, body) => {
      const who = unquote(name).replace(/;\d+$/, "");
      const lines = body.trim().split("\n").map((l) => `> ${l}`.trimEnd());
      return `\n${who ? `> **${who}:**\n` : ""}${lines.join("\n")}\n\n`;
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
    .replace(/\[color=([^\]]+)\]([\s\S]*?)\[\/color\]/gi, (_, c, body) => (COLORS[unquote(c).toLowerCase()] ? `[${body}]{${COLORS[unquote(c).toLowerCase()]}}` : body))
    .replace(/\[(size|font|align|left|center|right|indent|highlight|noparse)(?:=[^\]]*)?\]([\s\S]*?)\[\/\1\]/gi, "$2")
    .replace(/\[url\]([\s\S]*?)\[\/url\]/gi, "$1")
    .replace(/\[url=([^\]]+)\]([\s\S]*?)\[\/url\]/gi, (_, u, text) => `[${text}](${unquote(u)})`)
    .replace(/\[email\]([\s\S]*?)\[\/email\]/gi, "[$1](mailto:$1)")
    .replace(/\[email=([^\]]+)\]([\s\S]*?)\[\/email\]/gi, "[$2](mailto:$1)")
    .replace(/\[(?:img|img2)(?:=[^\]]*|\s[^\]]*)?\]([\s\S]*?)\[\/(?:img|img2)\]/gi, "![]($1)")
    .replace(/\[video=[^\]]*\]([\s\S]*?)\[\/video\]/gi, "$1")
    .replace(/\[(?:thread|post)(?:=[^\]]*)?\]([\s\S]*?)\[\/(?:thread|post)\]/gi, "$1")
    .replace(/\[hr\]/gi, "\n---\n")
    .replace(/\[attach(?:=[^\]]*)?\]\s*(\d+)\s*\[\/attach\]/gi, (_, id) => {
      const a = byId.get(id);
      if (!a) return "";
      used.add(id);
      return link(a);
    });
  t = t.replace(/\u0000(\d+)\u0000/g, (_, i) => held[Number(i)]);
  // vBulletin shows every line break: outside code a single newline becomes a hard break.
  t = t
    .split(/(\n```[a-z]*\n[\s\S]*?\n```\n)/)
    .map((part, i) => (i % 2 ? part : part.replace(/([^\n])\n(?!\n)/g, "$1  \n")))
    .join("");
  t = t.replace(/\n{3,}/g, "\n\n").trim();
  for (const a of attachments) if (!used.has(String(a.attachmentid))) t += `\n\n${link(a)}`;
  return t;
}

// ---- who can read what: per group, set on a forum or inherited, else the group's default -------------------------
const VIEW = 3; // canview (1) | canviewothers (2)
const groups = new Map((await optional("group permissions", () => q(`SELECT usergroupid, forumpermissions FROM ${p}usergroup`))).map((g) => [g.usergroupid, g.forumpermissions]));
const forumRows = (await q(`SELECT forumid, parentid, title, displayorder, options, parentlist, link, password FROM ${p}forum WHERE (options & 1) = 1 AND link = ''`)).filter((f) => !f.password);
const perms = await optional("forum permissions", () => q(`SELECT forumid, usergroupid, forumpermissions FROM ${p}forumpermission`));
/** Whether the group may view the forum: the nearest forum up the tree with a rule for the group decides, else the group's default. */
const canRead = (gid, forum) => {
  if (!groups.size) return true;
  for (const fid of String(forum.parentlist).split(",").map(Number)) {
    const rule = perms.find((r) => r.forumid === fid && r.usergroupid === gid);
    if (rule) return (rule.forumpermissions & VIEW) === VIEW;
  }
  return ((groups.get(gid) ?? 0) & VIEW) === VIEW;
};
const [GUESTS, REGISTERED] = [1, 2];
const byFid = new Map(forumRows.map((f) => [f.forumid, f]));
const children = (id) => forumRows.filter((f) => f.parentid === id);
const isCategory = (f) => (f.options & 4) === 0;
const readable = (f) => (isCategory(f) ? children(f.forumid).some(readable) : canRead(REGISTERED, f) || canRead(GUESTS, f));
const shownForums = forumRows.filter((f) => readable(f) && (f.parentid <= 0 || byFid.has(f.parentid)));
const shownIds = new Set(shownForums.map((f) => f.forumid));

// ---- members and files ----------------------------------------------------------------------------------------------
const users = await q(`SELECT userid, username, email, joindate, avatarrevision FROM ${p}user WHERE email <> ''`);
const known = new Set(users.map((u) => u.userid));
counts.members = await out.section("users", users.map((u) => ({ id: u.userid, username: u.username, email: u.email, password_hash: null, joined_at: iso(u.joindate) })));

const attachmentRows = await optional("attachments", () =>
  q(
    `SELECT a.attachmentid, a.contentid AS postid, a.userid, a.filedataid, a.filename, f.userid AS owner
     FROM ${p}attachment a
     JOIN ${p}contenttype c ON c.contenttypeid = a.contenttypeid AND c.class = 'Post'
     JOIN ${p}filedata f ON f.filedataid = a.filedataid
     WHERE a.state = 'visible' ORDER BY a.attachmentid`,
  ),
);
const byPost = new Map();
for (const a of attachmentRows) {
  if (!byPost.has(a.postid)) byPost.set(a.postid, []);
  byPost.get(a.postid).push(a);
}
const avatarUsers = new Set((await optional("avatars", () => q(`SELECT userid FROM ${p}customavatar`))).map((r) => r.userid));
const avatars = users
  .filter((u) => avatarUsers.has(u.userid))
  .map((u) => ({ id: 1_000_000 + u.userid, refs: [], path: `${AVATAR_DIR}/avatar${u.userid}_${u.avatarrevision ?? 1}.gif`, name: "avatar", user_id: u.userid, avatar_of: u.userid }));
counts.files = await out.section("uploads", [
  ...avatars,
  ...attachmentRows.map((a) => ({
    id: a.attachmentid,
    refs: [`vb${a.attachmentid}`],
    path: `${ATTACH_DIR}/${String(a.owner).split("").join("/")}/${a.filedataid}.attach`,
    name: a.filename,
    user_id: known.has(a.userid) ? a.userid : null,
    avatar_of: null,
  })),
]);

// ---- forums, topics, posts ------------------------------------------------------------------------------------------
counts.forums = await out.section(
  "forums",
  shownForums.map((f) => ({ id: f.forumid, parent_id: f.parentid > 0 ? f.parentid : null, title: f.title, kind: isCategory(f) ? "category" : "forum", public: isCategory(f) ? true : canRead(GUESTS, f), position: f.displayorder })),
);
// `open` is 1 for an open thread, 0 for a closed one and 10 for a redirect to a moved thread.
const topics = (await q(`SELECT threadid, forumid, title, views, sticky, open FROM ${p}thread WHERE visible = 1 AND open <> 10`)).filter((t) => shownIds.has(t.forumid));
const shown = new Set(topics.map((t) => t.threadid));
counts.topics = await out.section("topics", topics.map((t) => ({ id: t.threadid, forum_id: t.forumid, title: t.title, url: `/showthread.php?t=${t.threadid}`, views: t.views, pinned: t.sticky === 1, locked: t.open === 0 })));

async function* posts() {
  const rows = pages((offset, size) => q(`SELECT postid, threadid, userid, pagetext, dateline FROM ${p}post WHERE visible = 1 ORDER BY threadid, dateline, postid LIMIT ? OFFSET ?`, [size, offset]));
  for await (const x of rows) {
    if (shown.has(x.threadid)) yield { id: x.postid, topic_id: x.threadid, author_id: known.has(x.userid) ? x.userid : null, body: toMarkdown(x.pagetext, byPost.get(x.postid) ?? []), format: "markdown", created_at: iso(x.dateline) };
  }
}
counts.posts = await out.section("posts", posts());

// ---- tags, groups, polls, private messages --------------------------------------------------------------------------
const tags = await optional("tags", async () => {
  const rows = await q(`SELECT tt.threadid, t.tagtext FROM ${p}tagthread tt JOIN ${p}tag t ON t.tagid = tt.tagid`);
  return rows.filter((r) => shown.has(r.threadid)).map((r) => ({ topic_id: r.threadid, name: r.tagtext }));
});
counts.tags = await out.section("tags", tags);

// Custom groups: vBulletin's own are 1 to 8; a member is in one by their main group or an additional one.
const custom = await optional("groups", async () => {
  const gs = await q(`SELECT usergroupid, title FROM ${p}usergroup WHERE usergroupid > 8`);
  const result = [];
  for (const g of gs) {
    const members = await q(`SELECT userid FROM ${p}user WHERE usergroupid = ? OR FIND_IN_SET(?, membergroupids)`, [g.usergroupid, g.usergroupid]);
    result.push({ id: g.usergroupid, name: g.title, members: members.map((m) => m.userid) });
  }
  return result;
});
counts.groups = await out.section("groups", custom);

const polls = await optional("polls", async () => {
  const rows = await q(`SELECT p.pollid, t.threadid, p.question, p.dateline, p.options, p.timeout, p.active, p.multiple, p.public FROM ${p}poll p JOIN ${p}thread t ON t.pollid = p.pollid`);
  const result = [];
  for (const pl of rows) {
    if (!shown.has(pl.threadid)) continue;
    const options = String(pl.options).split("|||");
    if (options.length < 2) continue;
    const votes = (await q(`SELECT userid, voteoption FROM ${p}pollvote WHERE pollid = ? AND userid > 0`, [pl.pollid]))
      .filter((v) => v.voteoption >= 1 && v.voteoption <= options.length)
      .map((v) => ({ user_id: v.userid, option: v.voteoption - 1 }));
    // `timeout` is a number of days after the poll was made; 0 means it never closes on its own.
    const closes = pl.timeout > 0 ? iso(pl.dateline + pl.timeout * 86400) : pl.active ? null : iso(Date.now() / 1000);
    result.push({ id: pl.pollid, topic_id: pl.threadid, question: pl.question, options, max_choices: pl.multiple ? options.length : 1, closes_at: closes, public: pl.public === 1, final_votes: true, votes });
  }
  return result;
});
counts.polls = await out.section("polls", polls);

// Every message is a `pmtext`, with a copy per person in `pm`. Messages between the same people with the same
// subject (without "Re:") make one conversation.
const conversations = await optional("messages", async () => {
  const rows = await q(`SELECT pmtextid, fromuserid, title, message, dateline FROM ${p}pmtext ORDER BY dateline, pmtextid`);
  const to = new Map();
  for (const r of await q(`SELECT pmtextid, userid FROM ${p}pm`)) {
    if (!to.has(r.pmtextid)) to.set(r.pmtextid, new Set());
    to.get(r.pmtextid).add(r.userid);
  }
  const threads = new Map();
  for (const m of rows) {
    const people = new Set([m.fromuserid, ...(to.get(m.pmtextid) ?? [])].filter((u) => known.has(u)));
    if (people.size < 2) continue;
    const subject = m.title.replace(/^((re|fw|fwd):\s*)+/i, "");
    const id = `${[...people].sort((a, b) => a - b).join("-")}|${subject.toLowerCase()}`;
    if (!threads.has(id)) threads.set(id, { id: m.pmtextid, subject, people: new Set(), messages: [] });
    const t = threads.get(id);
    for (const u of people) t.people.add(u);
    t.messages.push({ author_id: known.has(m.fromuserid) ? m.fromuserid : null, body: toMarkdown(m.message), format: "markdown", created_at: iso(m.dateline) });
  }
  return [...threads.values()].map((t) => ({ id: t.id, subject: t.subject, participants: [...t.people], messages: t.messages }));
});
counts.messages = await out.section("messages", conversations);

await out.end();
await db.end();
console.log(`Wrote ${Object.entries(counts).map(([k, n]) => `${n} ${k}`).join(", ")}.`);
