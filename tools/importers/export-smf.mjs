// Writes a Simple Machines Forum 2.0 / 2.1 forum to the importer's JSON format:
//   npm i mysql2
//   SMF_DB="mysql://user:pass@host/dbname" node tools/importers/export-smf.mjs out.json
//   BF_IMPORT_FILES=/path/to/smf boring-forum import out.json
// Reads only. Set SMF_PREFIX if the tables do not use smf_. Give the importer the SMF folder (the one with
// `attachments/`) as BF_IMPORT_FILES: attachments and uploaded avatars are copied from there, and the pictures in
// posts show. Without it they are skipped.
// Exports members and their uploaded avatars, categories (as category forums) with their boards (child boards nested),
// topics (views, sticky, locked), approved posts, attachments, custom member groups, polls (who voted what, whether
// results are hidden) and personal messages (2.1 conversations by `id_pm_head`; 2.0 by people and subject).
// Written without a real install to test against; these are the assumptions:
//  - Passwords: SMF keeps sha1(lower(username) . password), which the importer does not check, so every member
//    resets the password at first sign-in.
//  - Who can read what: a board is exported when guests (-1) or ordinary members (0) are in its `member_groups` (2.0) or
//    allowed by `board_groups` (2.1). Boards only some other group can see (staff boards) are left out with their topics,
//    and so are redirect boards. Guests may read a board when -1 is in the list. Per-member bans are not looked at.
//  - Members: everyone with an email address is exported, activated or not.
//  - Attachment files: `attachments/<id_attach>_<file_hash>` (uploaded avatars too). Files kept in another upload
//    folder (SMF's "multiple folders" setting) are not found by the importer and are counted as skipped.
//  - Posts: SMF's BBCode (HTML line breaks, entities, quotes with author, lists, colours, inline `[attach]`) is converted to
//    Markdown here. Moved-topic stubs (2.1 `id_redirect_topic`, or the 2.0 "MOVED:" post) are left out. Nested lists lose
//    their nesting. Smileys stay as the text they were typed as.
//  - Private messages: blind-copy recipients are not shown as participants; a message whose sender no longer exists is
//    left out. SMF has no tags or reactions in core.
import { createRequire } from "node:module";
import { Out, iso, optional, pages } from "./lib.mjs";
const require = createRequire(`${process.cwd()}/`);
const mysql = require("mysql2/promise");
const db = await mysql.createConnection(process.env.SMF_DB);
const p = process.env.SMF_PREFIX ?? "smf_";
const q = async (sql, args = []) => (await db.query(sql, args))[0];
const out = new Out(process.argv[2]);
const counts = {};

/** The first of several versions of a query that this database accepts (columns differ between 2.0 and 2.1). */
async function first(what, sqls) {
  let last;
  for (const sql of sqls) {
    try {
      return await q(sql);
    } catch (e) {
      last = e;
    }
  }
  console.error(`skipped ${what}: ${last.message}`);
  return [];
}

// ---- post text ------------------------------------------------------------------------------------------------------
const COLORS = { red: "red", orange: "orange", yellow: "yellow", green: "green", blue: "blue", purple: "purple", violet: "purple", gray: "gray", grey: "gray" };
const IMAGE = /\.(png|jpe?g|gif|webp)$/i;
const unquote = (s) => (s ?? "").replace(/^['"]|['"]$/g, "");
const ENTITIES = { nbsp: " ", quot: '"', lt: "<", gt: ">", apos: "'", amp: "&" };
/** SMF stores text with HTML entities (`&quot;`, `&#039;`, `&nbsp;`); `&amp;` goes last so `&amp;lt;` stays `&lt;`. */
const decode = (s) =>
  s
    .replace(/&#(\d+);/g, (m, n) => (Number(n) < 0x110000 ? String.fromCodePoint(Number(n)) : m))
    .replace(/&#x([0-9a-f]+);/gi, (m, n) => (parseInt(n, 16) < 0x110000 ? String.fromCodePoint(parseInt(n, 16)) : m))
    .replace(/&(nbsp|quot|lt|gt|apos);/g, (_, e) => ENTITIES[e])
    .replace(/&amp;/g, "&");

/** SMF BBCode as Markdown. Each `[attach=<id>]` becomes a link to that file (`upload://sm<id>`, which the importer swaps
 *  for the stored copy); the post's files not placed in the text follow it, as SMF lists them. */
function toMarkdown(source, attachments = []) {
  const byId = new Map(attachments.map((a) => [String(a.id_attach), a]));
  const used = new Set();
  const link = (a) => `${IMAGE.test(a.filename) ? "!" : ""}[${decode(a.filename).replace(/[[\]]/g, " ")}](upload://sm${a.id_attach})`;
  const held = [];
  let t = source.replace(/\r\n?/g, "\n").replace(/<br\s*\/?>/gi, "\n");
  // Code is set aside first, so nothing below touches what is inside it.
  t = t.replace(/\[(code|php|pre|nobbc)(?:=[^\]]*)?\]([\s\S]*?)\[\/\1\]/gi, (_, kind, body) => {
    const text = decode(body.replace(/^\n|\n$/g, ""));
    held.push(kind.toLowerCase() === "nobbc" ? text : `\n\`\`\`\n${text}\n\`\`\`\n`);
    return `\u0000${held.length - 1}\u0000`;
  });
  // Quotes, innermost first: [quote author=Name link=topic=1.msg2#msg2 date=3], [quote=Name] and plain [quote].
  const inner = /\[quote((?:\s[^\]]*|=[^\]]*)?)\]((?:(?!\[quote)[\s\S])*?)\[\/quote\]/i;
  for (let guard = 0; inner.test(t) && guard < 200; guard++) {
    t = t.replace(inner, (_, attrs, body) => {
      const named = /^\s+author=(.*?)(?:\s+(?:link|date)=|$)/.exec(attrs) ?? /^=(.*)$/.exec(attrs);
      const name = named ? unquote(named[1].trim()) : "";
      const lines = body.trim().split("\n").map((l) => `> ${l}`.trimEnd());
      return `\n${name ? `> **${name}:**\n` : ""}${lines.join("\n")}\n\n`;
    });
  }
  // Lists: `[list][li]a[/li][li]b[/li][/list]`, or the older `[list][*]a[*]b[/list]`; a `type` makes it numbered.
  t = t.replace(/\[list([^\]]*)\]([\s\S]*?)\[\/list\]/gi, (_, attrs, body) => {
    const numbered = /decimal|lower|upper|=\s*[1aAiI]\b/.test(attrs);
    const li = [...body.matchAll(/\[li\]([\s\S]*?)\[\/li\]/gi)].map((m) => m[1]);
    const items = (li.length ? li : body.split(/\[\*\]/).slice(1)).map((i) => i.replace(/\[\/\*\]/g, "").trim());
    let n = 0;
    return `\n${items.map((i) => `${numbered ? `${++n}.` : "-"} ${i}`).join("\n")}\n\n`;
  });
  for (const [tag, mark] of [["b", "**"], ["i", "*"], ["u", "++"], ["s", "~~"]]) {
    t = t.replace(new RegExp(`\\[${tag}\\]([\\s\\S]*?)\\[\\/${tag}\\]`, "gi"), `${mark}$1${mark}`);
  }
  t = t
    .replace(/\[tt\]([\s\S]*?)\[\/tt\]/gi, "`$1`")
    .replace(/\[me=([^\]]*)\]([\s\S]*?)\[\/me\]/gi, (_, name, body) => `*${unquote(name)} ${body.trim()}*`)
    .replace(/\[color=([^\]]+)\]([\s\S]*?)\[\/color\]/gi, (_, c, body) => (COLORS[c.toLowerCase()] ? `[${body}]{${COLORS[c.toLowerCase()]}}` : body))
    .replace(/\[(size|font|glow|shadow|move|bdo|abbr)(?:=[^\]]*)?\]([\s\S]*?)\[\/\1\]/gi, "$2")
    .replace(/\[(?:\/)?(?:center|left|right|justify|sup|sub|table|th|tbody|thead)(?:=[^\]]*)?\]/gi, "")
    .replace(/\[\/td\]/gi, " | ")
    .replace(/\[\/tr\]/gi, "\n")
    .replace(/\[\/?(?:tr|td)(?:\s[^\]]*)?\]/gi, "")
    .replace(/\[(?:i?url|ftp)\]([\s\S]*?)\[\/(?:i?url|ftp)\]/gi, "$1")
    .replace(/\[(?:i?url|ftp)=([^\]]+)\]([\s\S]*?)\[\/(?:i?url|ftp)\]/gi, (_, u, text) => `[${text}](${unquote(u)})`)
    .replace(/\[email\]([\s\S]*?)\[\/email\]/gi, "[$1](mailto:$1)")
    .replace(/\[email=([^\]]+)\]([\s\S]*?)\[\/email\]/gi, "[$2](mailto:$1)")
    .replace(/\[img(?:=[^\]]*|\s[^\]]*)?\]([\s\S]*?)\[\/img\]/gi, "![]($1)")
    .replace(/\[hr\]/gi, "\n---\n")
    .replace(/\[attach(?:=|\s+id=)(\d+)[^\]]*\](?:\[\/attach\])?/gi, (_, id) => {
      const a = byId.get(id);
      if (!a) return "";
      used.add(id);
      return link(a);
    });
  t = decode(t);
  t = t.replace(/\u0000(\d+)\u0000/g, (_, i) => held[Number(i)]);
  // SMF shows every line break: outside code a single newline becomes a hard break.
  t = t
    .split(/(\n```\n[\s\S]*?\n```\n)/)
    .map((part, i) => (i % 2 ? part : part.replace(/([^\n])\n(?!\n)/g, "$1  \n")))
    .join("");
  t = t.replace(/\n{3,}/g, "\n\n").trim();
  for (const a of attachments) if (!used.has(String(a.id_attach))) t += `\n\n${link(a)}`;
  return t;
}

// ---- who can read what: SMF boards list the member groups that see them -----------------------------------------------
const catRows = await q(`SELECT id_cat, name, cat_order FROM ${p}categories ORDER BY cat_order, id_cat`);
let boardRows = await optional("boards", () => q(`SELECT id_board, id_cat, child_level, id_parent, board_order, name, redirect, member_groups FROM ${p}boards`));
if (!boardRows.length) {
  // 2.1 moved the list to its own table (`board_groups`, allow or deny per group).
  boardRows = await q(`SELECT id_board, id_cat, child_level, id_parent, board_order, name, redirect FROM ${p}boards`);
  const allow = await optional("board groups", () => q(`SELECT id_board, id_group, permission FROM ${p}board_groups`));
  const lists = new Map();
  for (const g of allow) {
    const l = lists.get(g.id_board) ?? { allow: new Set(), deny: new Set() };
    (g.permission === "deny" ? l.deny : l.allow).add(String(g.id_group));
    lists.set(g.id_board, l);
  }
  for (const b of boardRows) {
    const l = lists.get(b.id_board);
    b.member_groups = l ? [...l.allow].filter((g) => !l.deny.has(g)).join(",") : "";
  }
}
const groupsOf = (b) => new Set(String(b.member_groups ?? "").split(",").map((s) => s.trim()).filter(Boolean));
const guestsCan = (b) => groupsOf(b).has("-1");
const membersCan = (b) => guestsCan(b) || groupsOf(b).has("0");
boardRows.sort((a, b) => a.child_level - b.child_level || a.board_order - b.board_order || a.id_board - b.id_board);
const shownBoards = [];
const shownIds = new Set();
for (const b of boardRows) {
  if (b.redirect || !membersCan(b) || (b.id_parent && !shownIds.has(b.id_parent))) continue;
  shownBoards.push(b);
  shownIds.add(b.id_board);
}
const CAT = 1_000_000;
const shownCats = catRows.filter((c) => shownBoards.some((b) => b.id_cat === c.id_cat));

// ---- members and files ----------------------------------------------------------------------------------------------
const users = await q(`SELECT id_member, member_name, real_name, email_address, date_registered, id_group, additional_groups FROM ${p}members WHERE email_address <> ''`);
const known = new Set(users.map((u) => u.id_member));
// When each member was last on the old site (unix time or a date); a table or column this version lacks gives none.
const seen = new Map((await optional("last visits", () => q(`SELECT id_member AS id, last_login AS t FROM ${p}members`))).map((r) => [r.id, r.t]));
counts.members = await out.section("users", users.map((u) => ({ id: u.id_member, username: u.real_name || u.member_name, email: u.email_address, password_hash: null, joined_at: iso(u.date_registered), last_seen_at: iso(seen.get(u.id_member) || null) })));

// Files of posts in exported boards (the topic's board is joined in), and the uploaded avatars.
const attachmentRows = await first("attachments", [
  `SELECT a.id_attach, a.id_msg, a.id_member, a.filename, a.file_hash, a.attachment_type, t.id_board
     FROM ${p}attachments a LEFT JOIN ${p}messages m ON m.id_msg = a.id_msg LEFT JOIN ${p}topics t ON t.id_topic = m.id_topic
     WHERE (a.attachment_type = 0 AND a.id_msg > 0 AND a.approved = 1 AND m.approved = 1) OR (a.attachment_type = 1 AND a.id_member > 0) ORDER BY a.id_attach`,
  `SELECT a.id_attach, a.id_msg, a.id_member, a.filename, a.file_hash, a.attachment_type, t.id_board
     FROM ${p}attachments a LEFT JOIN ${p}messages m ON m.id_msg = a.id_msg LEFT JOIN ${p}topics t ON t.id_topic = m.id_topic
     WHERE (a.attachment_type = 0 AND a.id_msg > 0) OR (a.attachment_type = 1 AND a.id_member > 0) ORDER BY a.id_attach`,
]);
// Paths are relative to the SMF folder, which is what BF_IMPORT_FILES points at.
const filePath = (a) => `attachments/${a.id_attach}_${a.file_hash || a.filename}`;
const postFiles = attachmentRows.filter((a) => a.attachment_type === 0 && shownIds.has(a.id_board));
const avatars = attachmentRows.filter((a) => a.attachment_type === 1 && known.has(a.id_member));
const byPost = new Map();
for (const a of postFiles) {
  if (!byPost.has(a.id_msg)) byPost.set(a.id_msg, []);
  byPost.get(a.id_msg).push(a);
}
counts.files = await out.section("uploads", [
  ...avatars.map((a) => ({ id: 1_000_000 + a.id_member, refs: [], path: filePath(a), name: "avatar", user_id: a.id_member, avatar_of: a.id_member })),
  ...postFiles.map((a) => ({ id: a.id_attach, refs: [`sm${a.id_attach}`], path: filePath(a), name: a.filename, user_id: known.has(a.id_member) ? a.id_member : null, avatar_of: null })),
]);

// ---- forums, topics, posts ------------------------------------------------------------------------------------------
counts.forums = await out.section("forums", [
  ...shownCats.map((c) => ({ id: CAT + c.id_cat, parent_id: null, title: decode(c.name), kind: "category", public: true, position: c.cat_order })),
  ...shownBoards.map((b) => ({ id: b.id_board, parent_id: b.id_parent || CAT + b.id_cat, title: decode(b.name), kind: "forum", public: guestsCan(b), position: b.board_order })),
]);
const topicRows = await first("topics", [
  `SELECT t.id_topic, t.id_board, t.is_sticky, t.locked, t.num_views, t.id_poll, m.subject, LEFT(m.body, 80) AS head
     FROM ${p}topics t JOIN ${p}messages m ON m.id_msg = t.id_first_msg WHERE t.approved = 1 AND t.id_redirect_topic = 0`,
  `SELECT t.id_topic, t.id_board, t.is_sticky, t.locked, t.num_views, t.id_poll, m.subject, LEFT(m.body, 80) AS head
     FROM ${p}topics t JOIN ${p}messages m ON m.id_msg = t.id_first_msg WHERE t.approved = 1`,
]);
// A 2.0 "moved" stub is a topic of one post whose subject starts with MOVED:.
const topics = topicRows.filter((t) => shownIds.has(t.id_board) && !(/^MOVED:/i.test(t.subject) && /moved to/i.test(t.head ?? "")));
const shown = new Set(topics.map((t) => t.id_topic));
counts.topics = await out.section(
  "topics",
  topics.map((t) => ({ id: t.id_topic, forum_id: t.id_board, title: decode(t.subject), url: `/index.php?topic=${t.id_topic}.0`, views: t.num_views, pinned: t.is_sticky === 1, locked: t.locked !== 0 })),
);

async function* posts() {
  const rows = pages((offset, size) => q(`SELECT id_msg, id_topic, id_member, body, poster_time FROM ${p}messages WHERE approved = 1 ORDER BY id_topic, poster_time, id_msg LIMIT ? OFFSET ?`, [size, offset]));
  for await (const x of rows) {
    if (shown.has(x.id_topic)) yield { id: x.id_msg, topic_id: x.id_topic, author_id: known.has(x.id_member) ? x.id_member : null, body: toMarkdown(x.body, byPost.get(x.id_msg) ?? []), format: "markdown", created_at: iso(x.poster_time) };
  }
}
counts.posts = await out.section("posts", posts());

// ---- groups, polls, private messages --------------------------------------------------------------------------------
// Custom groups: those members are put in by hand (not the post-count ranks, which have min_posts >= 0, nor the built-in staff ones).
const custom = await optional("groups", async () => {
  const gs = await first("groups", [
    `SELECT id_group, group_name FROM ${p}membergroups WHERE id_group > 0 AND min_posts = -1 AND (id_group >= 4 OR group_type > 0)`,
    `SELECT id_group, group_name FROM ${p}membergroups WHERE id_group >= 4 AND min_posts = -1`,
  ]);
  const inGroup = (u, id) => u.id_group === id || String(u.additional_groups ?? "").split(",").some((g) => Number(g) === id);
  return gs.map((g) => ({ id: g.id_group, name: decode(g.group_name), members: users.filter((u) => inGroup(u, g.id_group)).map((u) => u.id_member) }));
});
counts.groups = await out.section("groups", custom);

const polls = await optional("polls", async () => {
  const rows = await q(`SELECT id_poll, question, voting_locked, max_votes, expire_time, hide_results, change_vote FROM ${p}polls`);
  const topicOf = new Map(topics.filter((t) => t.id_poll).map((t) => [t.id_poll, t.id_topic]));
  const choices = await q(`SELECT id_poll, id_choice, label FROM ${p}poll_choices ORDER BY id_poll, id_choice`);
  const casts = await q(`SELECT id_poll, id_member, id_choice FROM ${p}log_polls WHERE id_member > 0`);
  const result = [];
  for (const pl of rows) {
    const topic_id = topicOf.get(pl.id_poll);
    if (!topic_id) continue;
    const mine = choices.filter((c) => c.id_poll === pl.id_poll);
    if (mine.length < 2) continue;
    const index = new Map(mine.map((c, i) => [c.id_choice, i]));
    const votes = casts.filter((v) => v.id_poll === pl.id_poll && index.has(v.id_choice) && known.has(v.id_member)).map((v) => ({ user_id: v.id_member, option: index.get(v.id_choice) }));
    result.push({
      id: pl.id_poll,
      topic_id,
      question: decode(pl.question),
      options: mine.map((c) => decode(c.label)),
      max_choices: pl.max_votes > 1 ? Math.min(pl.max_votes, mine.length) : 1,
      closes_at: pl.expire_time > 0 ? iso(pl.expire_time) : pl.voting_locked ? iso(Date.now() / 1000) : null,
      public: false,
      results: ["always", "voted", "closed"][pl.hide_results] ?? "always",
      final_votes: pl.change_vote === 0,
      votes,
    });
  }
  return result;
});
counts.polls = await out.section("polls", polls);

// SMF stores each message once, with a list of recipients. 2.1 groups replies under the first message (`id_pm_head`);
// 2.0 has no threads, so the same people and the subject (without "RE:") make a conversation.
const conversations = await optional("messages", async () => {
  const rows = await first("personal messages", [
    `SELECT id_pm, id_pm_head, id_member_from, subject, body, msgtime FROM ${p}personal_messages ORDER BY msgtime, id_pm`,
    `SELECT id_pm, id_pm AS id_pm_head, id_member_from, subject, body, msgtime FROM ${p}personal_messages ORDER BY msgtime, id_pm`,
  ]);
  const to = new Map();
  for (const r of await q(`SELECT id_pm, id_member, bcc FROM ${p}pm_recipients`)) {
    if (r.bcc || !known.has(r.id_member)) continue;
    if (!to.has(r.id_pm)) to.set(r.id_pm, new Set());
    to.get(r.id_pm).add(r.id_member);
  }
  const hasHeads = rows.some((m) => m.id_pm_head && m.id_pm_head !== m.id_pm);
  const threads = new Map();
  for (const m of rows) {
    if (!known.has(m.id_member_from)) continue;
    const people = new Set([m.id_member_from, ...(to.get(m.id_pm) ?? [])]);
    if (people.size < 2) continue;
    const plain = m.subject.replace(/^(re|fw|fwd):\s*/i, "");
    const key = hasHeads ? `h${m.id_pm_head || m.id_pm}` : `${[...people].sort().join("-")}|${plain.toLowerCase()}`;
    if (!threads.has(key)) threads.set(key, { id: m.id_pm, subject: decode(plain), people: new Set(), messages: [] });
    const t = threads.get(key);
    for (const x of people) t.people.add(x);
    t.messages.push({ author_id: m.id_member_from, body: toMarkdown(m.body), format: "markdown", created_at: iso(m.msgtime) });
  }
  return [...threads.values()].map((t) => ({ id: t.id, subject: t.subject, participants: [...t.people], messages: t.messages }));
});
counts.messages = await out.section("messages", conversations);

await out.end();
await db.end();
console.log(`Wrote ${Object.entries(counts).map(([k, n]) => `${n} ${k}`).join(", ")}.`);
