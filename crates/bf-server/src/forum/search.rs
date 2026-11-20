//! Search pages and the search API.

use crate::forum::topics::post_url;
use crate::forum::tree::Tree;
use crate::http::render::layout;
use crate::http::render::page;
use crate::http::render::{DbResult, ago};
use crate::http::viewer::Viewer;
use crate::http::viewer::is_staff;
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use bf_core::AppError;
use bf_core::perms::Perms;
use bf_core::slug::{forum_path, slugify};
use bf_db::accounts::users;
use bf_db::forum::nodes::Node;
use bf_db::forum::{search, tags};
use bf_web::pages::{FacetLink, SearchFacets, SearchPage, SearchResult, render};
use serde::Deserialize;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new().route("/search", get(search)).route("/api/v1/search", get(search_api))
}

#[derive(Deserialize, Default)]
pub(crate) struct SearchQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    forum: String,
    #[serde(default)]
    after: String,
    #[serde(default)]
    before: String,
    #[serde(default)]
    page: i64,
    #[serde(default, rename = "in")]
    scope: String,
    topic: Option<i64>,
    #[serde(default)]
    edit_layout: String,
    /// `relevance` (default), `newest`, `oldest`, `likes` or `replies`; `order:` in the query is
    /// used when this is empty.
    #[serde(default)]
    sort: String,
    /// `0` lists every matching post instead of one hit per topic.
    #[serde(default)]
    group: String,
}

const SEARCH_PAGE: i64 = 20;
/// Keyword hits looked at when merging with the AI's list: three pages.
const FUSION_WINDOW: i64 = SEARCH_PAGE * 3;

/// Everything a search produced, for the page and the API.
pub(crate) struct Run {
    q: String,
    results: Vec<SearchResult>,
    total: i64,
    best_bets: Vec<SearchResult>,
    /// The full query with misspelled words fixed.
    suggestion: Option<String>,
    corrected: bool,
    fuzzy: bool,
    facets: search::Facets,
    sort: search::Sort,
    grouped: bool,
}

impl Run {
    fn empty(q: String) -> Run {
        Run {
            q,
            results: vec![],
            total: 0,
            best_bets: vec![],
            suggestion: None,
            corrected: false,
            fuzzy: false,
            facets: search::Facets::default(),
            sort: search::Sort::Relevance,
            grouped: true,
        }
    }
}

/// `YYYY-MM-DD`, or `7d`/`2w`/`3m`/`1y` counted back from `today`.
fn resolve_date(s: &str, today: chrono::NaiveDate) -> Option<chrono::NaiveDate> {
    let s = s.trim();
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .ok()
        .or_else(|| bf_core::search::relative_days(s).and_then(|d| today.checked_sub_signed(chrono::Duration::days(d))))
}

/// The ids of `matched` forums and everything below them.
fn with_descendants(nodes: &[Node], matched: Vec<i64>) -> Vec<i64> {
    let mut out = matched;
    let mut i = 0;
    while i < out.len() && out.len() < 5000 {
        let id = out[i];
        let below: Vec<i64> =
            nodes.iter().filter(|c| c.parent_id == Some(id) && !out.contains(&c.id)).map(|c| c.id).collect();
        out.extend(below);
        i += 1;
    }
    out
}

fn hit_result(state: &AppState, tree: &Tree, h: search::SearchHit, text: &str, by_meaning: bool) -> SearchResult {
    let node = tree.node(h.node_id);
    SearchResult {
        url: post_url(state, h.item_id, &h.slug, h.post_number),
        title_parts: highlight(&h.title, text),
        snippet_parts: highlight(&h.snippet, text),
        more_url: topic_search_url(text, h.item_id),
        title: h.title,
        snippet: h.snippet,
        author: h.author.unwrap_or_else(|| "deleted".into()),
        forum: node.map(|n| n.title.clone()).unwrap_or_default(),
        forum_url: node.map(|n| forum_path(n.id, &n.slug)).unwrap_or_default(),
        when: ago(h.created_at),
        number: h.post_number,
        by_meaning,
        more_in_topic: h.more_in_topic,
        likes: h.like_count,
        replies: h.reply_count,
    }
}

fn topic_search_url(text: &str, topic: i64) -> String {
    let mut u = url::form_urlencoded::Serializer::new(String::new());
    u.append_pair("q", text).append_pair("topic", &topic.to_string());
    format!("/search?{}", u.finish())
}

fn best_bet_results(state: &AppState, tree: &Tree, bets: &[search::BestBet], text: &str) -> Vec<SearchResult> {
    bets.iter()
        .map(|b| {
            let node = tree.node(b.node_id);
            SearchResult {
                url: post_url(state, b.item_id, &b.slug, 1),
                title_parts: highlight(&b.title, text),
                snippet_parts: vec![],
                more_url: String::new(),
                title: b.title.clone(),
                snippet: String::new(),
                author: String::new(),
                forum: node.map(|n| n.title.clone()).unwrap_or_default(),
                forum_url: node.map(|n| forum_path(n.id, &n.slug)).unwrap_or_default(),
                when: ago(b.created_at),
                number: 1,
                by_meaning: false,
                more_in_topic: 0,
                likes: 0,
                replies: 0,
            }
        })
        .collect()
}

/// Runs a search with the viewer's permissions; shared by the page and the JSON API. With
/// `facets`, the first page also counts forums, tags and years of everything found.
async fn run_search(state: &AppState, v: &Viewer, sq: &SearchQuery, limit: i64, facets: bool) -> Result<Run, AppError> {
    let q: String = sq.q.trim().chars().take(200).collect();
    if q.is_empty() {
        return Ok(Run::empty(q));
    }
    let q = crate::platform::plugins::filter_text(state, "search.query", "q", &q).await?;
    let parsed = bf_core::search::parse(&q);
    // Only words narrow a search enough to be cheap: `has:code` alone would read every post, which
    // a guest has no need to ask for.
    if v.user.is_none() && parsed.text.trim().is_empty() {
        return Ok(Run::empty(q));
    }
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    let mut nodes = tree.readable(&v.actor);
    if let Ok(f) = sq.forum.parse::<i64>() {
        nodes.retain(|n| *n == f);
    }
    if let Some(name) = &parsed.forum {
        // `#forum`, `forum:` and `category:` include everything below.
        let matched: Vec<i64> =
            tree.nodes.iter().filter(|n| n.slug == *name || n.title.to_lowercase() == *name).map(|n| n.id).collect();
        let matched = with_descendants(&tree.nodes, matched);
        nodes.retain(|n| matched.contains(n));
    }
    let uid = v.user.as_ref().map(|u| u.id);
    let today = chrono::Utc::now().date_naive();
    let tags: Vec<String> = parsed.tags.iter().map(|t| slugify(t)).collect();
    let not_tags: Vec<String> = parsed.not_tags.iter().map(|t| slugify(t)).collect();
    let filter = search::SearchFilter {
        author: parsed.author.as_deref().or(Some(sq.author.trim()).filter(|a| !a.is_empty())),
        after: parsed.after.as_deref().and_then(|s| resolve_date(s, today)).or_else(|| resolve_date(&sq.after, today)),
        before: parsed
            .before
            .as_deref()
            .and_then(|s| resolve_date(s, today))
            .or_else(|| resolve_date(&sq.before, today)),
        topics_only: sq.scope == "topics" || parsed.first_only,
        tags: &tags,
        not_tags: &not_tags,
        closed: parsed.closed,
        min_likes: parsed.min_likes,
        mine_of: if parsed.mine { Some(uid.map_or(-1, bf_core::ids::UserId::get)) } else { None },
        bookmarks_of: if parsed.bookmarks { Some(uid.map_or(-1, bf_core::ids::UserId::get)) } else { None },
        topic: sq.topic,
        has: &parsed.has,
        solved: parsed.solved,
        min_replies: parsed.min_replies,
        max_replies: parsed.max_replies,
        title_only: parsed.title_only,
    };
    let sort = if sq.sort.is_empty() {
        search::Sort::parse(parsed.order.unwrap_or(""))
    } else {
        search::Sort::parse(&sq.sort)
    };
    let grouped = sq.group != "0" && sq.topic.is_none();
    let page = sq.page.clamp(1, 500);
    let offset = (page - 1) * limit;
    let plain = parsed.text.trim() == q.trim()
        && sq.author.trim().is_empty()
        && sq.forum.is_empty()
        && sq.after.trim().is_empty()
        && sq.before.trim().is_empty();
    let mk = |limit: i64, offset: i64| search::SearchRequest {
        text: &parsed.text,
        nodes: &nodes,
        filter: &filter,
        sort,
        group: grouped,
        limit,
        offset,
        facets: facets && page == 1,
    };

    // With the AI's index on, its list and the keyword list are merged by reciprocal rank
    // fusion; with it off (or nothing found by meaning) this is the plain keyword page.
    let semantic = if plain && grouped && sort == search::Sort::Relevance && page <= 3 {
        by_meaning(state, v, &parsed.text).await
    } else {
        vec![]
    };
    let mut run = Run::empty(q.clone());
    run.sort = sort;
    run.grouped = grouped;
    let fused = !semantic.is_empty();
    let o = if fused {
        search::search_full(pool, &mk(FUSION_WINDOW, 0)).await.db()?
    } else {
        search::search_full(pool, &mk(limit, offset)).await.db()?
    };
    // What was highlighted: the corrected words when nothing matched the typed ones.
    let hl = if o.corrected { o.suggestion.clone().unwrap_or_default() } else { parsed.text.clone() };
    run.total = o.total;
    run.corrected = o.corrected;
    run.fuzzy = o.fuzzy;
    run.suggestion = o.suggestion.as_ref().map(|_| search::apply_corrections(&q, &o.corrections)).filter(|s| *s != q);
    run.facets = o.facets;
    run.best_bets = best_bet_results(state, &tree, &o.best_bets, &hl);
    let mut results: Vec<SearchResult>;
    if fused {
        let keyword: Vec<i64> = o.hits.iter().map(|h| h.item_id).collect();
        let order = search::rrf(&[&keyword, &semantic], 60.0);
        run.total = run.total.max(order.len() as i64);
        let wanted: Vec<i64> = order.iter().copied().skip(offset as usize).take(limit as usize).collect();
        let only_meaning: Vec<i64> = wanted.iter().copied().filter(|id| !keyword.contains(id)).collect();
        let cards = bf_db::ai::topic_cards(pool, &only_meaning).await.unwrap_or_default();
        // The AI's index is a few minutes old: a topic moved since to a forum this reader cannot
        // open is checked against its forum as it is now, and left out.
        let open = tree.readable(&v.actor);
        let cards: Vec<_> = cards.into_iter().filter(|c| open.contains(&c.3)).collect();
        let mut by_id: std::collections::HashMap<i64, search::SearchHit> =
            o.hits.into_iter().map(|h| (h.item_id, h)).collect();
        results = Vec::new();
        for id in wanted {
            if let Some(h) = by_id.remove(&id) {
                results.push(hit_result(state, &tree, h, &hl, false));
            } else if let Some(c) = cards.iter().find(|c| c.0 == id) {
                results.push(card_result(state, &tree, c));
            }
        }
    } else {
        results = o.hits.into_iter().map(|h| hit_result(state, &tree, h, &hl, false)).collect();
    }
    if plain && page <= 1 && sq.topic.is_none() {
        let extra = by_plugins(state, v, &parsed.text).await;
        // A search with no word matches would otherwise say "0 results" above a list of them.
        if run.total < limit {
            run.total += extra.len() as i64;
        }
        results.extend(extra);
    }
    // A best bet is shown on its own, not twice.
    if !run.best_bets.is_empty() {
        results.retain(|r| !run.best_bets.iter().any(|b| b.url == r.url));
    }
    run.results = results;
    Ok(run)
}

/// Entries that content-app plugins offered to search (`search.index`), for the plain words of
/// a search; the ones this viewer's groups may not see are left out in the query.
async fn by_plugins(state: &AppState, v: &Viewer, text: &str) -> Vec<SearchResult> {
    let plugins = crate::platform::plugins::searchable_plugins();
    if plugins.is_empty() || text.trim().is_empty() {
        return vec![];
    }
    let Some(pool) = state.pool.as_ref() else { return vec![] };
    let uid = v.user.as_ref().map(|u| u.id.get());
    let hits = bf_db::platform::plugin_content::search_docs(pool, text, &plugins, uid, 10).await.unwrap_or_default();
    hits.into_iter()
        .map(|h| SearchResult {
            url: h.url.clone(),
            title_parts: highlight(&h.title, text),
            snippet_parts: highlight(&h.snippet, text),
            title: h.title,
            snippet: h.snippet,
            author: h.author.unwrap_or_default(),
            forum_url: format!("/app/{}", h.plugin),
            forum: h.plugin,
            when: ago(h.updated_at),
            number: 1,
            by_meaning: false,
            more_in_topic: 0,
            more_url: String::new(),
            likes: 0,
            replies: 0,
        })
        .collect()
}

/// Topics about the same thing as the words searched for, best first, for merging with the
/// keyword hits. Quietly nothing when the feature is off or the model cannot be asked: search
/// must always work.
async fn by_meaning(state: &AppState, v: &Viewer, text: &str) -> Vec<i64> {
    if v.user.is_none()
        || text.chars().count() < 4
        || !state.settings().bool("ai_semantic_search_enabled")
        || !crate::ai::client::embeddings_configured(state)
    {
        return vec![];
    }
    let Ok(hits) = crate::ai::embeddings::by_meaning(state, v, text, 20, 0.35).await else { return vec![] };
    hits.into_iter().map(|h| h.0).collect()
}

/// A topic found only by meaning.
fn card_result(
    state: &AppState,
    tree: &Tree,
    c: &(i64, String, String, i64, Option<String>, chrono::DateTime<chrono::Utc>, String),
) -> SearchResult {
    let (id, slug, title, node_id, author, created, opening) = c;
    let node = tree.node(*node_id);
    SearchResult {
        url: post_url(state, *id, slug, 1),
        title_parts: vec![(title.clone(), false)],
        snippet_parts: vec![(opening.clone(), false)],
        more_url: String::new(),
        title: title.clone(),
        snippet: opening.clone(),
        author: author.clone().unwrap_or_else(|| "deleted".into()),
        forum: node.map(|n| n.title.clone()).unwrap_or_default(),
        forum_url: node.map(|n| forum_path(n.id, &n.slug)).unwrap_or_default(),
        when: ago(*created),
        number: 1,
        by_meaning: true,
        more_in_topic: 0,
        likes: 0,
        replies: 0,
    }
}

/// Letters folded to the plain ones a reader would type (the database does the same with
/// `bf_unaccent`), one character for one, so positions in the text stay valid.
fn fold(c: char) -> char {
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
}

/// Cuts `text` around matches of the search terms, for marking them in a template. A word of
/// three letters or more also matches as the start of a longer word (the search does the same),
/// and accents do not matter.
fn highlight(text: &str, query: &str) -> Vec<(String, bool)> {
    let terms: Vec<Vec<char>> = query
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).chars().map(fold).collect::<Vec<_>>())
        .filter(|w: &Vec<char>| w.len() >= 2 && w.iter().collect::<String>() != "or")
        .collect();
    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = chars.iter().map(|c| fold(*c)).collect();
    let mut out: Vec<(String, bool)> = Vec::new();
    let mut plain = String::new();
    let mut i = 0;
    while i < chars.len() {
        let hit = terms.iter().find(|t| {
            lower[i..].starts_with(t)
                && (i == 0 || !chars[i - 1].is_alphanumeric())
                && (t.len() >= 3 || chars.get(i + t.len()).is_none_or(|c| !c.is_alphanumeric()))
        });
        if let Some(t) = hit {
            // Take the rest of the word when the term is only its start.
            let mut end = i + t.len();
            if t.len() >= 3 {
                while chars.get(end).is_some_and(|c| c.is_alphanumeric()) {
                    end += 1;
                }
            }
            if !plain.is_empty() {
                out.push((std::mem::take(&mut plain), false));
            }
            out.push((chars[i..end].iter().collect(), true));
            i = end;
        } else {
            plain.push(chars[i]);
            i += 1;
        }
    }
    if !plain.is_empty() {
        out.push((plain, false));
    }
    out
}

/// Forums, tags and members whose name matches: (kind, url, name).
async fn name_matches(state: &AppState, v: &Viewer, q: &str) -> Result<Vec<(&'static str, String, String)>, AppError> {
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    let needle = q.to_lowercase();
    let mut out: Vec<(&'static str, String, String)> = tree
        .nodes
        .iter()
        .filter(|n| n.kind != "category" && n.title.to_lowercase().contains(&needle))
        .filter(|n| tree.perms(&v.actor, n.id).has(Perms::VIEW))
        .take(3)
        .map(|n| ("forum", forum_path(n.id, &n.slug), n.title.clone()))
        .collect();
    for (name, slug) in tags::tags_starting(pool, q, 3, &tree.readable(&v.actor)).await.db()? {
        out.push(("tag", format!("/tag/{slug}"), name));
    }
    for name in users::visible_usernames_starting(pool, q, 3, v.user.is_some(), is_staff(v)).await.db()? {
        out.push(("member", format!("/u/{name}"), name));
    }
    Ok(out)
}

/// Searches are the costliest read: one member (a guest: one address) gets a bucket of their own.
fn search_throttle(v: &Viewer, sq: &SearchQuery) -> Result<(), AppError> {
    if sq.q.trim().is_empty() {
        return Ok(());
    }
    let key = v
        .user
        .as_ref()
        .map_or_else(|| format!("ip:{}", crate::http::ratelimit::ip_key(v.ip)), |u| format!("u:{}", u.id));
    if v.user.is_none() {
        crate::http::ratelimit::SEARCH_BY_GUEST.enforce(&key)?;
    }
    crate::http::ratelimit::SEARCH_BY_USER.enforce(&key)
}

/// The parts of a search address, so the sort, facet and page links change one thing each.
#[derive(Clone)]
struct Params {
    q: String,
    author: String,
    forum: String,
    after: String,
    before: String,
    scope: String,
    sort: String,
    /// All posts instead of one per topic.
    ungrouped: bool,
    topic: Option<i64>,
}

impl Params {
    fn url(&self, page: i64) -> String {
        let mut u = url::form_urlencoded::Serializer::new(String::new());
        for (k, val) in [
            ("q", self.q.as_str()),
            ("author", self.author.as_str()),
            ("forum", self.forum.as_str()),
            ("after", self.after.as_str()),
            ("before", self.before.as_str()),
            ("in", self.scope.as_str()),
            ("sort", self.sort.as_str()),
        ] {
            if !val.is_empty() {
                u.append_pair(k, val);
            }
        }
        if self.ungrouped {
            u.append_pair("group", "0");
        }
        if let Some(t) = self.topic {
            u.append_pair("topic", &t.to_string());
        }
        if page > 1 {
            u.append_pair("page", &page.to_string());
        }
        format!("/search?{}", u.finish())
    }
}

fn facet_links(
    items: &[search::FacetCount],
    label: impl Fn(&search::FacetCount) -> Option<String>,
    active: impl Fn(&search::FacetCount) -> bool,
    url: impl Fn(&search::FacetCount, bool) -> String,
) -> Vec<FacetLink> {
    items
        .iter()
        .filter_map(|f| {
            let on = active(f);
            Some(FacetLink { label: label(f)?, count: f.count, url: url(f, on), active: on })
        })
        .collect()
}

async fn search(State(state): State<AppState>, v: Viewer, Query(sq): Query<SearchQuery>) -> Result<Response, AppError> {
    search_throttle(&v, &sq)?;
    let run = run_search(&state, &v, &sq, SEARCH_PAGE, true).await?;
    let Run { q, results, total, best_bets, suggestion, corrected, fuzzy, facets, sort, grouped } = run;
    let pool = state.db()?;
    if !q.is_empty() && sq.page <= 1 {
        let uid = v.user.as_ref().map(|u| u.id);
        if let Err(e) = search::log_search(pool, &q, total as i32, uid.map(bf_core::ids::UserId::get)).await {
            tracing::warn!(error = %e, "search log failed");
        }
    }
    let tree = state.forum.get(pool).await?;
    let forums = tree
        .nodes
        .iter()
        .filter(|n| n.kind == "forum" && tree.perms(&v.actor, n.id).has(Perms::READ))
        .map(|n| (n.id, n.title.clone(), sq.forum == n.id.to_string()))
        .collect();
    let filtered = sq.scope == "topics"
        || !sq.author.trim().is_empty()
        || !sq.forum.is_empty()
        || !sq.after.trim().is_empty()
        || !sq.before.trim().is_empty();
    // Only an unfiltered search is about names as well.
    let this_page = sq.page.clamp(1, 500);
    let base = Params {
        q: q.clone(),
        author: sq.author.trim().to_owned(),
        forum: sq.forum.clone(),
        after: sq.after.trim().to_owned(),
        before: sq.before.trim().to_owned(),
        scope: sq.scope.clone(),
        // A sort chosen in the query itself (`order:`) stays there.
        sort: if sq.sort.is_empty() { String::new() } else { sort.as_str().to_owned() },
        ungrouped: sq.group == "0",
        topic: sq.topic,
    };
    let page_url = |n: i64| base.url(n);
    let last = ((total + SEARCH_PAGE - 1) / SEARCH_PAGE).clamp(1, 500);
    let pages = bf_web::pages::page_window(last, this_page, page_url);
    let prev_url = (this_page > 1).then(|| page_url(this_page - 1));
    let next_url = (this_page < last).then(|| page_url(this_page + 1));
    let matches = if q.is_empty() || filtered || sq.topic.is_some() || this_page > 1 {
        vec![]
    } else {
        name_matches(&state, &v, &q).await?
    };
    let t = v.t();
    let sorts: Vec<(&'static str, String, String, bool)> = [
        (search::Sort::Relevance, t.msg("Relevance")),
        (search::Sort::Newest, t.msg("Newest")),
        (search::Sort::Oldest, t.msg("Oldest")),
        (search::Sort::Likes, t.msg("Most liked")),
        (search::Sort::Replies, t.msg("Most replies")),
    ]
    .into_iter()
    .map(|(s, label)| {
        let mut p = base.clone();
        p.sort = if s == search::Sort::Relevance { String::new() } else { s.as_str().to_owned() };
        (s.as_str(), label, p.url(1), s == sort)
    })
    .collect();
    let group_url = {
        let mut p = base.clone();
        p.ungrouped = grouped;
        p.url(1)
    };
    let parsed = bf_core::search::parse(&q);
    let facets_view = SearchFacets {
        forums: facet_links(
            &facets.forums,
            |f| f.key.parse::<i64>().ok().and_then(|id| tree.node(id)).map(|n| n.title.clone()),
            |f| sq.forum == f.key,
            |f, on| {
                let mut p = base.clone();
                p.forum = if on { String::new() } else { f.key.clone() };
                p.url(1)
            },
        ),
        tags: facet_links(
            &facets.tags,
            |f| Some(f.label.clone()),
            |f| parsed.tags.iter().any(|t| slugify(t) == f.key),
            |f, on| {
                let mut p = base.clone();
                let op = format!("tag:{}", f.key);
                p.q = if on {
                    q.split_whitespace().filter(|w| *w != op).collect::<Vec<_>>().join(" ")
                } else {
                    format!("{q} {op}")
                };
                p.url(1)
            },
        ),
        years: facet_links(
            &facets.years,
            |f| Some(f.label.clone()),
            |f| {
                f.key.parse::<i32>().is_ok_and(|y| {
                    sq.after.trim() == format!("{y}-01-01") && sq.before.trim() == format!("{}-01-01", y + 1)
                })
            },
            |f, on| {
                let mut p = base.clone();
                if let (false, Ok(y)) = (on, f.key.parse::<i32>()) {
                    p.after = format!("{y}-01-01");
                    p.before = format!("{}-01-01", y + 1);
                } else {
                    p.after.clear();
                    p.before.clear();
                }
                p.url(1)
            },
        ),
        capped: facets.capped,
    };
    let suggestion = suggestion.map(|s| {
        let mut p = base.clone();
        p.q.clone_from(&s);
        (s, p.url(1))
    });
    let original_url = {
        // Exactly what was typed, without the automatic fix: a fix is only applied when it is
        // the page's own result, so the link is the plain address.
        let mut p = base.clone();
        p.q = format!("\"{}\"", q.replace('"', ""));
        p.url(1)
    };
    let (mut l, set) = layout(&state, &v, Some(&v.t().msg("Search")));
    l.noindex = true;
    let body = SearchPage {
        layout: l,
        total,
        pages,
        prev_url,
        next_url,
        matches,
        filtered,
        before: sq.before.trim().to_owned(),
        q,
        author: sq.author.trim().to_owned(),
        after: sq.after.trim().to_owned(),
        topics_only: sq.scope == "topics",
        topic: sq.topic,
        forums,
        results,
        sorts,
        sort: sort.as_str().to_owned(),
        grouped,
        group_url,
        suggestion,
        corrected,
        original_url,
        fuzzy,
        facets: facets_view,
        best_bets,
        widgets: crate::admin::themes::page_widgets(&state, "search").await,
        can_edit_layout: v.actor.is_admin,
        editing_layout: v.actor.is_admin && sq.edit_layout == "1",
    };
    Ok(page(render(&body), set, false))
}

/// `GET /api/v1/search?q=` for the live search dropdown: matching forums,
/// tags and members first, then posts.
#[utoipa::path(get, path = "/api/v1/search", tag = "search",
    params(("q" = String, Query, description = "search text"), ("author" = Option<String>, Query), ("forum" = Option<String>, Query), ("in" = Option<String>, Query, description = "topics to search titles only"), ("topic" = Option<i64>, Query)),
    responses((status = 200, body = [Object], description = "kind, url, title, snippet; topics also id and forum")))]
pub(crate) async fn search_api(
    State(state): State<AppState>,
    v: Viewer,
    Query(sq): Query<SearchQuery>,
) -> Result<Response, AppError> {
    search_throttle(&v, &sq)?;
    let Run { q, results, .. } = run_search(&state, &v, &sq, 8, false).await?;
    let mut out = Vec::new();
    if sq.scope != "topics" && !q.is_empty() {
        for (kind, url, name) in name_matches(&state, &v, &q).await? {
            let label = match kind {
                "forum" => "Forum",
                "tag" => "Tag",
                _ => "Member",
            };
            out.push(json!({ "url": url, "title": name, "snippet": label, "kind": kind }));
        }
    }
    out.extend(
        results.into_iter().map(
            |r| json!({ "url": r.url, "title": r.title, "snippet": r.snippet, "kind": "topic", "forum": r.forum }),
        ),
    );
    // Topic results carry their id and the metadata plugins made public, in one query.
    for r in &mut out {
        if r["kind"] == "topic"
            && let Some(id) = r["url"]
                .as_str()
                .and_then(|u| u.split(['#', '?']).next()?.strip_prefix("/t/")?.split('/').nth(1)?.parse::<i64>().ok())
        {
            r["id"] = json!(id);
        }
    }
    crate::platform::plugins::attach_meta(&state, "topic", &mut out, "id").await;
    let mut res = Json(out).into_response();
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=10"));
    Ok(res)
}
