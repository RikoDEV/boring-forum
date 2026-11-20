// Writes a Flarum 1.x forum to the importer's JSON format:
//   npm i mysql2
//   FLARUM_DB="mysql://user:pass@host/dbname" node tools/importers/export-flarum.mjs out.json
//   BF_IMPORT_FILES=/path/to/flarum boring-forum import out.json
// Reads only. Set FLARUM_PREFIX if the tables have a prefix (Flarum's default is none). Give the importer the Flarum
// folder (the one with `assets/`) as BF_IMPORT_FILES: uploaded avatars (`assets/avatars/…`) are copied from there.
// Written from Flarum's schema, not tried against a live install: every table an extension adds is read through
// `optional`, so a missing one only skips that part.
// Exports members and their avatars (bcrypt hashes are carried over as they are, so members keep their passwords),
// forums (Flarum's primary tags, those with a position, with their parent tag; restricted tags and the discussions only
// in them are left out), topics (stuck and locked ones from the sticky and lock extensions; a discussion with no primary
// tag goes to a forum "General"), posts (comments only, not hidden or awaiting approval), the other tags as topic tags,
// custom groups (id above 4), likes (flarum/likes) and polls (fof/polls). Private messages (flarum-byobu) and
// attachments are not exported; neither is the number of views, which Flarum does not keep.
// Posts: Flarum stores TextFormatter XML (`<r><p>…</p></r>`); it is converted to Markdown here: bold, italic,
// strike, links, images, code, quotes, lists and headings. A post that is plain text is taken as it is.
import { createRequire } from "node:module";
import { Out, iso, optional, pages } from "./lib.mjs";
const require = createRequire(`${process.cwd()}/`);
const mysql = require("mysql2/promise");
// Flarum keeps its times in UTC.
const db = await mysql.createConnection({ uri: process.env.FLARUM_DB, timezone: "Z" });
const p = process.env.FLARUM_PREFIX ?? "";
const t = (name) => `\`${p}${name}\``;
const q = async (sql, args = []) => (await db.query(sql, args))[0];
const out = new Out(process.argv[2]);
const counts = {};

// ---- post text ------------------------------------------------------------------------------------------------------
const ENTITIES = { amp: "&", lt: "<", gt: ">", quot: '"', apos: "'" };
const decode = (s) =>
  s.replace(/&(#x[0-9a-f]+|#\d+|[a-z]+);/gi, (m, e) => {
    if (e[0] === "#") {
      const n = e[1].toLowerCase() === "x" ? parseInt(e.slice(2), 16) : parseInt(e.slice(1), 10);
      try {
        return String.fromCodePoint(n);
      } catch {
        return m;
      }
    }
    return ENTITIES[e.toLowerCase()] ?? m;
  });
const attr = (attrs, name) => {
  const m = new RegExp(`(?:^|\\s)${name}="([^"]*)"`).exec(attrs);
  return m ? decode(m[1]) : "";
};

/** Flarum's TextFormatter XML as Markdown. The lower-case `s`, `e` and `i` elements only hold the markup the author
 *  typed (`**`, "> "), which is rebuilt from the upper-case elements here, so their text is dropped. */
function toMarkdown(content) {
  if (!/^\s*<[rt][\s>]/.test(content)) return content.trim();
  const bufs = [""];
  const put = (s) => {
    bufs[bufs.length - 1] += s;
  };
  const open = [];
  const lists = [];
  let skip = 0;
  let code = 0;
  const tag = /<(\/?)([A-Za-z][A-Za-z0-9]*)([^>]*?)(\/?)>|([^<]+)/g;
  for (const m of content.matchAll(tag)) {
    const [, close, name, attrs, selfClose, text] = m;
    if (text !== undefined) {
      if (!skip) put(decode(text));
      continue;
    }
    if (name === name.toLowerCase()) {
      if (name === "s" || name === "e" || name === "i") {
        if (!selfClose) skip = Math.max(0, skip + (close ? -1 : 1));
      } else if (!skip && name === "br") put(code ? "\n" : "  \n");
      else if (!skip && name === "p" && close) put("\n\n");
      continue;
    }
    if (close) {
      const top = open.pop();
      if (name === "IMG") skip = Math.max(0, skip - 1);
      if (skip) continue;
      switch (name) {
        case "B": case "STRONG": put("**"); break;
        case "I": case "EM": put("*"); break;
        case "S": case "DEL": put("~~"); break;
        case "U": put("++"); break;
        case "URL": put(`](${top?.href ?? ""})`); break;
        case "CODE": code--; put("\n```\n"); break;
        case "C": code--; put("`"); break;
        case "QUOTE": {
          const inner = bufs.pop().trim();
          put(`\n${inner.split("\n").map((l) => `> ${l}`.trimEnd()).join("\n")}\n\n`);
          break;
        }
        case "LIST": lists.pop(); put("\n"); break;
        case "LI": put("\n"); break;
        case "H1": case "H2": case "H3": case "H4": case "H5": case "H6": put("\n\n"); break;
        default:
      }
      continue;
    }
    if (!selfClose) open.push({ name, href: name === "URL" ? attr(attrs, "url") : "" });
    if (skip) continue;
    switch (name) {
      case "B": case "STRONG": put("**"); break;
      case "I": case "EM": put("*"); break;
      case "S": case "DEL": put("~~"); break;
      case "U": put("++"); break;
      case "URL": put("["); break;
      case "IMG":
        put(`![${attr(attrs, "alt")}](${attr(attrs, "src")})`);
        if (!selfClose) skip++;
        break;
      case "CODE": code++; put(`\n\`\`\`${attr(attrs, "lang")}\n`); break;
      case "C": code++; put("`"); break;
      case "QUOTE": bufs.push(""); break;
      case "LIST": lists.push({ ordered: attr(attrs, "type") !== "", n: 0 }); put("\n"); break;
      case "LI": {
        const l = lists[lists.length - 1];
        put(l?.ordered ? `${++l.n}. ` : "- ");
        break;
      }
      case "H1": case "H2": case "H3": case "H4": case "H5": case "H6": put(`\n${"#".repeat(Number(name[1]))} `); break;
      case "HR": put("\n---\n"); break;
      default:
    }
  }
  while (bufs.length > 1) {
    const inner = bufs.pop();
    put(inner);
  }
  return bufs[0].replace(/\n{3,}/g, "\n\n").trim();
}

// ---- members and files ----------------------------------------------------------------------------------------------
const users = await q(`SELECT id, username, email, password, joined_at, avatar_url FROM ${t("users")} WHERE email <> ''`);
const known = new Set(users.map((u) => u.id));
counts.members = await out.section("users", users.map((u) => ({ id: u.id, username: u.username, email: u.email, password_hash: /^\$2[abxy]\$/.test(u.password ?? "") ? u.password : null, joined_at: iso(u.joined_at) })));

// Uploaded avatars are files in assets/avatars; an address is a picture kept elsewhere and is skipped.
counts.files = await out.section(
  "uploads",
  users
    .filter((u) => u.avatar_url && !/^(https?:)?\/\//.test(u.avatar_url))
    .map((u) => ({ id: 1_000_000 + u.id, refs: [], path: `assets/avatars/${u.avatar_url.split("?")[0]}`, name: "avatar", user_id: u.id, avatar_of: u.id })),
);

// ---- forums: the primary tags (Flarum gives only those a position) -------------------------------------------------
const tagRows = await q(`SELECT * FROM ${t("tags")}`);
const allowedTag = (g) => !g.is_restricted && !g.is_hidden;
const primary = tagRows.filter((g) => g.position !== null && allowedTag(g));
const primaryIds = new Set(primary.map((g) => g.id));
const labelIds = new Map(tagRows.filter((g) => g.position === null && allowedTag(g)).map((g) => [g.id, g.name]));
const restrictedIds = new Set(tagRows.filter((g) => !allowedTag(g)).map((g) => g.id));
const depth = new Map(tagRows.map((g) => [g.id, g.parent_id ? 1 : 0]));

const links = await optional("discussion tags", () => q(`SELECT discussion_id, tag_id FROM ${t("discussion_tag")}`));
const tagsOf = new Map();
for (const l of links) {
  if (!tagsOf.has(l.discussion_id)) tagsOf.set(l.discussion_id, []);
  tagsOf.get(l.discussion_id).push(l.tag_id);
}
const CATCH_ALL = 999999;
/** The forum of a discussion: its deepest primary tag; none at all means the catch-all; only restricted tags, no forum. */
const forumOf = (id) => {
  const ids = tagsOf.get(id) ?? [];
  const mine = ids.filter((g) => primaryIds.has(g)).sort((a, b) => depth.get(b) - depth.get(a) || a - b);
  if (mine.length) return mine[0];
  return ids.some((g) => restrictedIds.has(g)) ? null : CATCH_ALL;
};

const discussions = (await q(`SELECT * FROM ${t("discussions")} WHERE hidden_at IS NULL ORDER BY id`))
  .filter((d) => d.is_approved === undefined || d.is_approved)
  .map((d) => ({ d, forum: forumOf(d.id) }))
  .filter((x) => x.forum !== null);
const needCatchAll = discussions.some((x) => x.forum === CATCH_ALL);
const forums = [
  ...primary.map((g) => ({ id: g.id, parent_id: g.parent_id && primaryIds.has(g.parent_id) ? g.parent_id : null, title: g.name, kind: "forum", position: g.position })),
  ...(needCatchAll ? [{ id: CATCH_ALL, parent_id: null, title: "General", kind: "forum", position: 9999 }] : []),
];
counts.forums = await out.section("forums", forums);

const shown = new Set(discussions.map((x) => x.d.id));
counts.topics = await out.section(
  "topics",
  discussions.map(({ d, forum }) => ({ id: d.id, forum_id: forum, title: d.title, url: `/d/${d.id}${d.slug ? `-${d.slug}` : ""}`, pinned: !!d.is_sticky, locked: !!d.is_locked })),
);

// ---- posts ----------------------------------------------------------------------------------------------------------
const postIds = new Set();
async function* posts() {
  const rows = pages((offset, size) => q(`SELECT * FROM ${t("posts")} WHERE type = 'comment' AND hidden_at IS NULL ORDER BY discussion_id, number, id LIMIT ? OFFSET ?`, [size, offset]));
  for await (const x of rows) {
    if (!shown.has(x.discussion_id) || (x.is_approved !== undefined && !x.is_approved)) continue;
    postIds.add(x.id);
    yield { id: x.id, topic_id: x.discussion_id, author_id: known.has(x.user_id) ? x.user_id : null, body: toMarkdown(x.content ?? ""), format: "markdown", created_at: iso(x.created_at) };
  }
}
counts.posts = await out.section("posts", posts());

// ---- topic tags, groups, polls, likes -------------------------------------------------------------------------------
async function* topicTags() {
  for (const [discussion, ids] of tagsOf) {
    if (!shown.has(discussion)) continue;
    for (const g of ids) if (labelIds.has(g)) yield { topic_id: discussion, name: labelIds.get(g) };
  }
}
counts.tags = await out.section("tags", topicTags());

// Flarum's own groups are 1 (Admin), 2 (Guest), 3 (Member) and 4 (Moderator); the rest are custom.
const custom = await optional("groups", async () => {
  const gs = await q(`SELECT id, name_plural, name_singular FROM ${t("groups")} WHERE id > 4`);
  const result = [];
  for (const g of gs) {
    const members = await q(`SELECT user_id FROM ${t("group_user")} WHERE group_id = ?`, [g.id]);
    result.push({ id: g.id, name: g.name_plural || g.name_singular, members: members.map((m) => m.user_id).filter((id) => known.has(id)) });
  }
  return result;
});
counts.groups = await out.section("groups", custom);

// fof/polls (polls, poll_options, poll_votes). The poll belongs to a post, whose discussion is the topic.
const polls = await optional("polls", async () => {
  const rows = await q(`SELECT * FROM ${t("polls")}`);
  const result = [];
  for (const pl of rows) {
    let topic = pl.discussion_id;
    if (!topic && pl.post_id) topic = (await q(`SELECT discussion_id FROM ${t("posts")} WHERE id = ?`, [pl.post_id]))[0]?.discussion_id;
    if (!topic || !shown.has(topic)) continue;
    const options = await q(`SELECT id, answer FROM ${t("poll_options")} WHERE poll_id = ? ORDER BY id`, [pl.id]);
    if (options.length < 2) continue;
    const index = new Map(options.map((o, i) => [o.id, i]));
    const votes = (await q(`SELECT option_id, user_id FROM ${t("poll_votes")} WHERE poll_id = ?`, [pl.id]))
      .filter((v) => index.has(v.option_id) && known.has(v.user_id))
      .map((v) => ({ user_id: v.user_id, option: index.get(v.option_id) }));
    result.push({
      id: pl.id,
      topic_id: topic,
      question: pl.question,
      options: options.map((o) => o.answer),
      max_choices: pl.allow_multiple_votes ? pl.max_votes || options.length : 1,
      closes_at: pl.end_date ? iso(pl.end_date) : null,
      public: !!pl.public_poll,
      post_id: pl.post_id && postIds.has(pl.post_id) ? pl.post_id : undefined,
      votes,
    });
  }
  return result;
});
counts.polls = await out.section("polls", polls);

// flarum/likes: one row per liked post.
async function* likes() {
  const rows = pages((offset, size) => q(`SELECT post_id, user_id FROM ${t("post_likes")} ORDER BY post_id, user_id LIMIT ? OFFSET ?`, [size, offset]));
  try {
    for await (const l of rows) if (postIds.has(l.post_id) && known.has(l.user_id)) yield { post_id: l.post_id, user_id: l.user_id };
  } catch (e) {
    console.error(`skipped likes: ${e.message}`);
  }
}
counts.reactions = await out.section("reactions", likes());

await out.end();
await db.end();
console.log(`Wrote ${Object.entries(counts).map(([k, n]) => `${n} ${k}`).join(", ")}.`);
