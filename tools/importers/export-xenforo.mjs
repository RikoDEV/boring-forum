// Writes a XenForo 2.x forum to the importer's JSON format:
//   npm i mysql2
//   XENFORO_DB="mysql://user:pass@host/dbname" node tools/importers/export-xenforo.mjs out.json
//   BF_IMPORT_FILES=/path/to/xenforo boring-forum import out.json
// Reads only. Set XENFORO_PREFIX if the tables do not use xf_. Written from XenForo 2.2's schema and not run against a
// live install: every part that varies between versions is read through `optional`, so a missing table only drops that
// section (the message says which).
// Files: give the importer the XenForo folder (the one with `data/` and `internal_data/`) as BF_IMPORT_FILES.
// Attachments are read from `<XENFORO_ATTACH_DIR>/<floor(data_id / 1000)>/<data_id>-<file_hash>.data`, where the dir
// defaults to `internal_data/attachments` (XenForo 2 keeps the originals there; `data/attachments` holds only thumbnails;
// set XENFORO_ATTACH_DIR if the install moved them). Avatars come from `data/avatars/o/<floor(user_id / 1000)>/<user_id>.jpg`.
// Exports members (and avatars), forums (in the board's order, Category and Forum nodes only), topics (views, sticky,
// closed), posts, attachments, tags, custom groups (ids above 4), polls (who voted what, whether voters are shown),
// reactions (as likes) and conversations. Only content in the `visible` state is exported.
// Who can read what: XenForo's permissions are a cache of group, user and node rules that is not reproduced here. A
// forum is left out (with its topics) when both Unregistered and Registered have the node's `view` permission set to
// Never (`reset`) on it or on a node above; everything else is exported as a forum anyone with the right group may
// read, so check private forums after the import. BF_IMPORT_PUBLIC decides whether guests read the rest, except
// forums Unregistered cannot view, which are marked as not public.
// Posts: XenForo's BBCode (quotes with author, [ATTACH], [MEDIA], [SPOILER], [USER], lists, code) is converted to
// Markdown here.
// Passwords: the bcrypt hash XenForo 2 keeps (inside the serialized `xf_user_authenticate.data`) is carried over as it
// is; members with another scheme (an old XenForo 1 or imported hash) reset their password.
import { createRequire } from "node:module";
import { Out, iso, optional, pages } from "./lib.mjs";
const require = createRequire(`${process.cwd()}/`);
const mysql = require("mysql2/promise");
const db = await mysql.createConnection(process.env.XENFORO_DB);
const p = process.env.XENFORO_PREFIX ?? "xf_";
const ATTACH_DIR = (process.env.XENFORO_ATTACH_DIR ?? "internal_data/attachments").replace(/\/+$/, "");
const q = async (sql, args = []) => (await db.query(sql, args))[0];
const out = new Out(process.argv[2]);
const counts = {};

if (!(await q(`SHOW TABLES LIKE ?`, [`${p}thread`])).length) {
  console.error(`No ${p}thread table: is XENFORO_PREFIX right?`);
  process.exit(1);
}

// ---- post text ------------------------------------------------------------------------------------------------------
const COLORS = { red: "red", orange: "orange", yellow: "yellow", green: "green", blue: "blue", purple: "purple", violet: "purple", gray: "gray", grey: "gray" };
const IMAGE = /\.(png|jpe?g|gif|webp|avif)$/i;
const unquote = (s) => (s ?? "").trim().replace(/^['"]|['"]$/g, "");
const MEDIA = { youtube: (id) => `https://www.youtube.com/watch?v=${id}`, vimeo: (id) => `https://vimeo.com/${id}`, dailymotion: (id) => `https://www.dailymotion.com/video/${id}` };

/** XenForo BBCode as Markdown. Each `[ATTACH]<id>[/ATTACH]` becomes a link to that file (`upload://xf<id>`, which the
 *  importer swaps for the stored copy); the post's files not placed in the text follow it. */
function toMarkdown(source, attachments = []) {
  const byId = new Map(attachments.map((a) => [String(a.attachment_id), a]));
  const used = new Set();
  const link = (a) => `${IMAGE.test(a.filename) ? "!" : ""}[${a.filename.replace(/[[\]]/g, " ")}](upload://xf${a.attachment_id})`;
  const held = [];
  let t = (source ?? "").replace(/\r\n?/g, "\n");
  // Code is set aside first, so nothing below touches what is inside it.
  t = t.replace(/\[(code|php|html)(?:=([^\]]*))?\]([\s\S]*?)\[\/\1\]/gi, (_, kind, lang, body) => {
    const language = kind.toLowerCase() === "code" ? unquote(lang ?? "").toLowerCase() : kind.toLowerCase();
    held.push(`\n\`\`\`${/^[a-z0-9+#-]*$/.test(language) ? language : ""}\n${body.replace(/^\n|\n$/g, "")}\n\`\`\`\n`);
    return `\u0000${held.length - 1}\u0000`;
  });
  t = t.replace(/\[icode\]([\s\S]*?)\[\/icode\]/gi, (_, body) => {
    held.push(`\`${body.replace(/`/g, "'")}\``);
    return `\u0000${held.length - 1}\u0000`;
  });
  // Quotes, innermost first: [QUOTE="Name, post: 12, member: 3"], [QUOTE=Name] and plain [QUOTE].
  const inner = /\[quote(?:=([^\]]*))?\]((?:(?!\[quote)[\s\S])*?)\[\/quote\]/i;
  for (let guard = 0; inner.test(t) && guard < 200; guard++) {
    t = t.replace(inner, (_, name, body) => {
      const who = unquote(name).split(/,\s*(?:post|member|thread):/i)[0].trim();
      const lines = body.trim().split("\n").map((l) => `> ${l}`.trimEnd());
      return `\n${who ? `> **${who}:**\n` : ""}${lines.join("\n")}\n\n`;
    });
  }
  // Spoilers become a labelled quote, nested ones innermost first.
  const spoiler = /\[spoiler(?:=([^\]]*))?\]((?:(?!\[spoiler)[\s\S])*?)\[\/spoiler\]/i;
  for (let guard = 0; spoiler.test(t) && guard < 100; guard++) {
    t = t.replace(spoiler, (_, title, body) => `\n> **${unquote(title) || "Spoiler"}:**\n${body.trim().split("\n").map((l) => `> ${l}`.trimEnd()).join("\n")}\n\n`);
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
    .replace(/\[heading=(\d)\]([\s\S]*?)\[\/heading\]/gi, (_, n, body) => `\n${"#".repeat(Math.min(6, Math.max(2, Number(n) + 1)))} ${body.trim()}\n`)
    .replace(/\[color=([^\]]+)\]([\s\S]*?)\[\/color\]/gi, (_, c, body) => (COLORS[unquote(c).toLowerCase()] ? `[${body}]{${COLORS[unquote(c).toLowerCase()]}}` : body))
    .replace(/\[(size|font|left|center|right|indent|plain|user|noparse)(?:=[^\]]*)?\]([\s\S]*?)\[\/\1\]/gi, "$2")
    .replace(/\[url\]([\s\S]*?)\[\/url\]/gi, "$1")
    .replace(/\[url=([^\]]+)\]([\s\S]*?)\[\/url\]/gi, (_, u, text) => `[${text}](${unquote(u)})`)
    .replace(/\[email\]([\s\S]*?)\[\/email\]/gi, "[$1](mailto:$1)")
    .replace(/\[img(?:=[^\]]*|\s[^\]]*)?\]([\s\S]*?)\[\/img\]/gi, "![]($1)")
    .replace(/\[media=([a-z0-9_-]+)\]([\s\S]*?)\[\/media\]/gi, (_, site, id) => (MEDIA[site.toLowerCase()] ? MEDIA[site.toLowerCase()](id.trim()) : id.trim()))
    .replace(/\[hr\]/gi, "\n---\n")
    // [ATTACH], [ATTACH=full], [ATTACH type="full" alt="x"]: the id is the text between the tags.
    .replace(/\[attach(?:[=\s][^\]]*)?\]\s*(\d+)\s*\[\/attach\]/gi, (_, id) => {
      const a = byId.get(id);
      if (!a) return "";
      used.add(id);
      return link(a);
    });
  t = t.replace(/\u0000(\d+)\u0000/g, (_, i) => held[Number(i)]);
  // XenForo shows every line break: outside code a single newline becomes a hard break.
  t = t
    .split(/(\n```[a-z0-9+#-]*\n[\s\S]*?\n```\n)/)
    .map((part, i) => (i % 2 ? part : part.replace(/([^\n])\n(?!\n)/g, "$1  \n")))
    .join("");
  t = t.replace(/\n{3,}/g, "\n\n").trim();
  for (const a of attachments) if (!used.has(String(a.attachment_id))) t += `\n\n${link(a)}`;
  return t;
}

// ---- members --------------------------------------------------------------------------------------------------------
const BCRYPT = /\$2[aby]\$\d\d\$[./A-Za-z0-9]{53}/;
const hashes = new Map();
for (const r of await optional("passwords", () => q(`SELECT user_id, data FROM ${p}user_authenticate`))) {
  const m = BCRYPT.exec(Buffer.isBuffer(r.data) ? r.data.toString("latin1") : String(r.data ?? ""));
  if (m) hashes.set(r.user_id, m[0]);
}
const users = await q(`SELECT user_id, username, email, register_date, avatar_date FROM ${p}user WHERE email <> '' AND user_state NOT IN ('rejected', 'disabled')`);
const known = new Set(users.map((u) => u.user_id));
// When each member was last on the old site (unix time or a date); a table or column this version lacks gives none.
const seen = new Map((await optional("last visits", () => q(`SELECT user_id AS id, last_activity AS t FROM ${p}user`))).map((r) => [r.id, r.t]));
counts.members = await out.section("users", users.map((u) => ({ id: u.user_id, username: u.username, email: u.email, password_hash: hashes.get(u.user_id) ?? null, joined_at: iso(u.register_date), last_seen_at: iso(seen.get(u.user_id) || null) })));

// ---- who can see which forum ----------------------------------------------------------------------------------------
const nodes = await q(`SELECT node_id, parent_node_id, title, display_order, node_type_id FROM ${p}node WHERE node_type_id IN ('Category', 'Forum') ORDER BY display_order, node_id`);
const byNode = new Map(nodes.map((n) => [n.node_id, n]));
// Groups for which `view` is Never on a node: 1 = Unregistered, 2 = Registered.
const never = new Map();
for (const r of await optional("forum permissions", () => q(`SELECT user_group_id, content_id, permission_value FROM ${p}permission_entry_content WHERE content_type = 'node' AND permission_group_id = 'forum' AND permission_id = 'view' AND user_id = 0 AND user_group_id IN (1, 2)`))) {
  if (r.permission_value !== "reset") continue;
  if (!never.has(r.content_id)) never.set(r.content_id, new Set());
  never.get(r.content_id).add(r.user_group_id);
}
/** Whether the group is shut out of the node by a Never on it or on a node above. */
const hiddenFrom = (gid, node) => {
  for (let n = node, guard = 0; n && guard < 50; n = byNode.get(n.parent_node_id), guard++) if (never.get(n.node_id)?.has(gid)) return true;
  return false;
};
const children = (id) => nodes.filter((n) => n.parent_node_id === id);
const readable = (n) => (n.node_type_id === "Category" ? children(n.node_id).some(readable) : !(hiddenFrom(1, n) && hiddenFrom(2, n)));
const shownForums = nodes.filter((n) => readable(n) && (!n.parent_node_id || byNode.has(n.parent_node_id)));
const shownIds = new Set(shownForums.map((n) => n.node_id));

// ---- files ----------------------------------------------------------------------------------------------------------
const attachmentRows = await optional("attachments", () =>
  q(
    `SELECT a.attachment_id, a.content_id AS post_id, a.data_id, d.user_id, d.filename, d.file_hash
     FROM ${p}attachment a JOIN ${p}attachment_data d ON d.data_id = a.data_id
     WHERE a.content_type = 'post' ORDER BY a.attachment_id`,
  ),
);
const byPost = new Map();
for (const a of attachmentRows) {
  if (!byPost.has(a.post_id)) byPost.set(a.post_id, []);
  byPost.get(a.post_id).push(a);
}
// Paths are relative to the XenForo folder, which is what BF_IMPORT_FILES points at.
const avatars = users
  .filter((u) => u.avatar_date > 0)
  .map((u) => ({ id: 1_000_000 + u.user_id, refs: [], path: `data/avatars/o/${Math.floor(u.user_id / 1000)}/${u.user_id}.jpg`, name: "avatar", user_id: u.user_id, avatar_of: u.user_id }));
counts.files = await out.section("uploads", [
  ...avatars,
  ...attachmentRows.map((a) => ({
    id: a.attachment_id,
    refs: [`xf${a.attachment_id}`],
    path: `${ATTACH_DIR}/${Math.floor(a.data_id / 1000)}/${a.data_id}-${a.file_hash}.data`,
    name: a.filename,
    user_id: known.has(a.user_id) ? a.user_id : null,
    avatar_of: null,
  })),
]);

// ---- forums, topics, posts ------------------------------------------------------------------------------------------
counts.forums = await out.section(
  "forums",
  shownForums.map((n) => ({ id: n.node_id, parent_id: n.parent_node_id || null, title: n.title, kind: n.node_type_id === "Category" ? "category" : "forum", ...(n.node_type_id === "Category" ? { public: true } : hiddenFrom(1, n) ? { public: false } : {}), position: n.display_order })),
);
const topics = (await q(`SELECT thread_id, node_id, title, view_count, sticky, discussion_open FROM ${p}thread WHERE discussion_state = 'visible' AND discussion_type <> 'redirect'`)).filter((t) => shownIds.has(t.node_id));
const shown = new Set(topics.map((t) => t.thread_id));
counts.topics = await out.section("topics", topics.map((t) => ({ id: t.thread_id, forum_id: t.node_id, title: t.title, url: `/threads/${t.thread_id}/`, views: t.view_count, pinned: t.sticky === 1, locked: t.discussion_open === 0 })));

const shownPosts = new Set();
async function* posts() {
  const rows = pages((offset, size) => q(`SELECT post_id, thread_id, user_id, message, post_date FROM ${p}post WHERE message_state = 'visible' ORDER BY thread_id, post_date, post_id LIMIT ? OFFSET ?`, [size, offset]));
  for await (const x of rows) {
    if (!shown.has(x.thread_id)) continue;
    shownPosts.add(x.post_id);
    yield { id: x.post_id, topic_id: x.thread_id, author_id: known.has(x.user_id) ? x.user_id : null, body: toMarkdown(x.message, byPost.get(x.post_id) ?? []), format: "markdown", created_at: iso(x.post_date) };
  }
}
counts.posts = await out.section("posts", posts());

// ---- tags, groups, polls, reactions, conversations -----------------------------------------------------------------
const tags = await optional("tags", async () => {
  const rows = await q(`SELECT c.content_id, t.tag FROM ${p}tag_content c JOIN ${p}tag t ON t.tag_id = c.tag_id WHERE c.content_type = 'thread'`);
  return rows.filter((r) => shown.has(r.content_id)).map((r) => ({ topic_id: r.content_id, name: r.tag }));
});
counts.tags = await out.section("tags", tags);

// Custom groups (XenForo's own are 1 to 4); a member is in one by their main group or a secondary one.
const custom = await optional("groups", async () => {
  const gs = await q(`SELECT user_group_id, title FROM ${p}user_group WHERE user_group_id > 4`);
  const result = [];
  for (const g of gs) {
    const members = await q(`SELECT user_id FROM ${p}user WHERE user_group_id = ? OR FIND_IN_SET(?, secondary_group_ids)`, [g.user_group_id, g.user_group_id]);
    result.push({ id: g.user_group_id, name: g.title, members: members.map((m) => m.user_id) });
  }
  return result;
});
counts.groups = await out.section("groups", custom);

const polls = await optional("polls", async () => {
  const rows = await q(`SELECT * FROM ${p}poll WHERE content_type = 'thread'`);
  const result = [];
  for (const pl of rows) {
    if (!shown.has(pl.content_id)) continue;
    const responses = await q(`SELECT poll_response_id, response FROM ${p}poll_response WHERE poll_id = ? ORDER BY poll_response_id`, [pl.poll_id]);
    if (responses.length < 2) continue;
    const index = new Map(responses.map((r, i) => [r.poll_response_id, i]));
    const votes = (await q(`SELECT user_id, poll_response_id FROM ${p}poll_vote WHERE poll_id = ?`, [pl.poll_id])).filter((v) => index.has(v.poll_response_id)).map((v) => ({ user_id: v.user_id, option: index.get(v.poll_response_id) }));
    // XenForo 2.0 has `multiple`; later versions `max_votes` (0: any number of options).
    const max = "max_votes" in pl ? pl.max_votes || responses.length : pl.multiple ? responses.length : 1;
    result.push({
      id: pl.poll_id,
      topic_id: pl.content_id,
      question: pl.question,
      options: responses.map((r) => r.response),
      max_choices: max,
      closes_at: pl.close_date > 0 ? iso(pl.close_date) : null,
      public: pl.public_votes === 1,
      results: pl.view_results_unvoted === 0 ? "voted" : "always",
      final_votes: pl.change_vote_allowed === 0,
      votes,
    });
  }
  return result;
});
counts.polls = await out.section("polls", polls);

// Likes: XenForo 2.1+ keeps reactions in `reaction_content`, older versions likes in `liked_content`; each becomes a like.
async function* reactions() {
  const seen = new Set();
  for (const [table, content, user] of [["reaction_content", "content_type", "reaction_user_id"], ["liked_content", "content_type", "like_user_id"]]) {
    const rows = await optional("reactions", async () => {
      const result = [];
      for await (const r of pages((offset, size) => q(`SELECT content_id, ${user} AS user_id FROM ${p}${table} WHERE ${content} = 'post' ORDER BY content_id, ${user} LIMIT ? OFFSET ?`, [size, offset]))) result.push(r);
      return result;
    });
    for (const r of rows) {
      const key = `${r.content_id}|${r.user_id}`;
      if (!shownPosts.has(r.content_id) || !known.has(r.user_id) || seen.has(key)) continue;
      seen.add(key);
      yield { post_id: r.content_id, user_id: r.user_id };
    }
  }
}
counts.reactions = await out.section("reactions", reactions());

// A conversation is its master row, the people in `conversation_recipient` and its messages.
async function* conversations() {
  const people = new Map();
  for (const r of await optional("conversations", () => q(`SELECT conversation_id, user_id FROM ${p}conversation_recipient`))) {
    if (!people.has(r.conversation_id)) people.set(r.conversation_id, new Set());
    if (known.has(r.user_id)) people.get(r.conversation_id).add(r.user_id);
  }
  const masters = new Map((await optional("conversations", () => q(`SELECT conversation_id, title, user_id FROM ${p}conversation_master`))).map((c) => [c.conversation_id, c]));
  let current = null;
  const done = function* () {
    if (current && current.participants.length >= 2 && current.messages.length) yield current;
  };
  const rows = pages((offset, size) => q(`SELECT message_id, conversation_id, user_id, message, message_date FROM ${p}conversation_message ORDER BY conversation_id, message_id LIMIT ? OFFSET ?`, [size, offset]));
  for await (const m of rows) {
    if (current?.cid !== m.conversation_id) {
      yield* done();
      const master = masters.get(m.conversation_id);
      const set = new Set(people.get(m.conversation_id) ?? []);
      if (master && known.has(master.user_id)) set.add(master.user_id);
      current = { cid: m.conversation_id, id: m.conversation_id, subject: master?.title ?? "", participants: [...set], messages: [] };
    }
    current.messages.push({ author_id: known.has(m.user_id) ? m.user_id : null, body: toMarkdown(m.message), format: "markdown", created_at: iso(m.message_date) });
  }
  yield* done();
}
counts.messages = await out.section(
  "messages",
  (async function* () {
    for await (const c of conversations()) yield { id: c.id, subject: c.subject, participants: c.participants, messages: c.messages };
  })(),
);

await out.end();
await db.end();
console.log(`Wrote ${Object.entries(counts).map(([k, n]) => `${n} ${k}`).join(", ")}.`);
