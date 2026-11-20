//! The home page and forum pages.

use crate::community::profiles::follow_key;
use crate::forum::posting::create_targets;
use crate::forum::topics::path_id;
use crate::forum::tree::Tree;
use crate::forum::tree::guest;
use crate::http::render::{ago, DbResult};
use crate::http::render::layout;
use crate::http::render::page;
use crate::http::render::unread_count;
use crate::http::viewer::Viewer;
use crate::http::viewer::cookie;
use crate::state::AppState;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use bf_core::AppError;
use bf_core::perms::{Actor, Perms};
use bf_core::slug::{forum_path, profile_path, topic_path};
use bf_db::forum::nodes::Node;
use bf_db::forum::topics::TopicRow;
use bf_db::forum::{nodes, topics};
use bf_web::pages::{CategoryBlock, ForumCard, ForumPage, HomePage, TopicComposer, TopicLine, render};
use serde::Deserialize;

pub fn cached_pages() -> Router<AppState> {
    Router::new().route("/", get(home)).route("/c/{seg}", get(forum_by_slug)).route("/c/{seg}/{id}", get(forum))
}

pub fn router() -> Router<AppState> {
    Router::new().route("/c/{id}/go", get(link_node))
}

pub(crate) fn topic_line(t: &TopicRow) -> TopicLine {
    TopicLine {
        forum: None,
        url: topic_path(t.id, &t.slug),
        solved: t.solved_post_number.is_some(),
        id: t.id,
        title: t.title.clone(),
        prefix: t.prefix.clone(),
        author: t.author.clone().unwrap_or_else(|| "deleted".into()),
        pinned: t.pinned,
        locked: t.locked,
        replies: t.reply_count,
        views: t.view_count,
        last_poster: t.last_poster.clone().unwrap_or_else(|| "deleted".into()),
        last_posted: ago(t.last_posted_at),
        started: ago(t.created_at),
        unread: t.unread().map(|n| (n, format!("{}/unread", topic_path(t.id, &t.slug)))),
        posters: t.posters.clone(),
        tags: t.tag_pairs.iter().filter_map(|p| p.split_once('|')).map(|(n, s)| (n.to_owned(), s.to_owned())).collect(),
        votes: (t.vote_count > 0).then_some(t.vote_count),
        vote_button: None,
    }
}

fn forum_card(tree: &Tree, actor: &Actor, n: &Node, t: &bf_core::i18n::T) -> ForumCard {
    let (title, description_html) = crate::forum::translations::node_text(t, n);
    ForumCard {
        id: n.id,
        icon: n.icon.clone(),
        icon_image: n.icon_image.clone(),
        url: if n.redirect_url.is_some() { format!("/c/{}/go", n.id) } else { forum_path(n.id, &n.slug) },
        clicks: n.redirect_url.is_some().then_some(n.clicks),
        title,
        description_html,
        topic_count: n.topic_count,
        post_count: n.post_count,
        children: tree
            .nodes
            .iter()
            .filter(|c| c.parent_id == Some(n.id) && tree.perms(actor, c.id).has(Perms::VIEW))
            .map(|c| (forum_path(c.id, &c.slug), c.title.clone()))
            .collect(),
        last: None,
        unread: false,
    }
}

/// `node|item|slug|poster|epoch|title`; the title comes last because it may contain `|`.
fn parse_last_topics(rows: &[String]) -> std::collections::HashMap<i64, (String, String, String, bf_web::ago::Ago)> {
    rows.iter()
        .filter_map(|row| {
            let mut f = row.splitn(6, '|');
            let (node, item, slug) = (f.next()?.parse().ok()?, f.next()?.parse().ok()?, f.next()?);
            let (poster, at, title) = (f.next()?, f.next()?.parse().ok()?, f.next()?);
            let at = chrono::DateTime::from_timestamp(at, 0)?;
            let poster = if poster.is_empty() { "deleted" } else { poster };
            Some((node, (topic_path(item, slug), title.to_owned(), poster.to_owned(), ago(at))))
        })
        .collect()
}

async fn home(State(state): State<AppState>, v: Viewer, headers: HeaderMap) -> Result<Response, AppError> {
    let (mut l, set) = layout(&state, &v, None);
    l.browsing = crate::community::browsing::visit(&state, &v, &headers, "home");
    let Some(pool) = &state.pool else {
        let empty = HomePage {
            layout: l,
            categories: vec![],
            latest: vec![],
            member: false,
            stats: None,
            widgets_top: vec![],
            widgets_main: vec![(0, String::new(), String::new(), "forum_list".to_owned())],
            widgets_bottom: vec![],
            can_edit_layout: false,
        };
        return Ok(page(render(&empty), set, true));
    };
    let tree = state.forum.get(pool).await?;
    let uid = v.user.as_ref().map(|u| u.id);
    let latest = topics::latest_topics(pool, &tree.readable(&v.actor), uid.map(bf_core::ids::UserId::get), false, 20)
        .await
        .db()?;
    let unread_forums: std::collections::HashSet<i64> =
        latest.iter().filter(|t| t.unread().is_some()).map(|t| t.node_id).collect();
    let visible = |n: &&Node| tree.perms(&v.actor, n.id).has(Perms::VIEW);
    let st = bf_db::admin::maintenance::index_stats(pool).await.db()?;
    let last_topics = parse_last_topics(&st.last_topics);
    let card = |n: &Node| ForumCard {
        // VIEW shows the forum exists; its topics are for those who may READ.
        last: last_topics.get(&n.id).cloned().filter(|_| tree.perms(&v.actor, n.id).has(Perms::READ)),
        unread: unread_forums.contains(&n.id),
        ..forum_card(&tree, &v.actor, n, &v.t())
    };
    // Top-level nodes in order: a category is a headed box of its forums; a forum that sits at the
    // top level (moved out of its category) is a forum box of its own, and neighbours share one.
    let mut categories: Vec<CategoryBlock> = Vec::new();
    for n in tree.nodes.iter().filter(|n| n.parent_id.is_none()).filter(visible) {
        if n.kind == "category" {
            categories.push(CategoryBlock {
                headless: false,
                id: n.id,
                icon: n.icon.clone(),
                icon_image: n.icon_image.clone(),
                title: v.t().msg(&n.title),
                forums: tree.nodes.iter().filter(|c| c.parent_id == Some(n.id)).filter(visible).map(card).collect(),
            });
        } else if let Some(open) = categories.last_mut().filter(|b| b.headless) {
            open.forums.push(card(n));
        } else {
            categories.push(CategoryBlock {
                headless: true,
                id: n.id,
                icon: None,
                icon_image: None,
                title: String::new(),
                forums: vec![card(n)],
            });
        }
    }
    l.canonical = Some(format!("{}/", state.config.base_url));
    l.json_ld.push(bf_seo::json_ld_script(&bf_seo::website(&l.site_name, &state.config.base_url)));
    l.json_ld.push(bf_seo::json_ld_script(&bf_seo::organization(
        &l.site_name,
        &state.config.base_url,
        Some(l.brand.logo.as_str()).filter(|s| !s.is_empty()),
    )));
    l.unread_notifications = unread_count(&state, &v).await;
    // Each topic names its forum, since the list mixes them.
    let latest = latest
        .iter()
        .map(|t| TopicLine {
            forum: tree.node(t.node_id).map(|n| (n.title.clone(), forum_path(n.id, &n.slug))),
            ..topic_line(t)
        })
        .collect();
    let newest = st.newest_id.zip(st.newest).map(|(id, n)| {
        let url = profile_path(id, &n);
        (n, url)
    });
    let stats = Some((st.topics, st.posts, st.members, newest));
    let (widgets_top, widgets_main, widgets_bottom) = crate::admin::themes::home_widgets(&state).await;
    let html = render(&HomePage {
        layout: l,
        categories,
        latest,
        member: v.user.is_some(),
        stats,
        widgets_top,
        widgets_main,
        widgets_bottom,
        can_edit_layout: v.actor.is_admin,
    });
    Ok(page(html, set, v.user.is_none()))
}

#[derive(Deserialize)]
struct ForumQuery {
    page: Option<i64>,
    /// `1` opens the composer's calendar part (from the calendar's Add event button).
    #[serde(default)]
    event: String,
    #[serde(default)]
    date: String,
    #[serde(default)]
    prefix: String,
    #[serde(default)]
    sort: String,
    #[serde(default)]
    show: String,
    #[serde(default)]
    tag: String,
    #[serde(default)]
    after: String,
    #[serde(default)]
    edit_layout: String,
}

/// The active prefix, tag and date filters as a query string.
fn forum_filters(q: &ForumQuery) -> String {
    let mut out = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in [("prefix", &q.prefix), ("tag", &q.tag), ("after", &q.after)] {
        if !v.is_empty() {
            out.append_pair(k, v);
        }
    }
    out.finish()
}

fn keep_filters(url: String, filters: &str) -> String {
    match (filters.is_empty(), url.contains('?')) {
        (true, _) => url,
        (false, true) => format!("{url}&{filters}"),
        (false, false) => format!("{url}?{filters}"),
    }
}

/// Builds a forum URL keeping only non-default query parameters.
fn forum_url(base: &str, sort: topics::Sort, show: &str, page: i64) -> String {
    let mut q = Vec::new();
    if sort != topics::Sort::default() {
        q.push(format!("sort={}", sort.key()));
    }
    if show != "all" {
        q.push(format!("show={show}"));
    }
    if page > 1 {
        q.push(format!("page={page}"));
    }
    if q.is_empty() { base.to_owned() } else { format!("{base}?{}", q.join("&")) }
}

async fn forum(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path((slug, id)): Path<(String, String)>,
    Query(q): Query<ForumQuery>,
) -> Result<Response, AppError> {
    let pool = state.db()?;
    let id = path_id(&id)?;
    // The sort is remembered per forum in a cookie; an explicit choice updates it.
    let cookie_name = format!("bf_sort_{id}");
    let remembered = cookie(&headers, &cookie_name).and_then(topics::Sort::parse);
    let explicit = topics::Sort::parse(&q.sort);
    let chosen = explicit.or(remembered);
    let show = show_filter(&q.show, v.user.is_some());
    let tree = state.forum.get(pool).await?;
    let prefixes = tree.node(id).map(|n| n.prefixes.clone()).unwrap_or_default();
    let prefix = prefixes.iter().find(|p| **p == q.prefix).cloned();
    let filter = topics::TopicFilter {
        prefix: prefix.clone(),
        unanswered: show == "unanswered",
        unsolved: show == "unsolved",
        author_id: ((show == "mine").then(|| v.user.as_ref().map(|u| u.id)).flatten()).map(bf_core::ids::UserId::get),
        tag: Some(q.tag.as_str()).filter(|t| !t.is_empty()).map(str::to_owned),
        after: chrono::NaiveDate::parse_from_str(q.after.trim(), "%Y-%m-%d").ok(),
        unread: show == "unread",
        include_unlisted: tree.perms(&v.actor, id).has(Perms::MODERATE),
    };
    let node = tree.node(id).filter(|n| n.kind != "link").ok_or(AppError::NotFound)?;
    let perms = tree.perms(&v.actor, id);
    if !perms.has(Perms::VIEW) {
        return Err(AppError::NotFound);
    }
    let default_sort = node.default_sort.as_deref().and_then(topics::Sort::parse).unwrap_or(if node.mode == "ideas" {
        topics::Sort::Votes
    } else {
        topics::Sort::default()
    });
    let sort = chosen.unwrap_or(default_sort);
    if slug != node.slug {
        return Ok(Redirect::permanent(&forum_path(id, &node.slug)).into_response());
    }
    let per_page = state.settings().int("topics_per_page");
    let page_n = q.page.unwrap_or(1).clamp(1, 100_000);
    let topics = if perms.has(Perms::READ) {
        topics::forum_topics(
            pool,
            id,
            sort,
            filter,
            (v.user.as_ref().map(|u| u.id)).map(bf_core::ids::UserId::get),
            per_page + 1,
            (page_n - 1) * per_page,
        )
        .await
        .db()?
    } else {
        vec![]
    };
    let has_next = topics.len() as i64 > per_page;
    let base = forum_path(id, &node.slug);
    let filters = forum_filters(&q);
    let (mut l, set) = layout(&state, &v, Some(node.seo_title.as_deref().unwrap_or(&node.title)));
    l.browsing = crate::community::browsing::visit(&state, &v, &headers, &format!("forum:{id}"));
    l.feed = tree.readable(&guest()).contains(&id).then(|| format!("/c/{id}/feed.rss"));
    if let Some(d) = &node.seo_description {
        l.description = d.clone();
    }
    crate::admin::themes::apply(&mut l, &v, &v.theme_headers, node.theme_id);
    l.canonical =
        Some(format!("{}{}", state.config.base_url, forum_url(&base, topics::Sort::default(), "all", page_n)));
    // Sorted and filtered views duplicate the default listing; keep them out of the index.
    l.noindex = sort != default_sort || show != "all" || !q.tag.is_empty() || !q.after.is_empty();
    l.json_ld.push(bf_seo::json_ld_script(&bf_seo::breadcrumbs(&[
        ("Forums", &format!("{}/", state.config.base_url)),
        (&node.title, &format!("{}{base}", state.config.base_url)),
    ])));
    if !l.noindex {
        let items: Vec<(String, String)> = topics
            .iter()
            .take(per_page as usize)
            .map(|t| (format!("{}{}", state.config.base_url, topic_path(t.id, &t.slug)), t.title.clone()))
            .collect();
        let offset = usize::try_from((page_n - 1) * per_page).unwrap_or(0);
        l.json_ld.push(bf_seo::json_ld_script(&bf_seo::item_list(
            &node.title,
            &format!("{}{base}", state.config.base_url),
            &items,
            offset,
        )));
    }
    l.unread_notifications = unread_count(&state, &v).await;
    let moved = if node.has_redirects {
        bf_db::forum::topics::redirects(pool, id, &tree.readable(&v.actor))
            .await
            .db()?
            .into_iter()
            .map(|(tid, title, slug)| (topic_path(tid, &slug), title))
            .collect()
    } else {
        vec![]
    };
    // Only forums with subforums pay for this query.
    let sub_last = if tree.nodes.iter().any(|c| c.parent_id == Some(id)) {
        parse_last_topics(&bf_db::admin::maintenance::index_stats(pool).await.db()?.last_topics)
    } else {
        std::collections::HashMap::default()
    };
    // A forum can ask for a trust level; below it the button stays, disabled, and says why.
    let may_create = tree.perms(&v.actor, id).has(Perms::CREATE);
    let level_short = may_create
        && !tree.perms(&v.actor, id).has(Perms::MODERATE)
        && v.user.as_ref().is_some_and(|u| !u.is_admin && i32::from(node.min_trust_level) > i32::from(u.trust_level));
    let composer_blocked = level_short.then(|| {
        let t = v.t();
        let (have, need) = (v.user.as_ref().map_or(0, |u| u.trust_level), i16::from(node.min_trust_level));
        t.msgf(
            "Starting topics here needs trust level {need} ({name}). You are at level {have}.",
            &[
                ("need", &need.to_string()),
                ("name", &crate::community::trust::level_label(&state, &t, need)),
                ("have", &have.to_string()),
            ],
        )
    });
    let composer = (may_create && !level_short).then(|| TopicComposer {
        forums: create_targets(&tree, &v.actor),
        prefixes: prefixes.clone(),
        body: node.template_md.clone().unwrap_or_default(),
        event_open: q.event == "1",
        event_date: chrono::NaiveDate::parse_from_str(&q.date, "%Y-%m-%d").map(|d| d.to_string()).unwrap_or_default(),
    });
    // Ideas: members vote from the list, and see how many votes they have left.
    let ideas_voter = node.mode == "ideas" && v.user.is_some() && !v.actor.silenced;
    let votes_left = match &v.user {
        Some(u) if ideas_voter => crate::forum::votes::votes_left(&state, u).await?,
        _ => None,
    };
    let prev_url = (page_n > 1).then(|| keep_filters(forum_url(&base, sort, show, page_n - 1), &filters));
    let next_url = has_next.then(|| keep_filters(forum_url(&base, sort, show, page_n + 1), &filters));
    l.set_paging(&state.config.base_url, prev_url.as_deref(), next_url.as_deref());
    let p = ForumPage {
        widgets: crate::admin::themes::page_widgets(&state, "forum").await,
        can_edit_layout: v.actor.is_admin,
        editing_layout: v.actor.is_admin && q.edit_layout == "1",
        composer,
        composer_blocked,
        layout: l,
        moved,
        list_style: node.list_style.clone(),
        forum_id: id,
        stored_title: node.title.clone(),
        title: crate::forum::translations::node_text(&v.t(), node).0,
        description_html: crate::forum::translations::node_text(&v.t(), node).1,
        subforums: tree
            .nodes
            .iter()
            .filter(|c| c.parent_id == Some(id) && tree.perms(&v.actor, c.id).has(Perms::VIEW))
            .map(|c| ForumCard {
                // VIEW shows the subforum exists; its topics are for those who may READ.
                last: sub_last.get(&c.id).cloned().filter(|_| tree.perms(&v.actor, c.id).has(Perms::READ)),
                ..forum_card(&tree, &v.actor, c, &v.t())
            })
            .collect(),
        topics: topics
            .iter()
            .take(per_page as usize)
            .map(|t| TopicLine {
                vote_button: (ideas_voter && !t.locked).then_some((t.vote_count, t.voted)),
                ..topic_line(t)
            })
            .collect(),
        votes_left,
        page: page_n,
        prev_url,
        next_url,
        can_create: perms.has(Perms::CREATE) && node.kind == "forum",
        rules_html: node.rules_html.clone().filter(|r| !r.is_empty()),
        member: v.user.is_some(),
        moderator: perms.has(Perms::MODERATE),
        move_targets: tree
            .nodes
            .iter()
            .filter(|n| n.kind == "forum" && n.id != id && tree.perms(&v.actor, n.id).has(Perms::MODERATE))
            .map(|n| (n.id, n.title.clone()))
            .collect(),
        follow: follow_key(&state, &v, "node", id).await,
        prefixes: prefixes
            .iter()
            .map(|p| {
                let active = prefix.as_deref() == Some(p.as_str());
                let enc = bf_core::html::url_encode(p);
                let url = if active { base.clone() } else { format!("{base}?prefix={enc}") };
                (p.clone(), url, active)
            })
            .collect(),
        sorts: sort_links(&v, node.mode == "ideas", &base, show, &filters, sort),
        filters: show_links(&v, &base, show, &filters, sort),
    };
    let mut res = page(render(&p), set, v.user.is_none() && explicit.is_none() && remembered.is_none());
    if let Some(s) = explicit {
        let c = format!("{cookie_name}={}; Path=/c/; SameSite=Lax; Max-Age=31536000", s.key());
        res.headers_mut().append(header::SET_COOKIE, HeaderValue::from_str(&c).map_err(AppError::internal)?);
    }
    Ok(res)
}

/// The sort links under the forum title, the current one marked. Votes only sort an ideas forum.
fn sort_links(
    v: &Viewer,
    ideas: bool,
    base: &str,
    show: &str,
    filters: &str,
    sort: topics::Sort,
) -> Vec<(String, String, bool)> {
    [
        ("forum.sort_latest", topics::Sort::Activity),
        ("forum.sort_newest", topics::Sort::Newest),
        ("forum.sort_replies", topics::Sort::Replies),
        ("forum.sort_views", topics::Sort::Views),
        ("forum.sort_likes", topics::Sort::Likes),
        ("forum.sort_title", topics::Sort::Title),
        ("forum.sort_votes", topics::Sort::Votes),
    ]
    .into_iter()
    .filter(|(_, s)| *s != topics::Sort::Votes || ideas)
    .map(|(key, s)| (v.t().t(key), keep_filters(forum_url(base, s, show, 1), filters), s == sort))
    .collect()
}

/// The "show" links (all, unanswered, ...); mine and unread need an account.
fn show_links(v: &Viewer, base: &str, show: &str, filters: &str, sort: topics::Sort) -> Vec<(String, String, bool)> {
    [("forum.show_all", "all"), ("forum.show_unanswered", "unanswered"), ("forum.show_unsolved", "unsolved")]
        .into_iter()
        .chain(v.user.is_some().then_some(("forum.show_mine", "mine")))
        .chain(v.user.is_some().then_some(("home.unread", "unread")))
        .map(|(key, f)| (v.t().t(key), keep_filters(forum_url(base, sort, f, 1), filters), f == show))
        .collect()
}

/// Which list the "show" parameter asks for. "mine" and "unread" need an account; guests silently
/// get the full list.
fn show_filter(raw: &str, signed_in: bool) -> &'static str {
    match raw {
        "unanswered" => "unanswered",
        "unsolved" => "unsolved",
        "mine" if signed_in => "mine",
        "unread" if signed_in => "unread",
        _ => "all",
    }
}

/// `/c/{slug}`: the readable category of that name.
async fn forum_by_slug(
    State(state): State<AppState>,
    v: Viewer,
    Path(seg): Path<String>,
) -> Result<Response, AppError> {
    let tree = state.forum.get(state.db()?).await?;
    let readable = tree.readable(&v.actor);
    let node = tree.nodes.iter().find(|n| n.slug == seg && readable.contains(&n.id)).ok_or(AppError::NotFound)?;
    Ok(Redirect::permanent(&forum_path(node.id, &node.slug)).into_response())
}

/// A link forum: counts the click, then redirects to the address an admin stored.
async fn link_node(
    State(state): State<AppState>,
    v: Viewer,
    headers: axum::http::HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    if crate::http::viewer::is_subresource(&headers) {
        return Err(AppError::NotFound);
    }
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    if !tree.perms(&v.actor, id).has(Perms::VIEW) {
        return Err(AppError::NotFound);
    }
    let url = nodes::follow_link_node(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    Ok(Redirect::to(&url).into_response())
}
