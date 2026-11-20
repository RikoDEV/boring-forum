//! Full-text search and the search log.
//!
//! The matching rules, in one place:
//! - Text is matched with the `bf_search` configuration: accents are ignored
//!   (`zolw` finds `żółw`) and English words are stemmed. Postgres ships no Polish or Arabic
//!   dictionary, so those words match exactly or by prefix, and typos are caught by trigram
//!   similarity ("did you mean", and a title fallback when nothing matches).
//! - The query is built by [`build_tsquery`] and handed to Postgres as one bound parameter; the
//!   user's words never reach the SQL text.
//! - The last word of a query also matches as a prefix (`kot` finds `koty`).

use chrono::{DateTime, Utc};
pub use sqlx::Error;
use sqlx::postgres::Postgres;
use sqlx::{FromRow, PgPool, QueryBuilder};
use std::collections::HashMap;

#[derive(Debug, Clone, FromRow)]
pub struct SearchHit {
    pub item_id: i64,
    pub post_id: i64,
    pub title: String,
    pub slug: String,
    pub post_number: i32,
    pub snippet: String,
    pub author: Option<String>,
    pub node_id: i64,
    pub created_at: DateTime<Utc>,
    /// All matches, not just this page.
    pub total: i64,
    /// Other matching posts of the same topic folded into this hit (0 when not grouped).
    pub more_in_topic: i64,
    pub like_count: i32,
    pub reply_count: i32,
}

/// Search filters. `nodes` is already limited to what the viewer may read.
#[derive(Debug, Default)]
pub struct SearchFilter<'a> {
    pub author: Option<&'a str>,
    pub after: Option<chrono::NaiveDate>,
    pub before: Option<chrono::NaiveDate>,
    pub topics_only: bool,
    pub tags: &'a [String],
    /// Topics with any of these tags are left out.
    pub not_tags: &'a [String],
    pub closed: Option<bool>,
    pub min_likes: Option<i32>,
    /// Only the viewer's own posts.
    pub mine_of: Option<i64>,
    /// Only posts the viewer bookmarked.
    pub bookmarks_of: Option<i64>,
    /// Inside one topic.
    pub topic: Option<i64>,
    /// `image`, `link`, `poll`, `code`, `attachment`: the post must have all of them.
    pub has: &'a [String],
    pub solved: Option<bool>,
    pub min_replies: Option<i32>,
    pub max_replies: Option<i32>,
    /// Match topic titles only.
    pub title_only: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Relevance,
    Newest,
    Oldest,
    Likes,
    Replies,
}

impl Sort {
    /// `relevance`, `newest`, `oldest`, `likes` or `replies`; anything else is relevance.
    pub fn parse(s: &str) -> Sort {
        match s {
            "newest" | "latest" => Sort::Newest,
            "oldest" => Sort::Oldest,
            "likes" => Sort::Likes,
            "replies" => Sort::Replies,
            _ => Sort::Relevance,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Sort::Relevance => "relevance",
            Sort::Newest => "newest",
            Sort::Oldest => "oldest",
            Sort::Likes => "likes",
            Sort::Replies => "replies",
        }
    }

    /// Static SQL over the columns of the candidate set; never built from user text.
    fn order_by(self) -> &'static str {
        match self {
            Sort::Relevance => "rank DESC, created_at DESC, post_id DESC",
            Sort::Newest => "created_at DESC, post_id DESC",
            Sort::Oldest => "created_at ASC, post_id ASC",
            Sort::Likes => "like_count DESC, created_at DESC, post_id DESC",
            Sort::Replies => "reply_count DESC, created_at DESC, post_id DESC",
        }
    }
}

pub struct SearchRequest<'a> {
    /// The words of the query, operators already taken out (see `bf_core::search::parse`).
    pub text: &'a str,
    pub nodes: &'a [i64],
    pub filter: &'a SearchFilter<'a>,
    pub sort: Sort,
    /// One hit per topic, with the count of its other matching posts.
    pub group: bool,
    pub limit: i64,
    pub offset: i64,
    /// Also count forums, tags and years of the whole result set.
    pub facets: bool,
}

/// One facet value: `key` is what a filter takes (a forum id, a tag slug, a year), `label` what
/// to show (the forum's label is filled in by the caller, who knows the forum tree).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacetCount {
    pub key: String,
    pub label: String,
    pub count: i64,
}

#[derive(Debug, Clone, Default)]
pub struct Facets {
    /// Top 10 forums by matching topics.
    pub forums: Vec<FacetCount>,
    /// Top 10 tags.
    pub tags: Vec<FacetCount>,
    /// Newest year first.
    pub years: Vec<FacetCount>,
    /// True when more than [`FACET_CAP`] topics matched: the counts cover the first ones only.
    pub capped: bool,
}

/// How many matching topics the facet counts look at.
pub const FACET_CAP: i64 = 3000;

#[derive(Debug, Clone, FromRow)]
pub struct BestBet {
    pub item_id: i64,
    pub title: String,
    pub slug: String,
    pub node_id: i64,
    pub created_at: DateTime<Utc>,
}

/// What a search returns. The contract for the page and the API:
/// - `hits`: the page of results, in the requested sort. Each carries `total` (all matches) and,
///   when grouped, `more_in_topic`.
/// - `total`: matches over all pages (topics when grouped, posts otherwise).
/// - `suggestion`: a "did you mean" text (the corrected words of the query) when the query matched
///   few topics and some word is close to a title or tag word; `corrections` lists the
///   `(typed, suggested)` pairs so the caller can rewrite the full query.
/// - `corrected`: nothing matched what was typed, so `hits` are the results for `suggestion`.
/// - `fuzzy`: nothing matched at all, so `hits` are topics whose titles are merely similar.
/// - `facets`: counts for the whole result set, only when asked for.
/// - `best_bets`: topics staff pinned to this exact query (only ones the viewer may read), not
///   included in `hits` ranking rules; empty beyond page one.
/// - `expanded`: synonyms the query was widened with, for display.
#[derive(Debug, Default)]
pub struct SearchOutcome {
    pub hits: Vec<SearchHit>,
    pub total: i64,
    pub suggestion: Option<String>,
    pub corrections: Vec<(String, String)>,
    pub corrected: bool,
    pub fuzzy: bool,
    pub facets: Facets,
    pub best_bets: Vec<BestBet>,
    pub expanded: Vec<String>,
}

/// The older entry point: matching posts only, relevance order, one hit per post.
pub async fn search(
    pool: &PgPool,
    q: &str,
    nodes: &[i64],
    f: &SearchFilter<'_>,
    limit: i64,
    offset: i64,
) -> Result<Vec<SearchHit>, Error> {
    let req = SearchRequest { text: q, nodes, filter: f, sort: Sort::Relevance, group: false, limit, offset, facets: false };
    Ok(search_full(pool, &req).await?.hits)
}

// ---- the query ------------------------------------------------------------------------------

/// Lowercase letter and digit runs of `s`, each cut at 64 characters.
fn pieces(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|p| !p.is_empty())
        .map(|p| p.to_lowercase().chars().take(64).collect())
        .collect()
}

fn phrase(ps: &[String]) -> String {
    let q: Vec<String> = ps.iter().map(|p| format!("'{p}'")).collect();
    if q.len() == 1 { q[0].clone() } else { format!("({})", q.join(" <-> ")) }
}

struct Term {
    expr: String,
    /// The word itself when it is one plain positive word (synonyms and prefix apply).
    plain: Option<String>,
    or: bool,
}

const MAX_TERMS: usize = 12;
const MAX_SYNONYMS: usize = 8;

/// The plain words of `text` that synonyms may widen (positive, unquoted, one piece), in order.
pub fn plain_words(text: &str) -> Vec<String> {
    terms(text).into_iter().filter_map(|t| t.plain).collect()
}

fn terms(text: &str) -> Vec<Term> {
    let c: Vec<char> = text.chars().collect();
    let (n, mut i) = (c.len(), 0);
    let mut out: Vec<Term> = Vec::new();
    let mut or = false;
    while i < n && out.len() < MAX_TERMS {
        if c[i].is_whitespace() {
            i += 1;
            continue;
        }
        let neg = c[i] == '-' && i + 1 < n && !c[i + 1].is_whitespace();
        if neg {
            i += 1;
        }
        let (ps, plain_ok) = if c[i] == '"' {
            let end = c[i + 1..].iter().position(|&x| x == '"').map_or(n, |p| i + 1 + p);
            let ps = pieces(&c[i + 1..end].iter().collect::<String>());
            i = (end + 1).min(n);
            (ps, false)
        } else {
            let end = c[i..].iter().position(|x| x.is_whitespace()).map_or(n, |p| i + p);
            let w: String = c[i..end].iter().collect();
            i = end;
            if !neg && w.eq_ignore_ascii_case("or") {
                or = !out.is_empty();
                continue;
            }
            (pieces(&w), true)
        };
        if ps.is_empty() {
            continue;
        }
        let plain = (plain_ok && !neg && ps.len() == 1).then(|| ps[0].clone());
        let expr = if neg { format!("!{}", phrase(&ps)) } else { phrase(&ps) };
        out.push(Term { expr, plain, or: std::mem::take(&mut or) });
    }
    out
}

/// A `to_tsquery('bf_search', …)` text for the user's words, or `None` when they hold no word.
/// Words are ANDed, `or` between words ORs them, `"a b"` is a phrase, `-word` excludes, the last
/// plain word also matches as a prefix, and `synonyms` (word to alternatives) widen a word. Every
/// lexeme is a quoted run of letters and digits, so nothing a user types can be syntax.
pub fn build_tsquery(text: &str, synonyms: &HashMap<String, Vec<String>>) -> Option<String> {
    let ts = terms(text);
    if ts.is_empty() {
        return None;
    }
    let last = ts.len() - 1;
    let mut q = String::new();
    for (idx, t) in ts.iter().enumerate() {
        if idx > 0 {
            q.push_str(if t.or { " | " } else { " & " });
        }
        let prefix = idx == last && t.plain.as_ref().is_some_and(|w| w.chars().count() >= 3);
        match t.plain.as_ref() {
            Some(w) => {
                let alts = synonyms.get(w).map(|v| &v[..v.len().min(MAX_SYNONYMS)]).unwrap_or_default();
                let head = if prefix { format!("'{w}':*") } else { format!("'{w}'") };
                let mut group = vec![head];
                for a in alts {
                    let ps = pieces(a);
                    if !ps.is_empty() {
                        group.push(phrase(&ps));
                    }
                }
                if group.len() == 1 { q.push_str(&group[0]) } else { q.push_str(&format!("({})", group.join(" | "))) }
            }
            None => q.push_str(&t.expr),
        }
    }
    Some(q)
}

/// Synonym groups touching any of `words`: word to the other words of its group. One step only,
/// at most [`MAX_SYNONYMS`] each; a database without the table simply has none.
async fn load_synonyms(pool: &PgPool, words: &[String]) -> HashMap<String, Vec<String>> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    if words.is_empty() {
        return map;
    }
    let rows: Vec<(String, Vec<String>)> = sqlx::query_as(
        "SELECT term, synonyms FROM search_synonyms
         WHERE lower(term) = ANY($1) OR EXISTS (SELECT 1 FROM unnest(synonyms) s WHERE lower(s) = ANY($1)) LIMIT 50",
    )
    .bind(words)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    for (term, syns) in rows {
        let group: Vec<String> = std::iter::once(term).chain(syns).map(|s| s.trim().to_lowercase()).collect();
        for w in words {
            if group.contains(w) {
                let e = map.entry(w.clone()).or_default();
                for g in &group {
                    if g != w && !g.is_empty() && !e.contains(g) && e.len() < MAX_SYNONYMS {
                        e.push(g.clone());
                    }
                }
            }
        }
    }
    map
}

#[derive(Clone, Copy)]
enum Matcher<'a> {
    /// No words: every row that passes the filters.
    All,
    /// A `to_tsquery('bf_search', …)` text.
    Ts(&'a str),
    /// Titles trigram-similar to the plain words (the typo fallback).
    Trgm(&'a str),
}

/// `FROM posts … WHERE …` with every filter, shared by the hits and the facets.
fn push_base(qb: &mut QueryBuilder<Postgres>, req: &SearchRequest<'_>, m: Matcher<'_>) {
    let f = req.filter;
    qb.push(" FROM posts p JOIN items i ON i.id = p.item_id LEFT JOIN users u ON u.id = p.author_id CROSS JOIN (SELECT ");
    match m {
        Matcher::Ts(t) => {
            qb.push("to_tsquery('bf_search', ").push_bind(t.to_owned()).push(") AS query");
        }
        _ => {
            qb.push("NULL::tsquery AS query");
        }
    }
    qb.push(") q WHERE ");
    match m {
        Matcher::All => {
            qb.push("TRUE");
        }
        Matcher::Ts(_) if f.title_only => {
            qb.push("(i.search_vector @@ q.query AND p.post_number = 1)");
        }
        Matcher::Ts(_) => {
            qb.push("(p.search_vector @@ q.query OR (i.search_vector @@ q.query AND p.post_number = 1))");
        }
        Matcher::Trgm(t) => {
            qb.push("(p.post_number = 1 AND bf_unaccent(lower(").push_bind(t.to_owned());
            qb.push(")) <% bf_unaccent(lower(i.title)))");
        }
    }
    qb.push(" AND p.state = 'visible' AND p.action IS NULL AND i.state = 'visible' AND (i.listed OR ");
    qb.push_bind(f.topic).push("::bigint IS NOT NULL) AND i.node_id = ANY(").push_bind(req.nodes).push(")");
    if let Some(a) = f.author {
        qb.push(" AND p.author_id = (SELECT id FROM users WHERE username_lower = lower(").push_bind(a).push("))");
    }
    if let Some(d) = f.after {
        qb.push(" AND p.created_at >= ").push_bind(d);
    }
    if let Some(d) = f.before {
        qb.push(" AND p.created_at < ").push_bind(d);
    }
    if f.topics_only || f.title_only {
        qb.push(" AND p.post_number = 1");
    }
    if !f.tags.is_empty() {
        qb.push(" AND EXISTS (SELECT 1 FROM item_tags it JOIN tags tg ON tg.id = it.tag_id WHERE it.item_id = i.id AND tg.slug = ANY(");
        qb.push_bind(f.tags).push("))");
    }
    if !f.not_tags.is_empty() {
        qb.push(" AND NOT EXISTS (SELECT 1 FROM item_tags it JOIN tags tg ON tg.id = it.tag_id WHERE it.item_id = i.id AND tg.slug = ANY(");
        qb.push_bind(f.not_tags).push("))");
    }
    if let Some(c) = f.closed {
        qb.push(" AND i.locked = ").push_bind(c);
    }
    if let Some(n) = f.min_likes {
        qb.push(" AND p.like_count >= ").push_bind(n);
    }
    if let Some(u) = f.mine_of {
        qb.push(" AND p.author_id = ").push_bind(u);
    }
    if let Some(u) = f.bookmarks_of {
        qb.push(" AND EXISTS (SELECT 1 FROM bookmarks b WHERE b.post_id = p.id AND b.user_id = ").push_bind(u).push(")");
    }
    if let Some(t) = f.topic {
        qb.push(" AND i.id = ").push_bind(t);
    }
    match f.solved {
        Some(true) => {
            qb.push(" AND i.solved_post_number IS NOT NULL");
        }
        Some(false) => {
            qb.push(" AND i.solved_post_number IS NULL");
        }
        None => {}
    }
    if let Some(n) = f.min_replies {
        qb.push(" AND i.reply_count >= ").push_bind(n);
    }
    if let Some(n) = f.max_replies {
        qb.push(" AND i.reply_count <= ").push_bind(n);
    }
    for h in f.has {
        // Fixed fragments per operator; `has` only ever holds the five parsed names.
        qb.push(match h.as_str() {
            "image" => {
                " AND (p.raw LIKE '%![%' OR EXISTS (SELECT 1 FROM upload_uses uu JOIN uploads up ON up.id = uu.upload_id
                   WHERE uu.kind = 'post' AND uu.ref_id = p.id AND up.mime LIKE 'image/%'))"
            }
            "attachment" => {
                " AND EXISTS (SELECT 1 FROM upload_uses uu JOIN uploads up ON up.id = uu.upload_id
                   WHERE uu.kind = 'post' AND uu.ref_id = p.id AND up.mime NOT LIKE 'image/%')"
            }
            "link" => " AND EXISTS (SELECT 1 FROM post_links pl WHERE pl.post_id = p.id)",
            "poll" => " AND EXISTS (SELECT 1 FROM polls pl WHERE pl.item_id = i.id)",
            "code" => " AND p.raw LIKE '%`%'",
            _ => "",
        });
    }
}

/// The rank of a candidate: text relevance (a title match counts double) with a nudge for
/// reactions and for recency.
fn push_rank(qb: &mut QueryBuilder<Postgres>, m: Matcher<'_>) {
    match m {
        Matcher::Ts(_) => {
            qb.push(
                "((ts_rank(p.search_vector, q.query)
                   + CASE WHEN p.post_number = 1 THEN 2 * ts_rank(i.search_vector, q.query) ELSE 0 END)
                  * (1 + 0.1 * ln(1 + p.like_count))
                  * (1 + 1 / (1 + extract(epoch FROM now() - p.created_at) / 2592000)))::float8",
            );
        }
        Matcher::Trgm(t) => {
            qb.push("word_similarity(bf_unaccent(lower(").push_bind(t.to_owned()).push(")), bf_unaccent(lower(i.title)))::float8");
        }
        Matcher::All => {
            qb.push("0::float8");
        }
    }
}

async fn run_hits(pool: &PgPool, req: &SearchRequest<'_>, m: Matcher<'_>) -> Result<Vec<SearchHit>, Error> {
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new("WITH m AS (SELECT p.id AS post_id, i.id AS item_id, ");
    push_rank(&mut qb, m);
    qb.push(" AS rank, p.created_at, p.like_count, i.reply_count, count(*) OVER (PARTITION BY i.id) AS in_topic");
    push_base(&mut qb, req, m);
    qb.push("), g AS (");
    if req.group {
        qb.push("SELECT DISTINCT ON (item_id) * FROM m ORDER BY item_id, rank DESC, post_id DESC");
    } else {
        qb.push("SELECT * FROM m");
    }
    qb.push("), s AS (SELECT g.*, row_number() OVER (ORDER BY ").push(req.sort.order_by());
    qb.push(") AS ord, count(*) OVER () AS total FROM g ORDER BY ord LIMIT ").push_bind(req.limit);
    qb.push(" OFFSET ").push_bind(req.offset);
    qb.push(
        ") SELECT i.id AS item_id, p.id AS post_id, i.title, i.slug, p.post_number, u.username AS author, i.node_id,
                  p.created_at, s.total, ",
    );
    qb.push(if req.group { "s.in_topic - 1" } else { "0::bigint" });
    qb.push(
        " AS more_in_topic, p.like_count, i.reply_count,
          ts_headline('bf_search', bf_strip_hidden(p.raw), ",
    );
    match m {
        Matcher::Ts(t) => {
            qb.push("to_tsquery('bf_search', ").push_bind(t.to_owned()).push(")");
        }
        Matcher::Trgm(t) => {
            qb.push("plainto_tsquery('bf_search', ").push_bind(t.to_owned()).push(")");
        }
        Matcher::All => {
            qb.push("''::tsquery");
        }
    }
    qb.push(
        ", 'MaxFragments=1,MaxWords=30,MinWords=10,StartSel=\"\",StopSel=\"\"') AS snippet
         FROM s JOIN posts p ON p.id = s.post_id JOIN items i ON i.id = p.item_id
         LEFT JOIN users u ON u.id = p.author_id ORDER BY s.ord",
    );
    qb.build_query_as::<SearchHit>().fetch_all(pool).await
}

async fn run_facets(pool: &PgPool, req: &SearchRequest<'_>, m: Matcher<'_>) -> Result<Facets, Error> {
    let mut qb: QueryBuilder<Postgres> =
        QueryBuilder::new("WITH c AS MATERIALIZED (SELECT DISTINCT ON (i.id) i.id AS item_id, i.node_id, p.created_at");
    push_base(&mut qb, req, m);
    qb.push(" ORDER BY i.id LIMIT ").push_bind(FACET_CAP + 1);
    qb.push(
        ")
         (SELECT 'total', '', '', count(*) FROM c)
         UNION ALL (SELECT 'forum', node_id::text, node_id::text, count(*) AS n FROM c GROUP BY node_id ORDER BY n DESC, node_id LIMIT 10)
         UNION ALL (SELECT 'tag', tg.slug, tg.name, count(*) AS n FROM c JOIN item_tags it ON it.item_id = c.item_id
                    JOIN tags tg ON tg.id = it.tag_id GROUP BY tg.id ORDER BY n DESC, tg.slug LIMIT 10)
         UNION ALL (SELECT 'year', y::text, y::text, count(*) FROM (SELECT extract(year FROM created_at)::int AS y FROM c) t
                    GROUP BY y ORDER BY y DESC LIMIT 15)",
    );
    let rows: Vec<(String, String, String, i64)> = qb.build_query_as().fetch_all(pool).await?;
    let mut out = Facets::default();
    for (kind, key, label, count) in rows {
        let f = FacetCount { key, label, count };
        match kind.as_str() {
            "total" => out.capped = count > FACET_CAP,
            "forum" => out.forums.push(f),
            "tag" => out.tags.push(f),
            _ => out.years.push(f),
        }
    }
    Ok(out)
}

/// Fixes for the words of `text` that look misspelled: (typed, suggested). A word is replaced by
/// the closest word of the titles the viewer can read, or of tag names, when that is similar
/// enough and not the same word. Words under four letters are left alone.
async fn corrections(pool: &PgPool, text: &str, nodes: &[i64]) -> Result<Vec<(String, String)>, Error> {
    let words: Vec<String> = text
        .split_whitespace()
        .filter(|w| w.chars().all(char::is_alphanumeric) && w.chars().count() >= 4)
        .map(str::to_owned)
        .take(5)
        .collect();
    if words.is_empty() {
        return Ok(vec![]);
    }
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT set_config('pg_trgm.word_similarity_threshold', '0.4', true), set_config('pg_trgm.similarity_threshold', '0.4', true)")
        .execute(&mut *tx)
        .await?;
    let mut out = Vec::new();
    for w in words {
        let best: Option<(String, f32)> = sqlx::query_as(
            "WITH q AS (SELECT bf_unaccent(lower($1::text)) AS w)
             SELECT c.w, similarity(bf_unaccent(c.w), q.w) AS s FROM q, (
               SELECT regexp_split_to_table(lower(title), '[^[:alnum:]]+') AS w
                 FROM (SELECT i.title FROM items i, q
                       WHERE i.state = 'visible' AND i.listed AND i.node_id = ANY($2) AND q.w <% bf_unaccent(lower(i.title))
                       ORDER BY word_similarity(q.w, bf_unaccent(lower(i.title))) DESC LIMIT 40) t
               UNION
               SELECT regexp_split_to_table(lower(name), '[^[:alnum:]]+')
                 FROM (SELECT t.name FROM tags t, q WHERE q.w <% bf_unaccent(lower(t.name)) LIMIT 20) t2
             ) c WHERE length(c.w) >= 4 AND similarity(bf_unaccent(c.w), q.w) >= 0.4
             ORDER BY s DESC, length(c.w) LIMIT 1",
        )
        .bind(&w)
        .bind(nodes)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((fix, _)) = best
            && bf_fold(&fix) != bf_fold(&w)
        {
            out.push((w, fix));
        }
    }
    tx.commit().await?;
    Ok(out)
}

/// Lowercase with the accents of the languages the forum is written in removed, for comparing a
/// typed word with a stored one in Rust (the database does the same with `bf_unaccent`).
pub fn bf_fold(s: &str) -> String {
    s.chars()
        .map(|c| {
            let c = c.to_lowercase().next().unwrap_or(c);
            match c {
                'ą' | 'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
                'ć' | 'ç' | 'č' => 'c',
                'ę' | 'è' | 'é' | 'ê' | 'ë' | 'ě' => 'e',
                'ł' => 'l',
                'ń' | 'ñ' | 'ň' => 'n',
                'ó' | 'ò' | 'ô' | 'õ' | 'ö' | 'ø' => 'o',
                'ś' | 'š' => 's',
                'ź' | 'ż' | 'ž' => 'z',
                'ù' | 'ú' | 'û' | 'ü' | 'ů' => 'u',
                'ì' | 'í' | 'î' | 'ï' => 'i',
                'ý' | 'ÿ' => 'y',
                other => other,
            }
        })
        .collect()
}

/// Rewrites `text` with `fixes` applied to whole words.
pub fn apply_corrections(text: &str, fixes: &[(String, String)]) -> String {
    text.split_whitespace()
        .map(|w| fixes.iter().find(|(from, _)| from == w).map_or(w, |(_, to)| to.as_str()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Topics staff attached to this exact query that the viewer may read.
pub async fn best_bets(pool: &PgPool, text: &str, nodes: &[i64]) -> Result<Vec<BestBet>, Error> {
    let norm = text.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    if norm.is_empty() {
        return Ok(vec![]);
    }
    sqlx::query_as(
        "SELECT i.id AS item_id, i.title, i.slug, i.node_id, i.created_at
         FROM search_best_bets b JOIN items i ON i.id = b.item_id
         WHERE lower(regexp_replace(trim(b.query), '\\s+', ' ', 'g')) = $1
           AND i.state = 'visible' AND i.node_id = ANY($2)
         ORDER BY b.id LIMIT 5",
    )
    .bind(norm)
    .bind(nodes)
    .fetch_all(pool)
    .await
}

/// The whole search: matches, "did you mean", facets and best bets. See [`SearchOutcome`].
pub async fn search_full(pool: &PgPool, req: &SearchRequest<'_>) -> Result<SearchOutcome, Error> {
    let text = req.text.trim();
    let mut out = SearchOutcome::default();
    let words = plain_words(text);
    let synonyms = load_synonyms(pool, &words).await;
    out.expanded = {
        let mut v: Vec<String> = synonyms.values().flatten().cloned().collect();
        v.sort();
        v.dedup();
        v
    };
    let tsq = build_tsquery(text, &synonyms);
    let matcher = if text.is_empty() {
        Some(Matcher::All)
    } else {
        tsq.as_deref().map(Matcher::Ts)
    };
    let mut used = matcher;
    if let Some(m) = matcher {
        out.hits = run_hits(pool, req, m).await?;
    }
    out.total = out.hits.first().map_or(0, |h| h.total);

    if !text.is_empty() && out.total <= 2 && req.offset == 0 {
        let fixes = corrections(pool, text, req.nodes).await.unwrap_or_default();
        if !fixes.is_empty() {
            let fixed = apply_corrections(text, &fixes);
            // Nothing found: show what the corrected words find, and say so.
            let fixed_q = (out.total == 0).then(|| build_tsquery(&fixed, &HashMap::new())).flatten();
            out.suggestion = Some(fixed);
            out.corrections = fixes;
            if let Some(fq) = fixed_q.as_deref() {
                let hits = run_hits(pool, req, Matcher::Ts(fq)).await?;
                if !hits.is_empty() {
                    out.corrected = true;
                    out.total = hits[0].total;
                    out.hits = hits;
                    // The facets below must describe what is shown.
                    return finish(pool, req, out, Some(fq.to_owned())).await;
                }
            }
        }
    }
    if out.total == 0 && !text.is_empty() && req.offset == 0 && !plain_words(text).is_empty() {
        let hits = run_hits(pool, req, Matcher::Trgm(text)).await?;
        if !hits.is_empty() {
            out.fuzzy = true;
            out.total = hits[0].total;
            out.hits = hits;
            used = None;
        }
    }
    let used_q = match used {
        Some(Matcher::Ts(t)) => Some(t.to_owned()),
        _ => None,
    };
    if out.total > 0 && req.facets {
        let m = match (&used_q, used) {
            (Some(t), _) => Matcher::Ts(t),
            (None, Some(Matcher::All)) => Matcher::All,
            _ => Matcher::Trgm(text),
        };
        out.facets = run_facets(pool, req, m).await?;
    }
    if req.offset == 0 && !text.is_empty() {
        out.best_bets = best_bets(pool, text, req.nodes).await.unwrap_or_default();
    }
    Ok(out)
}

/// The tail of [`search_full`] for results found by corrected words.
async fn finish(pool: &PgPool, req: &SearchRequest<'_>, mut out: SearchOutcome, tsq: Option<String>) -> Result<SearchOutcome, Error> {
    if req.facets && let Some(t) = tsq.as_deref() {
        out.facets = run_facets(pool, req, Matcher::Ts(t)).await?;
    }
    if req.offset == 0 {
        out.best_bets = best_bets(pool, req.text, req.nodes).await.unwrap_or_default();
    }
    Ok(out)
}

/// Reciprocal rank fusion of ranked id lists: an id's score is the sum of `1 / (k + rank)` over
/// the lists that hold it (rank from 1), so what several lists agree on rises. Best first; ties
/// keep the order of the first list that had the id.
pub fn rrf(lists: &[&[i64]], k: f64) -> Vec<i64> {
    let mut score: Vec<(i64, f64)> = Vec::new();
    for list in lists {
        for (rank, id) in list.iter().enumerate() {
            let add = 1.0 / (k + rank as f64 + 1.0);
            match score.iter_mut().find(|(i, _)| i == id) {
                Some((_, s)) => *s += add,
                None => score.push((*id, add)),
            }
        }
    }
    score.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    score.into_iter().map(|(id, _)| id).collect()
}

pub async fn log_search(pool: &PgPool, term: &str, results: i32, user: Option<i64>) -> Result<(), Error> {
    sqlx::query("INSERT INTO search_logs (term, results, user_id) VALUES ($1, $2, $3)")
        .bind(term)
        .bind(results)
        .bind(user)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Most searched terms in the last 30 days: (term, searches, average results).
pub async fn search_report(pool: &PgPool) -> Result<Vec<(String, i64, f64)>, Error> {
    sqlx::query_as(
        "SELECT term, count(*), avg(results)::float8 FROM search_logs
         WHERE created_at > now() - interval '30 days' GROUP BY term ORDER BY count(*) DESC LIMIT 100",
    )
    .fetch_all(pool)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none() -> HashMap<String, Vec<String>> {
        HashMap::new()
    }

    #[test]
    fn plain_words_and_prefix() {
        assert_eq!(build_tsquery("żółw kot", &none()).as_deref(), Some("'żółw' & 'kot':*"));
        assert_eq!(build_tsquery("ab", &none()).as_deref(), Some("'ab'"));
        assert_eq!(build_tsquery("", &none()), None);
        assert_eq!(build_tsquery("?! ...", &none()), None);
    }

    #[test]
    fn syntax_from_users_is_inert() {
        let q = build_tsquery("a' | b:* & (c) <-> !d \\ ; DROP", &none()).unwrap();
        assert!(!q.contains(';') && !q.contains('\\') && !q.contains("(c)"));
        assert!(q.starts_with("'a' & 'b'"), "{q}");
    }

    #[test]
    fn or_phrases_and_exclusion() {
        assert_eq!(build_tsquery("cats or dogs", &none()).as_deref(), Some("'cats' | 'dogs':*"));
        assert_eq!(build_tsquery("\"red fox\" -fast", &none()).as_deref(), Some("('red' <-> 'fox') & !'fast'"));
        assert_eq!(build_tsquery("e-mail setup", &none()).as_deref(), Some("('e' <-> 'mail') & 'setup':*"));
        assert_eq!(build_tsquery("or cats", &none()).as_deref(), Some("'cats':*"));
        assert_eq!(build_tsquery("\"unclosed phrase", &none()).as_deref(), Some("('unclosed' <-> 'phrase')"));
    }

    #[test]
    fn synonyms_widen_a_word() {
        let mut s = none();
        s.insert("pc".into(), vec!["computer".into(), "desk top".into()]);
        assert_eq!(
            build_tsquery("pc fix", &s).as_deref(),
            Some("('pc' | 'computer' | ('desk' <-> 'top')) & 'fix':*")
        );
        // A word shorter than three letters is never a prefix, with or without synonyms: it would
        // match far too much.
        assert_eq!(build_tsquery("fix pc", &s).as_deref(), Some("'fix' & ('pc' | 'computer' | ('desk' <-> 'top'))"));
        assert_eq!(build_tsquery("fix pcs", &s).as_deref(), Some("'fix' & 'pcs':*"));
    }

    #[test]
    fn terms_are_capped() {
        let q = build_tsquery(&"word ".repeat(40), &none()).unwrap();
        assert_eq!(q.matches('&').count(), MAX_TERMS - 1);
    }

    #[test]
    fn corrections_apply_to_whole_words() {
        let fixes = vec![("zolv".to_string(), "żółw".to_string())];
        assert_eq!(apply_corrections("moj zolv zolvy", &fixes), "moj żółw zolvy");
        assert_eq!(bf_fold("Żółć"), "zolc");
    }

    #[test]
    fn fusion_rewards_agreement() {
        assert_eq!(rrf(&[&[1, 2, 3], &[3, 4]], 60.0), vec![3, 1, 2, 4]);
        assert_eq!(rrf(&[&[1, 2], &[]], 60.0), vec![1, 2]);
        assert!(rrf(&[], 60.0).is_empty());
    }

    #[test]
    fn sorts_parse() {
        assert_eq!(Sort::parse("likes"), Sort::Likes);
        assert_eq!(Sort::parse("latest"), Sort::Newest);
        assert_eq!(Sort::parse("whatever"), Sort::Relevance);
        assert_eq!(Sort::Oldest.as_str(), "oldest");
    }
}
