//! `boring-forum import <file.json>`: loads a neutral export into this forum.
//! `tools/importers/export-ips.mjs` writes that format from an IPS database. Re-running skips what was
//! already imported, so an interrupted import can continue.
//!
//! Format: `{"users": [{id, username, email, password_hash, joined_at}],
//! "forums": [{id, parent_id, title, kind, public}], "topics": [{id, forum_id, author_id, title, created_at, url}],
//! "posts": [{id, topic_id, author_id, body, format, created_at, attachments: [{name, url}]}]}`, with
//! `format` one of html, bbcode, markdown, and times in RFC 3339. Posts of a topic must be in order,
//! first post first. Optional sections, read after the ones above:
//! `"tags": [{topic_id, name}]` (a topic may also carry `"tags": ["a", "b"]`),
//! `"groups": [{id, name, members: [user_id]}]`,
//! `"polls": [{id, topic_id, question, options: ["a", "b"], max_choices, closes_at, votes: [{user_id, option}]}]`
//! (`option` is the index in `options`; a poll may also say `kind` (`choice`, `ranked`, `rating`: the
//! options are the numbers), `public`, `results` (`always`, `voted`, `closed`, `staff`),
//! `final_votes`, `post_id` (the reply it was in), `groups` (names that may vote) and
//! `min_trust_level`, and a ranked vote its `rank`, 1 first), `"reactions": [{post_id, user_id}]` (all become likes) and
//! `"messages": [{id, subject, participants: [user_id], messages: [{author_id, body, format, created_at}]}]`.
//! Attachments are references: each becomes a link at the end of its post, so the files can be
//! copied over separately and the addresses redirected.
//!
//! Files: `"uploads": [{id, refs: ["<name posts use>", ...], path, name, user_id, avatar_of}]`,
//! read after the users and before the posts. With `BF_IMPORT_FILES=<folder>` each `path` (relative to
//! that folder) is read and stored like an upload of the member `user_id`, without their daily limit,
//! and the links in posts and messages that name one of its `refs` (Discourse's `upload://…` short
//! links and its `/uploads/default/original/…` addresses) now point at the stored file, with the
//! image's size so it previews at once. `avatar_of` (a source member) makes the file that member's
//! avatar. Without `BF_IMPORT_FILES` the section is skipped and those links stay as they were. A file
//! this site does not accept (its kind, its size) is skipped and counted.
//! Members may carry `trust_level`, `days_visited` and `topics_read`, where they stood on the old site:
//! the level is kept (never lowered), and the days and topics count toward the requirements of the levels
//! here (`users.visit_credit`, `users.read_credit`). When the import ends, the promotion rules run once,
//! without telling anyone, so members who earned a higher level by that history have it.
//! Topics may carry `views`, how many times they were viewed (only ever raised), and `pinned`, `announcement`
//! (shown in every forum) and `locked`.
//! Forums may carry `position`, their place among the forums of the same parent (lowest first).
//!
//! The file is read as a stream, one entry at a time, so its size is bounded by the disk, not by
//! memory; only the topic list (id, forum, title) is kept in memory. Sections must come in the
//! order above: a post needs its topic, a reaction its post. `password_hash` may be bcrypt (`$2…`),
//! phpBB 3.0's phpass (`$H$…`), `discourse$<iterations>$<salt>$<hex>` or `ips3$<salt>$<hex>`; any
//! other value is dropped, so that member resets the password. A username registration here would
//! refuse is rewritten (its allowed characters, else `user-<old id>`), and the count is reported.
//!
//! Forums are imported for members and staff. Guests can read a forum only when its entry has
//! `"public": true`; `BF_IMPORT_PUBLIC=1` is the default for forums that do not say.
//!
//! Plugins: unless `BF_IMPORT_PLUGINS=0`, the enabled plugins are loaded and every imported post
//! goes through their `post.before_create` / `post.before_cook` and (for topics) `item.before_create`
//! filters, as a new post would. A post a plugin refuses is skipped and counted; a refused title
//! keeps its original. Each imported topic and reply is then announced to plugins only
//! (`topic.created` / `post.created` with `"imported": true`), never to webhooks or federation, so
//! a backfill does not flood outside systems. With no plugin listening it costs nothing.

// This module is the output of the `import` subcommand, not of a request.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use bf_core::perms::Perms;
use bf_core::slug::slugify;
use bf_db::platform::importer;
use serde::Deserialize;

#[derive(Deserialize)]
struct User {
    id: i64,
    username: String,
    email: String,
    password_hash: Option<String>,
    joined_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Where they stood on the old site: trust level, days visited, topics read.
    trust_level: Option<i16>,
    days_visited: Option<i32>,
    topics_read: Option<i32>,
}

#[derive(Deserialize)]
struct Forum {
    id: i64,
    parent_id: Option<i64>,
    title: String,
    #[serde(default = "forum_kind")]
    kind: String,
    /// Whether guests may read it. Left out: only with `BF_IMPORT_PUBLIC=1`.
    public: Option<bool>,
    /// Place among the forums of the same parent, lowest first. Left out: the order in the file.
    position: Option<i32>,
}

#[derive(Deserialize)]
struct UploadRow {
    #[allow(dead_code)]
    id: i64,
    /// What posts call this file: Discourse's base62 short name, its SHA-1.
    #[serde(default)]
    refs: Vec<String>,
    /// Where the file is, relative to `BF_IMPORT_FILES`.
    path: String,
    #[serde(default)]
    name: String,
    /// The member who uploaded it.
    user_id: Option<i64>,
    /// A source member whose avatar this is.
    avatar_of: Option<i64>,
}

fn forum_kind() -> String {
    "forum".into()
}

#[derive(Deserialize)]
struct Topic {
    id: i64,
    forum_id: i64,
    title: String,
    #[serde(default)]
    tags: Vec<String>,
    /// How many times it was viewed on the old site.
    views: Option<i32>,
    /// Stuck to the top of its forum, shown on every forum's list (an announcement), closed.
    #[serde(default)]
    pinned: bool,
    #[serde(default)]
    announcement: bool,
    #[serde(default)]
    locked: bool,
}

#[derive(Deserialize)]
struct Attachment {
    name: String,
    url: String,
}

#[derive(Deserialize)]
struct Post {
    id: i64,
    topic_id: i64,
    author_id: Option<i64>,
    body: String,
    #[serde(default)]
    format: String,
    created_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    attachments: Vec<Attachment>,
}

#[derive(Deserialize)]
struct TagRow {
    topic_id: i64,
    name: String,
}

#[derive(Deserialize)]
struct Group {
    id: i64,
    name: String,
    #[serde(default)]
    members: Vec<i64>,
}

#[derive(Deserialize)]
struct Vote {
    user_id: i64,
    option: usize,
    /// Place on a ranked ballot, 1 first.
    rank: Option<i16>,
}

#[derive(Deserialize)]
struct Poll {
    id: i64,
    topic_id: i64,
    question: String,
    options: Vec<String>,
    #[serde(default = "one")]
    max_choices: i32,
    closes_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    votes: Vec<Vote>,
    /// `choice` (the default), `ranked` or `rating` (the options are its numbers).
    #[serde(default)]
    kind: String,
    /// Voters are shown.
    #[serde(default)]
    public: bool,
    /// `always` (the default), `voted`, `closed` or `staff`.
    #[serde(default)]
    results: String,
    /// Votes cannot be changed; left out, they can.
    #[serde(default)]
    final_votes: bool,
    /// The reply the poll was in; left out, the topic.
    post_id: Option<i64>,
    /// Groups (by name) that may vote; one this forum does not have is ignored.
    #[serde(default)]
    groups: Vec<String>,
    #[serde(default)]
    min_trust_level: i16,
}

fn one() -> i32 {
    1
}

#[derive(Deserialize)]
struct Reaction {
    post_id: i64,
    user_id: i64,
}

#[derive(Deserialize)]
struct PmMessage {
    author_id: i64,
    body: String,
    #[serde(default)]
    format: String,
    created_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Deserialize)]
struct Conversation {
    id: i64,
    #[serde(default)]
    subject: String,
    #[serde(default)]
    participants: Vec<i64>,
    #[serde(default)]
    messages: Vec<PmMessage>,
}

/// One entry of the file, in file order.
enum Item {
    User(User),
    Upload(UploadRow),
    Forum(Forum),
    Topic(Topic),
    Post(Post),
    Tag(TagRow),
    Group(Group),
    Poll(Poll),
    Reaction(Reaction),
    Conversation(Conversation),
}

type Sender = tokio::sync::mpsc::Sender<Item>;

/// Reads one array of the file and sends each element on as it is parsed.
struct Section<'a, T> {
    tx: &'a Sender,
    wrap: fn(T) -> Item,
}

impl<'de, T: serde::de::DeserializeOwned> serde::de::DeserializeSeed<'de> for Section<'_, T> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de, T: serde::de::DeserializeOwned> serde::de::Visitor<'de> for Section<'_, T> {
    type Value = ();
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a list")
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while let Some(v) = seq.next_element::<T>()? {
            // The receiver is gone when the import stopped on an error; stop reading too.
            self.tx.blocking_send((self.wrap)(v)).map_err(serde::de::Error::custom)?;
        }
        Ok(())
    }
}

/// The top-level object: each known key is a section, anything else is skipped unread.
struct Root<'a>(&'a Sender);

impl<'de> serde::de::DeserializeSeed<'de> for Root<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de> serde::de::Visitor<'de> for Root<'_> {
    type Value = ();
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("an object")
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let tx = self.0;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "users" => map.next_value_seed(Section { tx, wrap: Item::User })?,
                "uploads" => map.next_value_seed(Section { tx, wrap: Item::Upload })?,
                "forums" => map.next_value_seed(Section { tx, wrap: Item::Forum })?,
                "topics" => map.next_value_seed(Section { tx, wrap: Item::Topic })?,
                "posts" => map.next_value_seed(Section { tx, wrap: Item::Post })?,
                "tags" => map.next_value_seed(Section { tx, wrap: Item::Tag })?,
                "groups" => map.next_value_seed(Section { tx, wrap: Item::Group })?,
                "polls" => map.next_value_seed(Section { tx, wrap: Item::Poll })?,
                "reactions" => map.next_value_seed(Section { tx, wrap: Item::Reaction })?,
                "messages" => map.next_value_seed(Section { tx, wrap: Item::Conversation })?,
                _ => {
                    map.next_value::<serde::de::IgnoredAny>()?;
                }
            }
        }
        Ok(())
    }
}

/// Keeps a password hash only in a format this forum can check.
fn usable_hash(hash: Option<&str>) -> Option<&str> {
    hash.filter(|h| h.len() <= 512 && (bf_core::auth::is_legacy_hash(h) || h.starts_with("$argon2")))
}

/// The name an imported member gets, and whether it had to be rewritten: the source's name when
/// registration here would accept it (`validate_username_for`), otherwise its allowed characters,
/// otherwise `user-<old id>`. Names end up in addresses, mentions and mail, so a name the site
/// would refuse to register must not arrive through an import either.
fn safe_username(name: &str, legacy: i64, unicode: bool) -> (String, bool) {
    let name = bf_core::auth::normalize_username(name);
    if bf_core::auth::validate_username_for(&name, unicode).is_ok() {
        return (name, false);
    }
    let edge = |c: char| !c.is_ascii_alphanumeric();
    let kept: String =
        name.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')).take(20).collect();
    let kept = kept.trim_matches(edge);
    if bf_core::auth::validate_username(kept).is_ok() {
        return (kept.to_owned(), true);
    }
    (format!("user-{}", legacy.unsigned_abs()), true)
}

/// Text inside tags, with entities decoded once, `&amp;` last so `&amp;lt;` stays literal.
fn decode_entities(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#039;", "'")
        .replace("&amp;", "&")
}

/// The few HTML shapes forum posts use; anything else keeps its text and loses its tag. The
/// result is Markdown and goes through the cooker's sanitizer like any post.
pub(crate) fn html_to_markdown(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    let mut link: Option<String> = None;
    while let Some(start) = rest.find('<') {
        out.push_str(&decode_entities(&rest[..start]));
        let Some(end) = rest[start..].find('>') else {
            out.push_str(&decode_entities(&rest[start..]));
            return out.trim().to_owned();
        };
        let tag = &rest[start + 1..start + end];
        let lower = tag.to_ascii_lowercase();
        let name = lower.trim_start_matches('/').split([' ', '/']).next().unwrap_or_default();
        let closing = lower.starts_with('/');
        match (name, closing) {
            ("br", _) | ("li", true) => out.push('\n'),
            ("p" | "div" | "blockquote" | "ul" | "ol" | "pre", true) => out.push_str("\n\n"),
            ("strong" | "b", _) => out.push_str("**"),
            ("em" | "i", _) => out.push('*'),
            ("code", _) => out.push('`'),
            ("li", false) => out.push_str("- "),
            ("blockquote", false) => out.push_str("> "),
            ("a", false) => {
                link = attribute(tag, "href");
                out.push('[');
            }
            ("a", true) => match link.take() {
                Some(href) => out.push_str(&format!("]({href})")),
                None => out.push(']'),
            },
            ("img", _) => {
                if let Some(src) = attribute(tag, "src") {
                    out.push_str(&format!("![]({src})"));
                }
            }
            _ => {}
        }
        rest = &rest[start + end + 1..];
    }
    out.push_str(&decode_entities(rest));
    out.trim().to_owned()
}

fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let at = lower.find(&format!("{name}="))? + name.len() + 1;
    let value = tag.get(at..)?;
    let quote = value.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    Some(decode_entities(value[1..].split(quote).next()?))
}

/// Common BBCode. The cooker handles what remains if the site's BBCode setting is on.
pub(crate) fn bbcode_to_markdown(s: &str) -> String {
    let mut t = s.to_owned();
    for (from, to) in [
        ("[b]", "**"),
        ("[/b]", "**"),
        ("[i]", "*"),
        ("[/i]", "*"),
        ("[code]", "\n```\n"),
        ("[/code]", "\n```\n"),
        ("[quote]", "\n> "),
        ("[/quote]", "\n\n"),
        ("[list]", "\n"),
        ("[/list]", "\n"),
        ("[*]", "\n- "),
        ("[B]", "**"),
        ("[/B]", "**"),
        ("[I]", "*"),
        ("[/I]", "*"),
    ] {
        t = t.replace(from, to);
    }
    while let Some(start) = t.find("[url=") {
        let Some(mid) = t[start..].find(']').map(|i| start + i) else { break };
        let Some(end) = t[mid..].find("[/url]").map(|i| mid + i) else { break };
        let href = t[start + 5..mid].trim_matches(['"', '\'']).to_owned();
        let text = t[mid + 1..end].to_owned();
        t.replace_range(start..end + 6, &format!("[{text}]({href})"));
    }
    t.trim().to_owned()
}

fn to_markdown(body: &str, format: &str) -> String {
    match format {
        "html" => html_to_markdown(body),
        "bbcode" => bbcode_to_markdown(body),
        _ => body.trim().to_owned(),
    }
}

/// A file an import stored: where it is now and how big, for the links that name it.
#[derive(Clone)]
struct Stored {
    key: String,
    width: Option<i32>,
    height: Option<i32>,
    image: bool,
}

/// Stored files by the names posts use for them: Discourse's short name (case matters) and its
/// SHA-1 (lower case).
type Files = std::collections::HashMap<String, Stored>;

/// A SHA-1 in lower case, so it is found however the source wrote it; any other name as it is.
fn hex_lower(name: &str) -> String {
    if name.len() == 40 && name.bytes().all(|b| b.is_ascii_hexdigit()) { name.to_ascii_lowercase() } else { name.to_owned() }
}

/// How long the size or kind mark is at the start of `s`: `690x388`, `690x388,50%` or `attachment`.
fn hint_len(s: &str) -> Option<usize> {
    if s.starts_with("attachment") {
        return Some("attachment".len());
    }
    let digits = |t: &str| t.bytes().take_while(u8::is_ascii_digit).count();
    let w = digits(s);
    if w == 0 || !s[w..].starts_with('x') {
        return None;
    }
    let h = digits(&s[w + 1..]);
    if h == 0 {
        return None;
    }
    let mut n = w + 1 + h;
    if s[n..].starts_with(',') {
        let p = digits(&s[n + 1..]);
        if p > 0 && s[n + 1 + p..].starts_with('%') {
            n += p + 2;
        }
    }
    Some(n)
}

/// Discourse puts the picture's display size (`![alt|690x388](…)`) and an `|attachment` mark at the
/// end of the link text. Neither belongs in the text here.
fn strip_link_hints(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s.as_bytes()[i] == b'|'
            && let Some(n) = hint_len(&s[i + 1..])
            && s[i + 1 + n..].starts_with("](")
        {
            i += 1 + n;
            continue;
        }
        let ch = s[i..].chars().next().unwrap_or(' ');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// A link to a picture: the text just written ends `![alt](`.
fn in_image_link(written: &str) -> bool {
    written
        .strip_suffix("](")
        .is_some_and(|head| head.rfind('[').is_some_and(|p| p > 0 && head.as_bytes()[p - 1] == b'!'))
}

/// `upload://<name>[.ext]` at the start of `t`: how much it takes and the name.
fn short_ref(t: &str) -> Option<(usize, &str)> {
    let rest = t.strip_prefix("upload://")?;
    let name = rest.bytes().take_while(u8::is_ascii_alphanumeric).count();
    if name == 0 {
        return None;
    }
    let name_end = "upload://".len() + name;
    let mut n = name_end;
    if let Some(ext) = t[n..].strip_prefix('.') {
        n += 1 + ext.bytes().take_while(u8::is_ascii_alphanumeric).count();
    }
    Some((n, &t["upload://".len()..name_end]))
}

/// `/uploads/default/original/2X/d/<sha1>.jpeg` (or `optimized`, with `_2_690x388` after the digest)
/// at the start of `t`: how much it takes and the SHA-1.
fn long_ref(t: &str) -> Option<(usize, String)> {
    let rest = t.strip_prefix("/uploads/default/")?;
    if !(rest.starts_with("original/") || rest.starts_with("optimized/")) {
        return None;
    }
    let end = t.find(|c: char| c.is_whitespace() || ")\"<'] ".contains(c)).unwrap_or(t.len());
    let file = t[..end].rsplit('/').next()?;
    let sha = file.get(..40).filter(|d| d.bytes().all(|b| b.is_ascii_hexdigit()))?;
    Some((end, sha.to_ascii_lowercase()))
}

/// Points the links to files of the old site at the stored copies (`files`): the short
/// `upload://…` names, and the addresses under `/uploads/default/` with or without the old host.
/// A picture's link gets its size (`#WxH`), so the cooker can reserve its room and make the sizes it
/// is shown in. Links to files that are not in `files` stay as they were.
fn rewrite_uploads(raw: &str, files: &Files) -> String {
    if files.is_empty() {
        return raw.to_owned();
    }
    let raw = strip_link_hints(raw);
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw.as_str();
    loop {
        let short = rest.find("upload://");
        let long = rest.find("/uploads/default/");
        let (i, is_short) = match (short, long) {
            (None, None) => break,
            (Some(a), None) => (a, true),
            (None, Some(b)) => (b, false),
            (Some(a), Some(b)) => (a.min(b), a < b),
        };
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let found = if is_short { short_ref(tail).map(|(n, name)| (n, name.to_owned())) } else { long_ref(tail) };
        let hit = found.and_then(|(n, name)| files.get(&name).map(|f| (n, f)));
        let Some((n, file)) = hit else {
            // Not ours to rewrite: copy one character on and look again after it.
            let step = tail.chars().next().map_or(1, char::len_utf8);
            out.push_str(&tail[..step]);
            rest = &tail[step..];
            continue;
        };
        if !is_short {
            // The old address may carry its host: that goes too.
            let start = out.rfind(|c: char| c.is_whitespace() || "(\"<'[".contains(c)).map_or(0, |p| p + 1);
            let host = &out[start..];
            let after_scheme = host.strip_prefix("https://").or_else(|| host.strip_prefix("http://"));
            if after_scheme.is_some_and(|h| !h.contains('/')) {
                out.truncate(start);
            }
        }
        let size = match (file.image && in_image_link(&out), file.width, file.height) {
            (true, Some(w), Some(h)) => format!("#{w}x{h}"),
            _ => String::new(),
        };
        out.push_str(&format!("/uploads/{}{size}", file.key));
        rest = &tail[n..];
    }
    out.push_str(rest);
    out
}

/// Counters for the final line.
#[derive(Default)]
struct Totals {
    users: u32,
    /// Members whose name was not one this forum accepts, and was rewritten.
    renamed: u32,
    /// Forums and categories imported without a grant for guests.
    members_only: u32,
    forums: u32,
    topics: u32,
    posts: u32,
    refused: u32,
    tags: u32,
    groups: u32,
    polls: u32,
    reactions: u32,
    conversations: u32,
    /// Files stored, files the export named that were not (missing, or not accepted here), and
    /// files not read at all for want of `BF_IMPORT_FILES`.
    uploads: u32,
    files_failed: u32,
    files_skipped: u32,
}

type Res<T> = Result<T, Box<dyn std::error::Error>>;

pub async fn run(path: &str) -> Result<String, Box<dyn std::error::Error>> {
    let config = bf_core::config::Config::from_env()?;
    let pool = bf_db::connect(&config.database_url, 4).await?;
    bf_db::migrate(&pool).await?;
    let plugins = std::env::var("BF_IMPORT_PLUGINS").map_or(true, |v| v != "0");
    let state = crate::state::AppState::new(config.clone(), pool.clone()).await?;
    if plugins && let Err(e) = crate::platform::plugins::reload(&state).await {
        eprintln!("plugins could not be loaded, importing without them: {e}");
    }
    let file = std::io::BufReader::new(std::fs::File::open(path)?);
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Item>(256);
    // Parsing is blocking work: it runs on its own thread and feeds this task through the channel.
    let parser = tokio::task::spawn_blocking(move || -> Result<(), serde_json::Error> {
        use serde::de::DeserializeSeed;
        let mut de = serde_json::Deserializer::from_reader(file);
        Root(&tx).deserialize(&mut de)?;
        de.end()
    });

    let mut ctx = Context {
        pool: &pool,
        state: &state,
        plugins,
        totals: Totals::default(),
        forums: Vec::new(),
        topics: std::collections::HashMap::new(),
        files_root: std::env::var_os("BF_IMPORT_FILES").map(std::path::PathBuf::from),
        files: Files::new(),
    };
    let mut failure: Option<Box<dyn std::error::Error>> = None;
    while let Some(item) = rx.recv().await {
        if let Err(e) = ctx.handle(item).await {
            failure = Some(e);
            break;
        }
    }
    drop(rx);
    let parsed = parser.await?;
    if let Some(e) = failure {
        return Err(e);
    }
    parsed?;
    ctx.flush_forums().await?;
    importer::refresh_counters(&pool).await?;
    // Levels members have earned by what they did on the old site, counted once and silently: the
    // promotion job would tell every one of them, and a backfill is not news.
    crate::community::trust::promote(&state, false).await;
    let t = &ctx.totals;
    let mut out =
        format!("Imported {} members, {} forums, {} topics and {} posts.", t.users, t.forums, t.topics, t.posts);
    let extras: Vec<String> = [
        (t.tags, "tag assignments"),
        (t.groups, "groups"),
        (t.polls, "polls"),
        (t.reactions, "reactions"),
        (t.conversations, "conversations"),
        (t.uploads, "files"),
    ]
    .iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, what)| format!("{n} {what}"))
    .collect();
    if !extras.is_empty() {
        out.push_str(&format!(" Also {}.", extras.join(", ")));
    }
    if t.refused > 0 {
        out.push_str(&format!(" {} posts were refused by plugins.", t.refused));
    }
    if t.files_failed > 0 {
        out.push_str(&format!(" {} files were missing or not accepted here (see above).", t.files_failed));
    }
    if t.files_skipped > 0 {
        out.push_str(&format!(
            " {} files were not read: set BF_IMPORT_FILES to the folder they are in, and images in posts will show.",
            t.files_skipped
        ));
    }
    if t.renamed > 0 {
        out.push_str(&format!(" {} usernames this forum does not accept were rewritten.", t.renamed));
    }
    if t.members_only > 0 {
        out.push_str(&format!(
            " {} forums are readable by members only: mark a forum \"public\": true in the export, or set BF_IMPORT_PUBLIC=1, to let guests read.",
            t.members_only
        ));
    }
    Ok(out)
}

struct Context<'a> {
    pool: &'a sqlx::PgPool,
    state: &'a crate::state::AppState,
    plugins: bool,
    totals: Totals,
    /// Forums wait here until a later section starts: a parent may come after its child.
    forums: Vec<Forum>,
    topics: std::collections::HashMap<i64, Topic>,
    /// `BF_IMPORT_FILES`: where the files of the `uploads` section are.
    files_root: Option<std::path::PathBuf>,
    /// Stored files by the names posts use for them.
    files: Files,
}

impl Context<'_> {
    async fn handle(&mut self, item: Item) -> Res<()> {
        if !matches!(item, Item::Forum(_)) {
            self.flush_forums().await?;
        }
        match item {
            Item::User(u) => self.user(u).await,
            Item::Upload(u) => self.upload(u).await,
            Item::Forum(f) => {
                self.forums.push(f);
                Ok(())
            }
            Item::Topic(t) => {
                // A topic imported before has its views brought up to date, too.
                if let Some(item) = importer::mapped(self.pool, "topic", t.id).await? {
                    if let Some(views) = t.views {
                        importer::set_views(self.pool, item, views).await?;
                    }
                    importer::set_topic_flags(self.pool, item, t.pinned, t.announcement, t.locked).await?;
                }
                self.topics.insert(t.id, t);
                Ok(())
            }
            Item::Post(p) => self.post(p).await,
            Item::Tag(t) => self.tag(t).await,
            Item::Group(g) => self.group(g).await,
            Item::Poll(p) => self.poll(p).await,
            Item::Reaction(r) => self.reaction(r).await,
            Item::Conversation(c) => self.conversation(c).await,
        }
    }

    async fn user(&mut self, u: User) -> Res<()> {
        if importer::mapped(self.pool, "user", u.id).await?.is_none() {
            let hash = usable_hash(u.password_hash.as_deref());
            let unicode = self.state.settings().bool("unicode_usernames");
            let (username, rewritten) = safe_username(&u.username, u.id, unicode);
            let id = importer::import_user(self.pool, &username, &u.email, hash, u.joined_at).await?;
            importer::map(self.pool, "user", u.id, id).await?;
            self.totals.users += 1;
            self.totals.renamed += u32::from(rewritten);
        }
        // Also for a member imported before: running the import again brings their standing up to date.
        if let Some(id) = importer::mapped(self.pool, "user", u.id).await?
            && (u.trust_level.is_some() || u.days_visited.is_some() || u.topics_read.is_some())
        {
            let top = self.state.settings().trust_levels().max_level();
            let level = u.trust_level.unwrap_or(0).clamp(0, top);
            importer::set_standing(self.pool, id, level, u.days_visited.unwrap_or(0).max(0), u.topics_read.unwrap_or(0).max(0)).await?;
        }
        Ok(())
    }

    /// Stores one file of the export and remembers which names posts know it by.
    async fn upload(&mut self, u: UploadRow) -> Res<()> {
        let Some(root) = self.files_root.clone() else {
            self.totals.files_skipped += 1;
            return Ok(());
        };
        let relative = std::path::Path::new(&u.path);
        if !relative.components().all(|c| matches!(c, std::path::Component::Normal(_))) {
            self.totals.files_failed += 1;
            return Ok(());
        }
        let Ok(bytes) = tokio::fs::read(root.join(relative)).await else {
            self.totals.files_failed += 1;
            return Ok(());
        };
        let owner = match u.user_id {
            Some(id) => importer::mapped(self.pool, "user", id).await?,
            None => None,
        };
        let Some(owner) = owner.or(importer::import_placeholder(self.pool).await.ok()) else { return Ok(()) };
        let max_kb = self.state.settings().int("max_upload_kb");
        let stored = match crate::forum::uploads::store_from(
            self.state,
            owner,
            &u.name,
            &bytes,
            crate::forum::uploads::Accept::Files,
            max_kb,
            crate::forum::uploads::Origin::Import,
        )
        .await
        {
            Ok(s) => s,
            Err(e) => {
                eprintln!("file {} not imported: {e:?}", u.name);
                self.totals.files_failed += 1;
                return Ok(());
            }
        };
        let file = Stored {
            key: stored.upload.storage_key.clone(),
            width: stored.upload.width,
            height: stored.upload.height,
            image: stored.is_image,
        };
        for name in u.refs.iter().filter(|r| !r.is_empty()) {
            self.files.insert(hex_lower(name), file.clone());
        }
        if let (Some(legacy), true) = (u.avatar_of, stored.is_image) {
            let small = file.width.zip(file.height).is_some_and(|(w, h)| w <= 2048 && h <= 2048);
            if let (Some(user), true) = (importer::mapped(self.pool, "user", legacy).await?, small) {
                bf_db::forum::uploads::set_avatar(self.pool, user, Some(stored.upload.id)).await?;
            }
        }
        self.totals.uploads += 1;
        Ok(())
    }

    /// The local ids of those source users that were imported.
    async fn users(&self, ids: &[i64]) -> Res<Vec<i64>> {
        let mut out = Vec::new();
        for id in ids {
            out.extend(importer::mapped(self.pool, "user", *id).await?);
        }
        Ok(out)
    }

    async fn flush_forums(&mut self) -> Res<()> {
        if self.forums.is_empty() {
            return Ok(());
        }
        let read = Perms::READER;
        let post = read.union(Perms::CREATE).union(Perms::REPLY).union(Perms::ATTACH);
        // Guests read a forum only when the export says it was public (or the operator says all
        // were, with `BF_IMPORT_PUBLIC=1`): a staff or private forum of the old site must not
        // open to everyone because the format could not say so.
        let all_public = std::env::var("BF_IMPORT_PUBLIC").is_ok_and(|v| v == "1");
        let members: [(i64, Perms); 3] = [(2, post), (3, post.union(Perms::MODERATE)), (4, Perms::ALL)];
        let with_guests: [(i64, Perms); 4] = [(1, read), (2, post), (3, post.union(Perms::MODERATE)), (4, Perms::ALL)];
        // Go round until nothing changes: parents may come after children in the file.
        // Siblings are created in the order the export gives them; the sort is stable, so forums
        // without a position keep the order of the file, after those that have one.
        let mut pending = std::mem::take(&mut self.forums);
        pending.sort_by_key(|f| f.position.unwrap_or(i32::MAX));
        loop {
            let before = pending.len();
            let mut later = Vec::new();
            for f in pending {
                if importer::mapped(self.pool, "forum", f.id).await?.is_some() {
                    continue;
                }
                let mut parent = None;
                if let Some(p) = f.parent_id.filter(|p| *p > 0) {
                    parent = importer::mapped(self.pool, "forum", p).await?;
                    if parent.is_none() {
                        later.push(f);
                        continue;
                    }
                }
                let kind = if f.kind == "category" { "category" } else { "forum" };
                let public = f.public.unwrap_or(all_public);
                let grants: &[(i64, Perms)] = if public { &with_guests } else { &members };
                self.totals.members_only += u32::from(!public);
                let id =
                    bf_db::forum::nodes::create_node(self.pool, parent, kind, &f.title, &slugify(&f.title), "", grants)
                        .await?;
                importer::map(self.pool, "forum", f.id, id).await?;
                self.totals.forums += 1;
            }
            if later.is_empty() || later.len() == before {
                if !later.is_empty() {
                    eprintln!("{} forums skipped: their parent is missing", later.len());
                }
                return Ok(());
            }
            pending = later;
        }
    }

    async fn post(&mut self, p: Post) -> Res<()> {
        let pool = self.pool;
        if importer::mapped(pool, "post", p.id).await?.is_some() {
            return Ok(());
        }
        let Some(topic) = self.topics.get(&p.topic_id) else { return Ok(()) };
        let Some(node) = importer::mapped(pool, "forum", topic.forum_id).await? else { return Ok(()) };
        let author = match p.author_id {
            Some(a) => importer::mapped(pool, "user", a).await?,
            None => None,
        };
        let Some(author) = author.or(importer::import_placeholder(pool).await.ok()) else { return Ok(()) };
        let mut raw = rewrite_uploads(&to_markdown(&p.body, &p.format), &self.files);
        if raw.is_empty() {
            return Ok(());
        }
        // Attachments stay references: a link per file, to be served from the old address or a copy.
        for a in p.attachments.iter().filter(|a| a.url.starts_with("http") || a.url.starts_with('/')) {
            let name = a.name.replace(['[', ']', '\n'], " ");
            raw.push_str(&format!("\n\n[{}]({})", name.trim(), a.url.replace(')', "%29").replace(' ', "%20")));
        }
        if self.plugins {
            let Some(t) = crate::platform::plugins::before_post_system(self.state, author, &raw).await else {
                self.totals.refused += 1;
                return Ok(());
            };
            raw = t;
        }
        let title = if self.plugins {
            crate::platform::plugins::filter_text(self.state, "item.before_create", "title", &topic.title)
                .await
                .unwrap_or_else(|_| topic.title.clone())
        } else {
            topic.title.clone()
        };
        let cooked = bf_cook::cook(&raw);
        let new = bf_db::forum::posts::NewPost {
            author_id: author,
            raw: &raw,
            cooked: &cooked.html,
            cooked_version: bf_cook::COOK_VERSION,
            ip: None,
            pending: false,
        };
        let existing = importer::mapped(pool, "topic", p.topic_id).await?;
        let (item, number) = if let Some(item) = existing {
            (item, bf_db::forum::posts::create_reply(pool, item, node, &new).await?)
        } else {
            let slug = slugify(&title);
            let item = bf_db::forum::posts::create_topic(pool, node, &title, &slug, &new).await?;
            importer::map(pool, "topic", p.topic_id, item).await?;
            let tags: Vec<(String, String)> = topic
                .tags
                .iter()
                .map(|t| (t.trim().to_owned(), slugify(t)))
                .filter(|(n, s)| !n.is_empty() && !s.is_empty())
                .collect();
            if !tags.is_empty() {
                importer::add_tags(pool, item, &tags).await?;
                self.totals.tags += u32::try_from(tags.len()).unwrap_or(0);
            }
            if let Some(views) = topic.views {
                importer::set_views(pool, item, views).await?;
            }
            importer::set_topic_flags(pool, item, topic.pinned, topic.announcement, topic.locked).await?;
            self.totals.topics += 1;
            (item, 1)
        };
        if self.plugins {
            let event = if number == 1 { "topic.created" } else { "post.created" };
            let payload = serde_json::json!({
                "topic_id": item, "post_number": number, "title": title, "node_id": node,
                "author_id": author, "imported": true,
            });
            crate::platform::plugins::dispatch_wait(self.state, event, &payload).await;
        }
        importer::set_post_time(pool, item, number, p.created_at).await?;
        importer::map(pool, "post", p.id, item).await?;
        importer::map(pool, "postnum", p.id, i64::from(number)).await?;
        self.totals.posts += 1;
        Ok(())
    }

    async fn tag(&mut self, t: TagRow) -> Res<()> {
        let Some(item) = importer::mapped(self.pool, "topic", t.topic_id).await? else { return Ok(()) };
        let (name, slug) = (t.name.trim().to_owned(), slugify(&t.name));
        if name.is_empty() || slug.is_empty() {
            return Ok(());
        }
        importer::add_tags(self.pool, item, &[(name, slug)]).await?;
        self.totals.tags += 1;
        Ok(())
    }

    async fn group(&mut self, g: Group) -> Res<()> {
        if importer::mapped(self.pool, "group", g.id).await?.is_some() {
            return Ok(());
        }
        let slug = slugify(&g.name);
        if g.name.trim().is_empty() || slug.is_empty() {
            return Ok(());
        }
        let id = importer::import_group(self.pool, g.name.trim(), &slug).await?;
        let members = self.users(&g.members).await?;
        importer::add_group_members(self.pool, id, &members).await?;
        importer::map(self.pool, "group", g.id, id).await?;
        self.totals.groups += 1;
        Ok(())
    }

    async fn poll(&mut self, p: Poll) -> Res<()> {
        if importer::mapped(self.pool, "poll", p.id).await?.is_some() || p.options.len() < 2 {
            return Ok(());
        }
        let Some(item) = importer::mapped(self.pool, "topic", p.topic_id).await? else { return Ok(()) };
        let mut votes = Vec::new();
        for v in &p.votes {
            votes.extend(importer::mapped(self.pool, "user", v.user_id).await?.map(|u| (u, v.option, v.rank)));
        }
        let post_number = match p.post_id {
            Some(post) => importer::mapped(self.pool, "postnum", post).await?.and_then(|n| i32::try_from(n).ok()).filter(|n| *n > 1),
            None => None,
        };
        let (groups, _) = bf_db::community::polls::groups_by_name(self.pool, &p.groups).await?;
        let kind = match p.kind.as_str() {
            k @ ("ranked" | "rating") => k,
            _ => "choice",
        };
        let results = match p.results.as_str() {
            r @ ("voted" | "closed" | "staff") => r,
            _ => "always",
        };
        let new = bf_db::community::polls::NewPoll {
            question: &p.question,
            options: &p.options,
            max_choices: p.max_choices,
            closes_at: p.closes_at,
            results,
            kind,
            is_public: p.public,
            final_votes: p.final_votes,
            min_trust_level: p.min_trust_level.clamp(0, bf_core::trust::LIMIT),
            post_number,
            groups: &groups,
        };
        if let Some(id) = importer::import_poll(self.pool, item, &new, &votes).await? {
            importer::map(self.pool, "poll", p.id, id).await?;
            self.totals.polls += 1;
        }
        Ok(())
    }

    async fn reaction(&mut self, r: Reaction) -> Res<()> {
        let pool = self.pool;
        let (Some(item), Some(number), Some(user)) = (
            importer::mapped(pool, "post", r.post_id).await?,
            importer::mapped(pool, "postnum", r.post_id).await?,
            importer::mapped(pool, "user", r.user_id).await?,
        ) else {
            return Ok(());
        };
        let Some(post) = importer::post_row(pool, item, i32::try_from(number)?).await? else { return Ok(()) };
        let like = bf_db::forum::reactions::like_reaction_id(pool).await?;
        bf_db::forum::reactions::set_reaction(pool, post, user, Some(like)).await?;
        self.totals.reactions += 1;
        Ok(())
    }

    async fn conversation(&mut self, c: Conversation) -> Res<()> {
        let pool = self.pool;
        if importer::mapped(pool, "pm", c.id).await?.is_some() {
            return Ok(());
        }
        let mut people = self.users(&c.participants).await?;
        let mut messages = Vec::new();
        for m in &c.messages {
            let Some(author) = importer::mapped(pool, "user", m.author_id).await? else { continue };
            let raw = rewrite_uploads(&to_markdown(&m.body, &m.format), &self.files);
            if raw.is_empty() {
                continue;
            }
            if !people.contains(&author) {
                people.push(author);
            }
            let cooked = bf_cook::cook(&raw).html;
            messages.push((author, raw, cooked, m.created_at));
        }
        if people.len() < 2 {
            return Ok(());
        }
        let subject: String = c.subject.trim().chars().take(200).collect();
        if let Some(id) = importer::import_conversation(pool, &subject, &people, &messages).await? {
            importer::map(pool, "pm", c.id, id).await?;
            self.totals.conversations += 1;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_becomes_markdown() {
        assert_eq!(html_to_markdown("<p>Hi <strong>you</strong> &amp; me</p>"), "Hi **you** & me");
        assert_eq!(html_to_markdown("<a href=\"https://a.example/\">x</a>"), "[x](https://a.example/)");
        assert_eq!(html_to_markdown("a<br>b"), "a\nb");
    }

    #[test]
    fn scripts_lose_their_tags() {
        assert!(!html_to_markdown("<script>alert(1)</script>ok").contains("<script"));
    }

    #[test]
    fn bbcode_becomes_markdown() {
        assert_eq!(bbcode_to_markdown("[b]x[/b]"), "**x**");
        assert_eq!(bbcode_to_markdown("[url=https://a.example/]y[/url]"), "[y](https://a.example/)");
    }

    fn files() -> Files {
        let mut f = Files::new();
        let img = Stored { key: "abc.jpg".into(), width: Some(800), height: Some(600), image: true };
        let pdf = Stored { key: "def.pdf".into(), width: None, height: None, image: false };
        f.insert("Tok123".into(), img.clone());
        f.insert("a".repeat(40), img);
        f.insert("Pdf456".into(), pdf);
        f
    }

    #[test]
    fn short_links_become_stored_files_with_their_size() {
        let out = rewrite_uploads("![shot|690x388](upload://Tok123.jpeg) text", &files());
        assert_eq!(out, "![shot](/uploads/abc.jpg#800x600) text");
    }

    #[test]
    fn attachments_keep_their_link_without_a_size() {
        let out = rewrite_uploads("[guide.pdf|attachment](upload://Pdf456.pdf) (12 KB)", &files());
        assert_eq!(out, "[guide.pdf](/uploads/def.pdf) (12 KB)");
    }

    #[test]
    fn old_addresses_go_with_their_host() {
        let sha = "a".repeat(40);
        let raw = format!(
            "see https://anyforum.pl/uploads/default/original/2X/a/{sha}.jpeg and ![](/uploads/default/optimized/2X/a/{sha}_2_690x388.jpeg)"
        );
        let out = rewrite_uploads(&raw, &files());
        assert_eq!(out, "see /uploads/abc.jpg and ![](/uploads/abc.jpg#800x600)");
    }

    #[test]
    fn unknown_files_and_other_text_stay() {
        let raw = "![x](upload://Nope.png) and /uploads/default/other/1.png and upload:// alone";
        assert_eq!(rewrite_uploads(raw, &files()), raw);
        assert_eq!(rewrite_uploads("a | b 3x4 c", &files()), "a | b 3x4 c");
    }

    #[test]
    fn size_hints_only_go_from_link_text() {
        assert_eq!(
            strip_link_hints("![a|10x20,50%](u) and x|1x2 and [b|attachment](u)"),
            "![a](u) and x|1x2 and [b](u)"
        );
    }
}
