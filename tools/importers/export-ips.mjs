// Writes an IPS 4.x/5.x forum to the importer's JSON format:
//   npm i mysql2
//   IPS_DB="mysql://user:pass@host/dbname" node tools/importers/export-ips.mjs out.json
//   boring-forum import out.json
// Reads only; run it from a directory where mysql2 is installed. IPS keeps forum names in its
// language tables, so those are joined. Set IPS_PREFIX if the tables have a prefix, and
// IPS_UPLOADS to the address the old uploads are served from (default /uploads), which becomes the
// base of attachment links. Exports members, forums, topics, posts (in pages), tags, groups, polls
// (first question of each), likes, private messages and attachment references. Sections an
// installation does not have are skipped with a note.
// Passwords stay as they are: IPS 4 hashes are bcrypt, and members sign in with their old
// password, which is upgraded then. IPS 3 hashes (md5 with a salt) are carried as ips3$salt$hash.
import { createRequire } from "node:module";
import { Out, iso, optional, pages } from "./lib.mjs";
const require = createRequire(`${process.cwd()}/`);
const mysql = require("mysql2/promise");
const db = await mysql.createConnection(process.env.IPS_DB);
const p = process.env.IPS_PREFIX ?? "";
const uploads = (process.env.IPS_UPLOADS ?? "/uploads").replace(/\/$/, "");
const q = async (sql, args = []) => (await db.query(sql, args))[0];
const json = (s) => {
  try {
    return JSON.parse(s);
  } catch {
    return null;
  }
};

const out = new Out(process.argv[2]);
const counts = {};
const hash = (m) => {
  const h = m.members_pass_hash || "";
  if (/^[0-9a-f]{32}$/i.test(h) && m.members_pass_salt) return `ips3$${m.members_pass_salt}$${h.toLowerCase()}`;
  return h || null;
};
const members = await q(`SELECT member_id, name, email, members_pass_hash, members_pass_salt, joined, member_group_id, mgroup_others FROM ${p}core_members WHERE email <> ''`);
counts.members = await out.section("users", members.map((m) => ({ id: m.member_id, username: m.name, email: m.email, password_hash: hash(m), joined_at: iso(m.joined) })));

const forums = await q(`SELECT f.id, f.parent_id, COALESCE(l.word_custom, l.word_default, CONCAT('Forum ', f.id)) AS title
  FROM ${p}forums_forums f LEFT JOIN ${p}core_sys_lang_words l ON l.word_key = CONCAT('forums_forum_', f.id) AND l.lang_id = (SELECT MIN(lang_id) FROM ${p}core_sys_lang)`);
const hasChildren = new Set(forums.map((f) => f.parent_id));
counts.forums = await out.section("forums", forums.map((f) => ({ id: f.id, parent_id: f.parent_id > 0 ? f.parent_id : null, title: f.title, kind: f.parent_id <= 0 && hasChildren.has(f.id) ? "category" : "forum" })));

const topics = await q(`SELECT tid, forum_id, starter_id, title, title_seo FROM ${p}forums_topics WHERE approved = 1 AND moved_to IS NULL`);
counts.topics = await out.section("topics", topics.map((t) => ({ id: t.tid, forum_id: t.forum_id, author_id: t.starter_id || null, title: t.title, url: `/topic/${t.tid}-${t.title_seo}/` })));

// Attachments belong to posts: collected first, then written as references on each post.
const attachments = new Map();
for (const a of await optional("attachments", () => q(`SELECT m.id2 AS pid, a.attach_file, a.attach_location FROM ${p}core_attachments_map m JOIN ${p}core_attachments a ON a.attach_id = m.attachment_id WHERE m.location_key = 'forums_Forums'`))) {
  if (!attachments.has(a.pid)) attachments.set(a.pid, []);
  attachments.get(a.pid).push({ name: a.attach_file, url: `${uploads}/${a.attach_location}` });
}
const postRows = pages((offset, size) => q(`SELECT pid, topic_id, author_id, post, post_date FROM ${p}forums_posts WHERE queued = 0 ORDER BY topic_id, new_topic DESC, post_date, pid LIMIT ? OFFSET ?`, [size, offset]));
async function* posts() {
  for await (const x of postRows) {
    yield { id: x.pid, topic_id: x.topic_id, author_id: x.author_id || null, body: x.post, format: "html", created_at: iso(x.post_date), attachments: attachments.get(x.pid) ?? [] };
  }
}
counts.posts = await out.section("posts", posts());

counts.tags = await out.section("tags", (await optional("tags", () => q(`SELECT tag_meta_id AS topic_id, tag_text AS name FROM ${p}core_tags WHERE tag_meta_app = 'forums'`))));

const names = new Map((await optional("group names", () => q(`SELECT word_key, COALESCE(word_custom, word_default) AS w FROM ${p}core_sys_lang_words WHERE word_key LIKE 'core_group_%' AND lang_id = (SELECT MIN(lang_id) FROM ${p}core_sys_lang)`))).map((r) => [r.word_key, r.w]));
const byGroup = new Map();
for (const m of members) {
  for (const g of [m.member_group_id, ...String(m.mgroup_others ?? "").split(",").map(Number)].filter((x) => x > 0)) {
    if (!byGroup.has(g)) byGroup.set(g, []);
    byGroup.get(g).push(m.member_id);
  }
}
counts.groups = await out.section("groups", [...byGroup].filter(([g]) => names.has(`core_group_${g}`)).map(([g, users]) => ({ id: g, name: names.get(`core_group_${g}`), members: users })));

// IPS polls: `choices` is JSON {"1": {question, choice: [..], multi}}; votes keep their picks as JSON.
const polls = await optional("polls", async () => {
  const rows = await q(`SELECT t.tid, o.pid, o.poll_question, o.choices FROM ${p}core_polls o JOIN ${p}forums_topics t ON t.poll_state = o.pid`);
  const result = [];
  for (const r of rows) {
    const choices = json(r.choices);
    const first = choices && Object.values(choices)[0];
    if (!first || !Array.isArray(first.choice) || first.choice.length < 2) continue;
    const key = Object.keys(choices)[0];
    const votes = [];
    for (const v of await q(`SELECT member_id, member_choices FROM ${p}core_voters WHERE poll = ?`, [r.pid])) {
      const picks = json(v.member_choices)?.[key];
      for (const i of Array.isArray(picks) ? picks : picks === undefined ? [] : [picks]) votes.push({ user_id: v.member_id, option: Number(i) });
    }
    result.push({ id: r.pid, topic_id: r.tid, question: first.question || r.poll_question, options: first.choice, max_choices: first.multi ? first.choice.length : 1, votes });
  }
  return result;
});
counts.polls = await out.section("polls", polls);

counts.reactions = await out.section("reactions", pages((offset, size) => optional("reactions", () => q(`SELECT type_id AS post_id, member_id AS user_id FROM ${p}core_reputation_index WHERE app = 'forums' AND type = 'pid' AND rep_rating > 0 ORDER BY id LIMIT ? OFFSET ?`, [size, offset]))));

const conversations = await optional("messages", async () => {
  const topics = await q(`SELECT mt_id, mt_title FROM ${p}core_message_topics WHERE mt_is_deleted = 0 OR mt_is_deleted IS NULL`);
  const result = [];
  for (const t of topics) {
    const people = await q(`SELECT map_user_id FROM ${p}core_message_topic_user_map WHERE map_topic_id = ?`, [t.mt_id]);
    const msgs = await q(`SELECT msg_author_id, msg_post, msg_date FROM ${p}core_message_posts WHERE msg_topic_id = ? ORDER BY msg_date, msg_id`, [t.mt_id]);
    result.push({ id: t.mt_id, subject: t.mt_title, participants: people.map((x) => x.map_user_id), messages: msgs.map((m) => ({ author_id: m.msg_author_id, body: m.msg_post, format: "html", created_at: iso(m.msg_date) })) });
  }
  return result;
});
counts.messages = await out.section("messages", conversations);

await out.end();
await db.end();
console.log(`Wrote ${Object.entries(counts).map(([k, n]) => `${n} ${k}`).join(", ")}.`);
