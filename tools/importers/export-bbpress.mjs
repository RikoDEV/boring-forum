// Writes a bbPress 2.x forum (a WordPress plugin) to the importer's JSON format:
//   npm i mysql2
//   BBPRESS_DB="mysql://user:pass@host/dbname" node tools/importers/export-bbpress.mjs out.json
//   boring-forum import out.json
// Reads only. Set BBPRESS_PREFIX if the tables do not use wp_ (a multisite blog uses wp_<n>_).
// Written from bbPress's schema, not tried against a live install: what varies between versions (view counts,
// sticky lists, topic tags) is read through `optional`, so a missing one only skips that part.
// Exports members (every WordPress user with an email), forums (published ones, nested as in bbPress; private and
// hidden forums are left out with their topics, and categories keep their type), topics (closed, stuck and
// "super sticky" ones, which are shown in every forum; views when a plugin keeps `_bbp_view_count`), the topic's own
// text as the first post and then its replies, and topic tags. Spam, trashed, pending and draft posts are left out.
// bbPress core has no polls, reactions, private messages, groups or attachments, so those sections are not written.
// Posts: the text is HTML as WordPress shows it: paragraphs and line breaks are rebuilt the way WordPress's
// `wpautop` does, and the importer sanitises the rest.
// Passwords: WordPress's phpass hashes (`$P$…`) are the same scheme as phpBB 3.0's (`$H$…`), and are carried over
// with that prefix; the importer checks them at the member's next sign-in and replaces them. Other hashes (WordPress
// 6.8's `$wp$2y$…`, plugins' own) are dropped, and that member resets the password.
import { createRequire } from "node:module";
import { Out, iso, optional, pages } from "./lib.mjs";
const require = createRequire(`${process.cwd()}/`);
const mysql = require("mysql2/promise");
// Dates as text: WordPress writes 0000-00-00 for posts that have none, which a Date cannot hold.
const db = await mysql.createConnection({ uri: process.env.BBPRESS_DB, dateStrings: true });
const p = process.env.BBPRESS_PREFIX ?? "wp_";
const q = async (sql, args = []) => (await db.query(sql, args))[0];
const out = new Out(process.argv[2]);
const counts = {};

/** A `*_gmt` or registration time (UTC, as text) in RFC 3339; nothing for WordPress's zero date. */
const gmt = (s) => (s && !s.startsWith("0000") ? iso(`${s.replace(" ", "T")}Z`) : null);

// ---- post text ------------------------------------------------------------------------------------------------------
const BLOCK = /^<(p|pre|ul|ol|blockquote|h[1-6]|div|table|hr)[\s>/]/i;
/** WordPress stores what was typed and adds paragraphs when it shows it: blank lines separate paragraphs, a single
 *  line break becomes <br />. Code blocks are set aside first, so their lines are kept as they are. */
function autop(source) {
  const held = [];
  let t = source.replace(/\r\n?/g, "\n").replace(/<pre[\s\S]*?<\/pre>/gi, (m) => {
    held.push(m);
    return `\u0000${held.length - 1}\u0000`;
  });
  t = t
    .split(/\n\s*\n/)
    .map((chunk) => chunk.trim())
    .filter(Boolean)
    .map((chunk) => (BLOCK.test(chunk) ? chunk : `<p>${chunk.replace(/\n/g, "<br />\n")}</p>`))
    .join("\n");
  return t.replace(/\u0000(\d+)\u0000/g, (_, i) => held[Number(i)]);
}

// ---- members --------------------------------------------------------------------------------------------------------
const users = await q(`SELECT ID, user_login, user_email, user_pass, user_registered FROM ${p}users WHERE user_email <> ''`);
const known = new Set(users.map((u) => u.ID));
counts.members = await out.section(
  "users",
  users.map((u) => ({ id: u.ID, username: u.user_login, email: u.user_email, password_hash: /^\$P\$/.test(u.user_pass) ? `$H$${u.user_pass.slice(3)}` : null, joined_at: gmt(u.user_registered) })),
);

// ---- forums: published ones whose parents are published too ---------------------------------------------------------
const forumRows = await q(`SELECT ID, post_parent, post_title, menu_order, post_status FROM ${p}posts WHERE post_type = 'forum' AND post_status IN ('publish', 'private', 'hidden') ORDER BY menu_order, ID`);
const byId = new Map(forumRows.map((f) => [f.ID, f]));
const isPublic = (f, depth = 0) => f.post_status === "publish" && (!f.post_parent || (depth < 20 && byId.has(f.post_parent) && isPublic(byId.get(f.post_parent), depth + 1)));
const shownForums = forumRows.filter((f) => isPublic(f));
const shownIds = new Set(shownForums.map((f) => f.ID));
const types = new Map(
  (await optional("forum types", () => q(`SELECT post_id, meta_value FROM ${p}postmeta WHERE meta_key = '_bbp_forum_type'`))).map((m) => [m.post_id, m.meta_value]),
);
counts.forums = await out.section(
  "forums",
  shownForums.map((f) => ({ id: f.ID, parent_id: f.post_parent || null, title: f.post_title, kind: types.get(f.ID) === "category" ? "category" : "forum", public: true, position: f.menu_order })),
);

// ---- topics ---------------------------------------------------------------------------------------------------------
// Sticky lists are PHP arrays of topic ids: per forum in post meta, for every forum in an option.
const ids = (serialized) => [...String(serialized ?? "").matchAll(/i:\d+;i:(\d+);/g)].map((m) => Number(m[1]));
const sticky = new Set((await optional("sticky topics", () => q(`SELECT meta_value FROM ${p}postmeta WHERE meta_key = '_bbp_sticky_topics'`))).flatMap((m) => ids(m.meta_value)));
const superSticky = new Set(
  (await optional("super sticky topics", () => q(`SELECT option_value FROM ${p}options WHERE option_name IN ('_bbp_super_sticky_topics', '_bbp_sticky_topics')`))).flatMap((o) => ids(o.option_value)),
);
const views = new Map((await optional("views", () => q(`SELECT post_id, meta_value FROM ${p}postmeta WHERE meta_key = '_bbp_view_count'`))).map((m) => [m.post_id, Number(m.meta_value) || 0]));
const topics = (
  await q(
    `SELECT t.ID, t.post_title, t.post_name, t.post_status, CAST(COALESCE(m.meta_value, t.post_parent) AS UNSIGNED) AS forum
     FROM ${p}posts t LEFT JOIN ${p}postmeta m ON m.post_id = t.ID AND m.meta_key = '_bbp_forum_id'
     WHERE t.post_type = 'topic' AND t.post_status IN ('publish', 'closed') ORDER BY t.ID`,
  )
).filter((x) => shownIds.has(x.forum));
const shown = new Set(topics.map((x) => x.ID));
counts.topics = await out.section(
  "topics",
  topics.map((x) => ({
    id: x.ID,
    forum_id: x.forum,
    title: x.post_title,
    url: `/forums/topic/${x.post_name}/`,
    views: views.get(x.ID),
    pinned: sticky.has(x.ID) || superSticky.has(x.ID),
    announcement: superSticky.has(x.ID),
    locked: x.post_status === "closed",
  })),
);

// ---- posts: each topic's own text first, then its replies in order ---------------------------------------------------
async function* posts() {
  const rows = pages((offset, size) =>
    q(
      `SELECT x.ID, x.post_type, x.post_author, x.post_content, x.post_date_gmt,
              CAST(IF(x.post_type = 'topic', x.ID, COALESCE(m.meta_value, x.post_parent)) AS UNSIGNED) AS topic
       FROM ${p}posts x LEFT JOIN ${p}postmeta m ON m.post_id = x.ID AND m.meta_key = '_bbp_topic_id'
       WHERE x.post_type IN ('topic', 'reply') AND x.post_status IN ('publish', 'closed')
       ORDER BY topic, (x.post_type = 'reply'), x.post_date_gmt, x.ID LIMIT ? OFFSET ?`,
      [size, offset],
    ),
  );
  for await (const x of rows) {
    if (shown.has(x.topic)) yield { id: x.ID, topic_id: x.topic, author_id: known.has(x.post_author) ? x.post_author : null, body: autop(x.post_content ?? ""), format: "html", created_at: gmt(x.post_date_gmt) };
  }
}
counts.posts = await out.section("posts", posts());

// ---- topic tags (the `topic-tag` taxonomy) --------------------------------------------------------------------------
const tags = await optional("topic tags", () =>
  q(
    `SELECT r.object_id AS topic_id, tm.name FROM ${p}term_relationships r
     JOIN ${p}term_taxonomy tt ON tt.term_taxonomy_id = r.term_taxonomy_id AND tt.taxonomy = 'topic-tag'
     JOIN ${p}terms tm ON tm.term_id = tt.term_id ORDER BY r.object_id`,
  ),
);
counts.tags = await out.section("tags", tags.filter((x) => shown.has(x.topic_id)));

await out.end();
await db.end();
console.log(`Wrote ${Object.entries(counts).map(([k, n]) => `${n} ${k}`).join(", ")}.`);
