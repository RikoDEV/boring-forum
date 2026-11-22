// Writes a phpBB 3.x forum to the importer's JSON format:
//   npm i mysql2
//   PHPBB_DB="mysql://user:pass@host/dbname" node tools/importers/export-phpbb.mjs out.json
//   BF_IMPORT_FILES=/path/to/phpBB BF_IMPORT_PUBLIC= boring-forum import out.json
// Reads only. Set PHPBB_PREFIX if the tables do not use phpbb_. Give the importer the phpBB folder (the one with
// `files/` and `images/`) as BF_IMPORT_FILES: attachments and uploaded avatars are copied from there, and
// the pictures in posts show. Without it they are skipped.
// Exports members and their avatars, forums (in the board's order; those staff alone can read are left out, and
// so are their topics), topics (views, sticky, announcement, locked), posts, attachments, groups, polls and
// private messages. phpBB has no tags or reactions in core.
// Posts: phpBB 3.2 and later store them as XML (`<r><B><s>[b]</s>…`), which is converted to Markdown here;
// older boards store BBCode with a per-post id in each tag, which the importer converts.
// Passwords: bcrypt ($2y$), argon2 (phpBB 3.3) and the older portable hash ($H$) are all kept and checked at
// the member's next sign-in, when they are replaced; anything else (phpBB 2 md5, $CP$ conversions)
// is dropped and that member resets the password.
import { createRequire } from "node:module";
import { Out, iso, optional, pages } from "./lib.mjs";
const require = createRequire(`${process.cwd()}/`);
const mysql = require("mysql2/promise");
const db = await mysql.createConnection(process.env.PHPBB_DB);
const p = process.env.PHPBB_PREFIX ?? "phpbb_";
const q = async (sql, args = []) => (await db.query(sql, args))[0];
const out = new Out(process.argv[2]);
const counts = {};

// ---- post text ------------------------------------------------------------------------------------------------------
// phpBB 3.0 and 3.1 store BBCode with a per-post id inside each tag: [b:abc123]. Removing it gives plain BBCode.
const oldBBCode = (t) => t.replace(/\[(\/?[a-z*]+(?:=[^\]:]*)?):[a-z0-9]+\]/gi, "[$1]").replace(/<!--.*?-->/g, "");
const decode = (s) =>
  s.replace(/&#x([0-9a-f]+);/gi, (_, h) => String.fromCodePoint(parseInt(h, 16))).replace(/&#(\d+);/g, (_, d) => String.fromCodePoint(+d)).replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&quot;/g, '"').replace(/&apos;/g, "'").replace(/&amp;/g, "&");
const COLORS = { red: "red", orange: "orange", yellow: "yellow", green: "green", blue: "blue", purple: "purple", violet: "purple", gray: "gray", grey: "gray" };
const IMAGE = /\.(png|jpe?g|gif|webp)$/i;
const LI = ["\u0001", "\u0002"];

/** One post of the XML format as Markdown. `attachments` are the post's files in order; each `[attachment=n]` becomes a link to its
 *  file (`upload://pb<id>`, which the importer swaps for the stored copy) and the rest follow the text, as phpBB lists them. */
function xmlToMarkdown(xml, attachments = []) {
  const used = new Set();
  const place = (index) => {
    const a = attachments[Number(index)];
    if (!a) return "";
    used.add(a.attach_id);
    return `${IMAGE.test(a.real_filename) ? "!" : ""}[${a.real_filename.replace(/[[\]]/g, " ")}](upload://pb${a.attach_id})`;
  };
  const render = (tag, a, inner) => {
    switch (tag) {
      case "B": return `**${inner}**`;
      case "I": return `*${inner}*`;
      case "U": return `++${inner}++`;
      case "COLOR": return COLORS[(a.color ?? "").toLowerCase()] ? `[${inner}]{${COLORS[a.color.toLowerCase()]}}` : inner;
      case "QUOTE": {
        const lines = inner.trim().split("\n").map((l) => `> ${l}`.trimEnd());
        return `\n${a.author ? `> **${decode(a.author)}:**\n` : ""}${lines.join("\n")}\n\n`;
      }
      case "URL": {
        const text = inner.trim();
        return text === a.url || /\.\.\./.test(text) || !text ? a.url : `[${text}](${a.url})`;
      }
      case "IMG": return `![](${a.src})`;
      case "EMAIL": return `[${inner.trim()}](mailto:${a.email})`;
      case "CODE": return `\n\`\`\`\n${inner.replace(/^\n|\n$/g, "")}\n\`\`\`\n`;
      case "LI": return `${LI[0]}${inner.trim()}${LI[1]}`;
      case "LIST": {
        let n = 0;
        const items = [...inner.matchAll(/\u0001([\s\S]*?)\u0002/g)].map((m) => `${a.type === "decimal" || a.type === "1" ? `${++n}.` : "-"} ${m[1]}`);
        return `\n${items.join("\n")}\n\n`;
      }
      case "ATTACHMENT": return place(a.index);
      case "s": case "e": return "";
      default: return inner; // SIZE, FONT, E (smilies keep their text), LINK_TEXT and anything unknown
    }
  };
  const stack = [{ tag: "", a: {}, text: "" }];
  let afterBreak = false;
  for (const m of xml.matchAll(/<(\/?)([A-Za-z][A-Za-z0-9]*)((?:\s+[A-Za-z_:-]+="[^"]*")*)\s*(\/?)>|([^<]+)/g)) {
    const [, closing, tag, attrs, selfClosing, text] = m;
    const top = stack[stack.length - 1];
    if (text !== undefined) {
      let t = decode(text);
      if (afterBreak && t.startsWith("\n")) t = t.slice(1);
      afterBreak = false;
      top.text += t;
    } else if (tag === "br") {
      top.text += "  \n";
      afterBreak = true;
    } else if (closing) {
      if (stack.length > 1) {
        const f = stack.pop();
        stack[stack.length - 1].text += render(f.tag, f.a, f.text);
      }
    } else {
      const a = Object.fromEntries([...(attrs ?? "").matchAll(/([A-Za-z_:-]+)="([^"]*)"/g)].map(([, k, v]) => [k, decode(v)]));
      if (selfClosing) top.text += render(tag, a, "");
      else stack.push({ tag, a, text: "" });
    }
  }
  while (stack.length > 1) {
    const f = stack.pop();
    stack[stack.length - 1].text += render(f.tag, f.a, f.text);
  }
  let md = stack[0].text.replace(/\n{3,}/g, "\n\n").trim();
  for (const a of attachments) if (!used.has(a.attach_id)) md += `\n\n${IMAGE.test(a.real_filename) ? "!" : ""}[${a.real_filename.replace(/[[\]]/g, " ")}](upload://pb${a.attach_id})`;
  return md;
}

/** `{ body, format }` for a stored post or message. */
const text = (stored, attachments = []) => (/^<[rt]\b/.test(stored) ? { body: xmlToMarkdown(stored, attachments), format: "markdown" } : { body: oldBBCode(stored), format: "bbcode" });

// ---- who can read what ---------------------------------------------------------------------------------------------
const group = async (name) => (await q(`SELECT group_id FROM ${p}groups WHERE group_name = ?`, [name]))[0]?.group_id;
const [GUESTS, REGISTERED] = [await group("GUESTS"), await group("REGISTERED")];
const read = (await optional("permissions", () => q(`SELECT auth_option_id FROM ${p}acl_options WHERE auth_option = 'f_read'`)))[0]?.auth_option_id;
const grants = await optional("permissions", () => q(`SELECT group_id, forum_id, auth_option_id, auth_role_id, auth_setting FROM ${p}acl_groups WHERE forum_id > 0 AND group_id IN (?, ?)`, [GUESTS, REGISTERED]));
const roles = new Set((await optional("roles", () => q(`SELECT role_id FROM ${p}acl_roles_data WHERE auth_option_id = ? AND auth_setting = 1`, [read]))).map((r) => r.role_id));
/** Whether the group may read the forum: the permission set on the forum itself, directly or through a role. */
const canRead = (groupId, forumId) => {
  let yes = false;
  for (const g of grants.filter((g) => g.group_id === groupId && g.forum_id === forumId)) {
    if (g.auth_option_id === read) {
      if (g.auth_setting === 0) return false; // NEVER
      yes ||= g.auth_setting === 1;
    } else if (g.auth_role_id && roles.has(g.auth_role_id)) yes = true;
  }
  return yes;
};

// ---- members and files ----------------------------------------------------------------------------------------------
const users = await q(`SELECT user_id, username, user_email, user_password, user_regdate, user_avatar, user_avatar_type FROM ${p}users WHERE user_type <> 2 AND user_email <> ''`);
const known = new Set(users.map((u) => u.user_id));
const salt = (await optional("config", () => q(`SELECT config_value FROM ${p}config WHERE config_name = 'avatar_salt'`)))[0]?.config_value;
// When each member was last on the old site (unix time or a date); a table or column this version lacks gives none.
const seen = new Map((await optional("last visits", () => q(`SELECT user_id AS id, user_lastvisit AS t FROM ${p}users`))).map((r) => [r.id, r.t]));
counts.members = await out.section("users", users.map((u) => ({ id: u.user_id, username: u.username, email: u.user_email, password_hash: /^\$(2[abxy]\$|H\$|P\$|argon2)/.test(u.user_password) ? u.user_password : null, joined_at: iso(u.user_regdate), last_seen_at: iso(seen.get(u.user_id) || null) })));

const attachmentRows = await optional("attachments", () => q(`SELECT attach_id, post_msg_id, poster_id, physical_filename, real_filename FROM ${p}attachments WHERE in_message = 0 AND is_orphan = 0 ORDER BY attach_id`));
const byPost = new Map();
for (const a of attachmentRows) {
  if (!byPost.has(a.post_msg_id)) byPost.set(a.post_msg_id, []);
  byPost.get(a.post_msg_id).push(a);
}
// Paths are relative to the phpBB folder, which is what BF_IMPORT_FILES points at.
const avatars = users.filter((u) => u.user_avatar && /upload/i.test(String(u.user_avatar_type)) && salt).map((u) => ({ id: 1_000_000 + u.user_id, refs: [], path: `images/avatars/upload/${salt}_${u.user_id}.${u.user_avatar.split(".").pop()}`, name: `avatar.${u.user_avatar.split(".").pop()}`, user_id: u.user_id, avatar_of: u.user_id }));
counts.files = await out.section("uploads", [
  ...avatars,
  ...attachmentRows.map((a) => ({ id: a.attach_id, refs: [`pb${a.attach_id}`], path: `files/${a.physical_filename}`, name: a.real_filename, user_id: known.has(a.poster_id) ? a.poster_id : null, avatar_of: null })),
]);

// ---- forums, in the board's order ----------------------------------------------------------------------------------
const forumRows = await q(`SELECT forum_id, parent_id, forum_name, forum_type, left_id FROM ${p}forums ORDER BY left_id`);
const children = (id) => forumRows.filter((f) => f.parent_id === id);
const visible = (f) => (f.forum_type === 0 ? children(f.forum_id).some(visible) : canRead(REGISTERED, f.forum_id) || canRead(GUESTS, f.forum_id));
const shownForums = forumRows.filter(visible);
const shownIds = new Set(shownForums.map((f) => f.forum_id));
// A forum under one that is left out would have no parent: it stays out too.
counts.forums = await out.section(
  "forums",
  shownForums.filter((f) => !f.parent_id || shownIds.has(f.parent_id)).map((f) => ({ id: f.forum_id, parent_id: f.parent_id || null, title: f.forum_name, kind: f.forum_type === 0 ? "category" : "forum", public: f.forum_type === 0 ? true : canRead(GUESTS, f.forum_id), position: f.left_id })),
);

const topics = (await q(`SELECT topic_id, forum_id, topic_title, topic_views, topic_type, topic_status FROM ${p}topics WHERE topic_visibility = 1 AND topic_moved_id = 0`)).filter((t) => shownIds.has(t.forum_id));
const shown = new Set(topics.map((t) => t.topic_id));
counts.topics = await out.section("topics", topics.map((t) => ({ id: t.topic_id, forum_id: t.forum_id, title: t.topic_title, url: `/viewtopic.php?t=${t.topic_id}`, views: t.topic_views, pinned: t.topic_type === 1, announcement: t.topic_type >= 2, locked: t.topic_status === 1 })));

async function* posts() {
  const rows = pages((offset, size) => q(`SELECT post_id, topic_id, poster_id, post_text, post_time FROM ${p}posts WHERE post_visibility = 1 ORDER BY topic_id, post_time, post_id LIMIT ? OFFSET ?`, [size, offset]));
  for await (const x of rows) {
    if (!shown.has(x.topic_id)) continue;
    yield { id: x.post_id, topic_id: x.topic_id, author_id: known.has(x.poster_id) ? x.poster_id : null, ...text(x.post_text, byPost.get(x.post_id) ?? []), created_at: iso(x.post_time) };
  }
}
counts.posts = await out.section("posts", posts());

// ---- groups, polls, private messages -------------------------------------------------------------------------------
// User-made groups only (group_type 3 is special: guests, registered, bots, admins, moderators).
const groups = await optional("groups", async () => {
  const gs = await q(`SELECT group_id, group_name FROM ${p}groups WHERE group_type <> 3`);
  const result = [];
  for (const g of gs) {
    const members = await q(`SELECT user_id FROM ${p}user_group WHERE group_id = ? AND user_pending = 0`, [g.group_id]);
    result.push({ id: g.group_id, name: g.group_name, members: members.map((m) => m.user_id) });
  }
  return result;
});
counts.groups = await out.section("groups", groups);

const polls = await optional("polls", async () => {
  const rows = await q(`SELECT topic_id, poll_title, poll_max_options, poll_length, poll_start, poll_vote_change FROM ${p}topics WHERE poll_title <> '' AND topic_visibility = 1 AND topic_moved_id = 0`);
  const result = [];
  for (const t of rows) {
    if (!shown.has(t.topic_id)) continue;
    const options = await q(`SELECT poll_option_id, poll_option_text FROM ${p}poll_options WHERE topic_id = ? ORDER BY poll_option_id`, [t.topic_id]);
    if (options.length < 2) continue;
    const index = new Map(options.map((o, i) => [o.poll_option_id, i]));
    const votes = (await q(`SELECT vote_user_id, poll_option_id FROM ${p}poll_votes WHERE topic_id = ?`, [t.topic_id])).filter((v) => index.has(v.poll_option_id)).map((v) => ({ user_id: v.vote_user_id, option: index.get(v.poll_option_id) }));
    result.push({ id: t.topic_id, topic_id: t.topic_id, question: t.poll_title, options: options.map((o) => o.poll_option_text), max_choices: t.poll_max_options || 1, closes_at: t.poll_length ? iso(t.poll_start + t.poll_length) : null, final_votes: !t.poll_vote_change, votes });
  }
  return result;
});
counts.polls = await out.section("polls", polls);

// A conversation is a message and the replies to it (`root_level` names the first).
const conversations = await optional("messages", async () => {
  const rows = await q(`SELECT msg_id, root_level, message_subject, message_text, message_time, author_id FROM ${p}privmsgs ORDER BY message_time, msg_id`);
  const threads = new Map();
  for (const m of rows) {
    const id = m.root_level || m.msg_id;
    if (!threads.has(id)) threads.set(id, { id, subject: m.message_subject, people: new Set(), messages: [] });
    const t = threads.get(id);
    t.people.add(m.author_id);
    for (const to of await q(`SELECT DISTINCT user_id FROM ${p}privmsgs_to WHERE msg_id = ?`, [m.msg_id])) t.people.add(to.user_id);
    t.messages.push({ author_id: m.author_id, ...text(m.message_text), created_at: iso(m.message_time) });
  }
  return [...threads.values()].map((t) => ({ id: t.id, subject: t.subject, participants: [...t.people], messages: t.messages }));
});
counts.messages = await out.section("messages", conversations);

await out.end();
await db.end();
console.log(`Wrote ${Object.entries(counts).map(([k, n]) => `${n} ${k}`).join(", ")}.`);
