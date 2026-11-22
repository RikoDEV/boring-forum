// Writes a Discourse forum to the importer's JSON format, from its Postgres database:
//   npm i pg
//   DISCOURSE_DB="postgres://user:pass@host/discourse" node tools/importers/export-discourse.mjs out.json
//   boring-forum import out.json
// Reads only. Exports members, avatars, the files of the posts (BF_IMPORT_FILES), categories, topics, posts (in pages), tags, groups, polls (the
// discourse-poll plugin: choice, multiple, number and ranked, with their visibility, results and groups), likes and private messages. Post text is taken as Markdown; uploads stay
// as the upload:// or /uploads/ links in the text, so copy the uploads folder and redirect it.
// Passwords: Discourse's salted PBKDF2-HMAC-SHA256 is carried over as
// discourse$<iterations>$<salt>$<hash> and checked at the member's next sign-in, when it is
// replaced. Other algorithms are dropped, and that member resets the password.
import { createRequire } from "node:module";
import { Out, iso, pages } from "./lib.mjs";
const require = createRequire(`${process.cwd()}/`);
const { Client } = require("pg");
const db = new Client({ connectionString: process.env.DISCOURSE_DB });
await db.connect();
const q = async (sql, args = []) => (await db.query(sql, args)).rows;
const optional = async (what, run) => {
  try {
    return await run();
  } catch (e) {
    console.error(`skipped ${what}: ${e.message}`);
    // A failed statement aborts a transaction block only; outside one the connection is fine.
    return [];
  }
};
const out = new Out(process.argv[2]);
const counts = {};

const hash = (u) => {
  const m = /^\$pbkdf2-sha256\$i=(\d+)/.exec(u.password_algorithm ?? "");
  return m && u.password_hash && u.salt ? `discourse$${m[1]}$${u.salt}$${u.password_hash}` : null;
};
// Newer Discourse versions keep the password in user_passwords; older ones on the user itself.
const split = (await q(`SELECT to_regclass('user_passwords') AS t`))[0].t !== null;
// Real members, and system accounts (ids at or below 0: the system user, bots) only when they wrote posts, so a
// bot's posts keep their author. A member without an email gets a placeholder address.
const exported = `(u.id > 0 OR EXISTS (SELECT 1 FROM posts bp WHERE bp.user_id = u.id AND bp.post_type = 1 AND bp.deleted_at IS NULL)) AND u.staged = false`;
const users = await q(
  split
    ? `SELECT u.id, u.username, coalesce(e.email, u.username || '@import.invalid') AS email, u.created_at, u.last_seen_at, u.trust_level, st.days_visited, st.topics_entered, up.password_hash, up.password_salt AS salt, up.password_algorithm FROM users u LEFT JOIN user_emails e ON e.user_id = u.id AND e.primary LEFT JOIN user_stats st ON st.user_id = u.id LEFT JOIN user_passwords up ON up.user_id = u.id AND up.password_expired_at IS NULL WHERE ${exported}`
    : `SELECT u.id, u.username, coalesce(e.email, u.username || '@import.invalid') AS email, u.created_at, u.last_seen_at, u.trust_level, st.days_visited, st.topics_entered, u.password_hash, u.salt, u.password_algorithm FROM users u LEFT JOIN user_emails e ON e.user_id = u.id AND e.primary LEFT JOIN user_stats st ON st.user_id = u.id WHERE ${exported}`,
);
const known = new Set(users.map((u) => u.id));

// The forums and topics come first so the files can be limited to those the export keeps.
const cats = await q(`SELECT id, parent_category_id, name, position FROM categories WHERE read_restricted = false ORDER BY position NULLS LAST, id`);
const allowed = new Set(cats.map((c) => c.id));
const topics = (await q(`SELECT t.id, t.category_id, t.title, t.slug, t.views FROM topics t WHERE t.deleted_at IS NULL AND t.archetype = 'regular' AND t.visible AND t.category_id IS NOT NULL`)).filter((t) => allowed.has(t.category_id));
const shown = new Set(topics.map((t) => t.id));

counts.members = await out.section("users", users.map((u) => ({ id: u.id, username: u.username, email: u.email, password_hash: hash(u), joined_at: iso(u.created_at), last_seen_at: iso(u.last_seen_at), trust_level: u.trust_level, days_visited: u.days_visited, topics_read: u.topics_entered })));

// Outside logins the members had, so they sign in the same way here: DiscourseConnect (SSO) records and the
// OAuth accounts (Google, GitHub, Discord, Microsoft, Twitter/X) Discourse linked. The subject is the provider's own id for the
// member. Other providers (Facebook, Twitter, ...) are not offered here and are left out.
const LOGINS = { google_oauth2: "google", github: "github", discord: "discord", microsoft_office365: "microsoft", twitter: "x" };
const identities = [
  ...(await optional("DiscourseConnect records", () => q(`SELECT user_id, external_id, external_email FROM single_sign_on_records WHERE external_id IS NOT NULL`))).map((r) => ({ user_id: r.user_id, provider: "sso", subject: String(r.external_id), email: r.external_email ?? null })),
  ...(await optional("associated accounts", () => q(`SELECT user_id, provider_name, provider_uid, info FROM user_associated_accounts WHERE provider_uid IS NOT NULL`))).flatMap((r) => (LOGINS[r.provider_name] ? [{ user_id: r.user_id, provider: LOGINS[r.provider_name], subject: String(r.provider_uid), email: (typeof r.info === "string" ? JSON.parse(r.info || "{}") : (r.info ?? {})).email ?? null }] : [])),
].filter((i) => known.has(i.user_id));
counts.logins = await out.section("identities", identities);

// Site settings. Discourse keeps in site_settings only the ones changed from their defaults (booleans are "t" or
// "f"; a picture is an upload id). They are written under this forum's own setting keys, and an import never
// overwrites one that was already set here. The OAuth client IDs and secrets work here too once this site's callback
// address (/auth/google/callback, and so on) is added at the provider.
const siteRows = await optional("site settings", () => q(`SELECT name, value FROM site_settings`));
const ss = new Map(siteRows.map((r) => [r.name, r.value ?? ""]));
const text = (...names) => names.map((n) => String(ss.get(n) ?? "").trim()).find((v) => v) ?? "";
const yes = (name) => ss.get(name) === "t";
const settings = {};
const put = (key, value) => {
  if (value !== "" && value != null) settings[key] = value;
};
put("site_name", text("title"));
put("site_description", text("site_description", "short_site_description"));
put("contact_email", text("contact_email"));
const locale = text("default_locale").split(/[_-]/)[0];
if (["en", "pl", "ar"].includes(locale)) put("default_locale", locale);
for (const name of ["login_required", "invite_only", "auth_skip_create_confirm", "auth_immediately", "auth_overrides_email", "auth_overrides_username", "auth_overrides_avatar", "auth_overrides_name"]) {
  if (ss.has(name)) settings[name] = yes(name);
}
// DiscourseConnect (older versions call it sso_*). Its own "overrides" settings are this forum's auth_overrides_*.
const ssoUrl = text("discourse_connect_url", "sso_url");
if (ssoUrl) {
  put("login_sso_url", ssoUrl);
  put("login_sso_secret", text("discourse_connect_secret", "sso_secret"));
  settings.login_sso_enabled = yes("enable_discourse_connect") || yes("enable_sso");
  for (const field of ["email", "username", "name", "avatar"]) {
    if (yes(`discourse_connect_overrides_${field}`) || yes(`sso_overrides_${field}`)) settings[`auth_overrides_${field}`] = true;
  }
}
for (const [key, on, id, secret] of [
  ["google", "enable_google_oauth2_logins", "google_oauth2_client_id", "google_oauth2_client_secret"],
  ["github", "enable_github_logins", "github_client_id", "github_client_secret"],
  ["discord", "enable_discord_logins", "discord_client_id", "discord_secret"],
]) {
  if (!text(id) || !text(secret)) continue;
  put(`login_${key}_id`, text(id));
  put(`login_${key}_secret`, text(secret));
  settings[`login_${key}_enabled`] = yes(on);
}
// The site's pictures are uploads: they travel in the uploads section below, marked with the setting they become.
const BRAND = { logo: "logo_url", logo_dark: "logo_dark_url", logo_small: "logo_small_url", favicon: "favicon_url", large_icon: "large_icon_url", opengraph_image: "og_image_url" };
const brand = new Map();
for (const [name, key] of Object.entries(BRAND)) {
  const id = Number(text(name));
  if (id > 0) brand.set(id, key);
}
counts.settings = await out.object("settings", settings);

// Files: the pictures and attachments of the posts that are exported, and the members' avatars. A post names
// a file by its short name, the SHA-1 written in base62 (digits, then lower case, then upper case, 27 places).
// The files are in the backup's uploads folder: give it to the importer as BF_IMPORT_FILES.
const ALPHABET = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
const short = (sha1) => {
  let n = BigInt(`0x${sha1}`);
  let text = "";
  for (; n > 0n; n /= 62n) text = ALPHABET[Number(n % 62n)] + text;
  return text.padStart(27, "0");
};
const avatars = new Map((await optional("avatars", () => q(`SELECT uploaded_avatar_id AS upload, id FROM users WHERE uploaded_avatar_id IS NOT NULL`))).map((r) => [r.upload, r.id]));
// The posts themselves say which files they use: their short names and the old addresses. (The reference table
// Discourse keeps is not complete on every site.)
const usedNames = new Set();
for (const p of await q(`SELECT p.topic_id, p.raw FROM posts p WHERE p.deleted_at IS NULL AND (p.raw LIKE '%upload://%' OR p.raw LIKE '%/uploads/default/%')`)) {
  if (!shown.has(p.topic_id)) continue;
  for (const m of p.raw.matchAll(/upload:\/\/([0-9A-Za-z]+)/g)) usedNames.add(m[1]);
  for (const m of p.raw.matchAll(/\/uploads\/default\/(?:original|optimized)\/[^\s)"'<>\]]*?([0-9a-f]{40})/g)) usedNames.add(m[1]);
}
const files = (await optional("uploads", () => q(`SELECT id, user_id, sha1, original_filename, url FROM uploads WHERE id > 0 AND url LIKE '/uploads/%' ORDER BY id`)))
  .filter((u) => u.sha1 && (usedNames.has(short(u.sha1)) || usedNames.has(u.sha1) || avatars.has(u.id) || brand.has(u.id)));
counts.files = await out.section("uploads", files.map((u) => ({ id: u.id, refs: [short(u.sha1), u.sha1], path: u.url.replace(/^\/uploads\//, ""), name: u.original_filename, user_id: known.has(u.user_id) ? u.user_id : null, avatar_of: known.has(avatars.get(u.id)) ? avatars.get(u.id) : null, site_setting: brand.get(u.id) ?? null })));

// Position is the place among the categories of the same parent, as Discourse shows them.
counts.forums = await out.section("forums", cats.map((c) => ({ id: c.id, parent_id: c.parent_category_id, title: c.name, kind: "forum", position: c.position })));
counts.topics = await out.section("topics", topics.map((t) => ({ id: t.id, forum_id: t.category_id, title: t.title, url: `/t/${t.slug}/${t.id}`, views: t.views })));

async function* posts() {
  const rows = pages((offset, size) => q(`SELECT p.id, p.topic_id, p.user_id, p.raw, p.created_at FROM posts p WHERE p.deleted_at IS NULL AND p.hidden = false AND p.post_type = 1 ORDER BY p.topic_id, p.post_number LIMIT $1 OFFSET $2`, [size, offset]));
  for await (const p of rows) {
    if (shown.has(p.topic_id)) yield { id: p.id, topic_id: p.topic_id, author_id: known.has(p.user_id) ? p.user_id : null, body: p.raw, format: "markdown", created_at: iso(p.created_at) };
  }
}
counts.posts = await out.section("posts", posts());

const tags = (await optional("tags", () => q(`SELECT tt.topic_id, t.name FROM topic_tags tt JOIN tags t ON t.id = tt.tag_id`))).filter((t) => shown.has(t.topic_id));
counts.tags = await out.section("tags", tags.map((t) => ({ topic_id: t.topic_id, name: t.name })));

const groups = await optional("groups", async () => {
  const gs = await q(`SELECT id, name FROM groups WHERE automatic = false AND visibility_level = 0`);
  const result = [];
  for (const g of gs) result.push({ id: g.id, name: g.name, members: (await q(`SELECT user_id FROM group_users WHERE group_id = $1`, [g.id])).map((m) => m.user_id) });
  return result;
});
counts.groups = await out.section("groups", groups);

// Discourse stores its enums as numbers in some versions and names in others: accept either.
const named = (value, names) => (typeof value === "number" || /^\d+$/.test(String(value)) ? names[Number(value)] : String(value ?? ""));
const polls = await optional("polls", async () => {
  const ps = await q(`SELECT pl.*, po.topic_id, po.post_number FROM polls pl JOIN posts po ON po.id = pl.post_id WHERE po.deleted_at IS NULL`);
  const result = [];
  for (const pl of ps) {
    if (!shown.has(pl.topic_id)) continue;
    const type = named(pl.type, ["regular", "multiple", "number", "ranked_choice"]);
    const options = await q(`SELECT id, html FROM poll_options WHERE poll_id = $1 ORDER BY position, id`, [pl.id]);
    if (options.length < 2) continue;
    const index = new Map(options.map((o, i) => [o.id, i]));
    const votes = (await q(`SELECT * FROM poll_votes WHERE poll_id = $1`, [pl.id])).filter((v) => index.has(v.poll_option_id)).map((v) => ({ user_id: v.user_id, option: index.get(v.poll_option_id), rank: v.rank || null }));
    const results = { always: "always", on_vote: "voted", on_close: "closed", staff_only: "staff" }[named(pl.results, ["always", "on_vote", "on_close", "staff_only"])] ?? "always";
    const closed = named(pl.status, ["open", "closed"]) === "closed";
    result.push({
      id: pl.id,
      topic_id: pl.topic_id,
      post_id: pl.post_number > 1 ? pl.post_id : null,
      question: !pl.title ? (pl.name === "poll" ? "Poll" : pl.name) : pl.title,
      options: options.map((o) => o.html.replace(/<[^>]*>/g, "")),
      kind: type === "ranked_choice" ? "ranked" : type === "number" ? "rating" : "choice",
      max_choices: type === "multiple" ? pl.max || options.length : 1,
      // "everyone" (1) shows who voted; "secret" (0) does not.
      public: named(pl.visibility, ["secret", "everyone"]) === "everyone",
      results,
      closes_at: iso(pl.close_at) ?? (closed ? iso(new Date()) : null),
      groups: String(pl.groups ?? "").split(",").map((g) => g.trim()).filter(Boolean),
      votes,
    });
  }
  return result;
});
counts.polls = await out.section("polls", polls);

// post_action_type_id 2 is the like.
counts.reactions = await out.section("reactions", pages((offset, size) => optional("reactions", () => q(`SELECT post_id, user_id FROM post_actions WHERE post_action_type_id = 2 AND deleted_at IS NULL ORDER BY id LIMIT $1 OFFSET $2`, [size, offset]))));

const conversations = await optional("messages", async () => {
  const ts = await q(`SELECT id, title FROM topics WHERE archetype = 'private_message' AND deleted_at IS NULL ORDER BY id`);
  const result = [];
  for (const t of ts) {
    const people = await q(`SELECT user_id FROM topic_allowed_users WHERE topic_id = $1`, [t.id]);
    // The welcome and tutorial messages bots send are not conversations between people.
    if (people.filter((x) => x.user_id > 0).length < 2) continue;
    const msgs = await q(`SELECT user_id, raw, created_at FROM posts WHERE topic_id = $1 AND deleted_at IS NULL AND post_type = 1 ORDER BY post_number`, [t.id]);
    result.push({ id: t.id, subject: t.title, participants: people.map((x) => x.user_id), messages: msgs.map((m) => ({ author_id: m.user_id, body: m.raw, format: "markdown", created_at: iso(m.created_at) })) });
  }
  return result;
});
counts.messages = await out.section("messages", conversations);

await out.end();
await db.end();
console.log(`Wrote ${Object.entries(counts).map(([k, n]) => `${n} ${k}`).join(", ")}.`);
