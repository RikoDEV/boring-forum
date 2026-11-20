//! Sitemaps, robots.txt and RSS, Atom and JSON feeds.

use crate::forum::tree::guest;
use crate::http::render::DbResult;
use crate::http::render::xml;
use crate::state::AppState;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use bf_core::AppError;
use bf_core::slug::{forum_path, topic_path};
use bf_db::forum::feeds;

pub fn cached_pages() -> Router<AppState> {
    Router::new()
        .route("/sitemap.xml", get(sitemap))
        .route("/sitemap/{part}", get(sitemap_part))
        .route("/robots.txt", get(robots))
        // Each feed in RSS 2.0, Atom 1.0 and JSON Feed 1.1; the extension picks the format.
        .route("/latest.rss", get(latest_rss))
        .route("/latest.atom", get(latest_rss))
        .route("/latest.json", get(latest_rss))
        .route("/c/{id}/feed.rss", get(forum_rss))
        .route("/c/{id}/feed.atom", get(forum_rss))
        .route("/c/{id}/feed.json", get(forum_rss))
        .route("/t/{id}/feed.rss", get(topic_rss))
        .route("/t/{id}/feed.atom", get(topic_rss))
        .route("/t/{id}/feed.json", get(topic_rss))
        .route("/tag/{slug}/feed.rss", get(tag_rss))
        .route("/tag/{slug}/feed.atom", get(tag_rss))
        .route("/tag/{slug}/feed.json", get(tag_rss))
}

/// `/sitemap.xml`: the whole sitemap when it fits one file, otherwise an index of
/// `/sitemap/{n}.xml` files, each with at most `SITEMAP_CHUNK` addresses.
async fn sitemap(State(state): State<AppState>) -> Result<Response, AppError> {
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    let readable = tree.readable(&guest());
    let topics = usize::try_from(feeds::sitemap_topic_count(pool, &readable).await.db()?).unwrap_or(0);
    let fixed = fixed_sitemap_urls(&state, &tree, &readable).await;
    let files = bf_seo::sitemap_files(fixed.len() + topics);
    if files == 1 {
        return sitemap_file(&state, 1).await;
    }
    let base = &state.config.base_url;
    let locs: Vec<String> = (1..=files).map(|n| format!("{base}/sitemap/{n}.xml")).collect();
    Ok(xml(bf_seo::sitemap_index(&locs)))
}

/// `/sitemap/{n}.xml`, one file of a sitemap that needed several.
async fn sitemap_part(State(state): State<AppState>, Path(part): Path<String>) -> Result<Response, AppError> {
    let n = part.strip_suffix(".xml").and_then(|n| n.parse::<usize>().ok()).ok_or(AppError::NotFound)?;
    sitemap_file(&state, n).await
}

/// The addresses that always come first: home, forums and plugin paths.
async fn fixed_sitemap_urls(
    state: &AppState,
    tree: &crate::forum::tree::Tree,
    readable: &[i64],
) -> Vec<bf_seo::SitemapUrl> {
    let base = &state.config.base_url;
    let mut urls = vec![bf_seo::SitemapUrl { loc: format!("{base}/"), lastmod: None }];
    urls.extend(tree.nodes.iter().filter(|n| n.kind == "forum" && readable.contains(&n.id)).map(|n| {
        bf_seo::SitemapUrl { loc: format!("{base}{}", forum_path(n.id, &n.slug)), lastmod: n.last_activity_at }
    }));
    // Pages that list things and are indexed on their own: members, clubs and the calendar.
    let clubs = state.settings().bool("clubs_enabled");
    for path in ["/u", "/clubs", "/calendar"] {
        if path == "/clubs" && !clubs {
            continue;
        }
        urls.push(bf_seo::SitemapUrl { loc: format!("{base}{path}"), lastmod: None });
    }
    // Tags and profiles, only what guests may see and search engines may list.
    if let Some(pool) = &state.pool {
        urls.extend(feeds::sitemap_tags(pool, readable).await.unwrap_or_default().into_iter().map(|(slug, at)| {
            bf_seo::SitemapUrl { loc: format!("{base}/tag/{}", bf_core::html::url_encode(&slug)), lastmod: Some(at) }
        }));
        urls.extend(feeds::sitemap_profiles(pool, 5000).await.unwrap_or_default().into_iter().map(|name| {
            bf_seo::SitemapUrl { loc: format!("{base}/u/{}", bf_core::html::url_encode(&name.to_ascii_lowercase())), lastmod: None }
        }));
    }
    urls.extend(
        crate::platform::plugins::sitemap_paths(state)
            .await
            .into_iter()
            .map(|(p, at)| bf_seo::SitemapUrl { loc: format!("{base}{p}"), lastmod: at }),
    );
    urls
}

/// File number `part` (from 1): the fixed addresses first, then topics, newest first.
async fn sitemap_file(state: &AppState, part: usize) -> Result<Response, AppError> {
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    let readable = tree.readable(&guest());
    let topics = usize::try_from(feeds::sitemap_topic_count(pool, &readable).await.db()?).unwrap_or(0);
    let mut fixed = fixed_sitemap_urls(state, &tree, &readable).await;
    let (own, offset, count) = bf_seo::sitemap_slice(part, fixed.len(), topics).ok_or(AppError::NotFound)?;
    let mut urls: Vec<bf_seo::SitemapUrl> = fixed.drain(own).collect();
    let base = &state.config.base_url;
    let rows = feeds::sitemap_topics(pool, &readable, offset as i64, count as i64).await.db()?;
    urls.extend(
        rows.into_iter().map(|(id, slug, t)| bf_seo::SitemapUrl {
            loc: format!("{base}{}", topic_path(id, &slug)),
            lastmod: Some(t),
        }),
    );
    Ok(xml(bf_seo::sitemap(&urls)))
}

async fn robots(State(state): State<AppState>) -> Response {
    // AI training crawlers are turned away when the site says so.
    let block_ai = state.settings().str("ai_crawlers") != "allow";
    let body = bf_seo::robots_txt_with(&state.config.base_url, block_ai);
    ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], body).into_response()
}

/// Which feed format a feed URL asks for, from its extension.
#[derive(Clone, Copy, PartialEq)]
enum FeedFormat {
    Rss,
    Atom,
    Json,
}

impl FeedFormat {
    fn of(uri: &axum::http::Uri) -> Self {
        let p = uri.path();
        if p.ends_with(".atom") {
            Self::Atom
        } else if p.ends_with(".json") {
            Self::Json
        } else {
            Self::Rss
        }
    }
}

/// One feed response. Links in post text are made absolute, since readers show it elsewhere.
fn rss_response(
    state: &AppState,
    fmt: FeedFormat,
    self_path: &str,
    title: &str,
    link: &str,
    rows: Vec<feeds::FeedRow>,
) -> Response {
    let base = &state.config.base_url;
    let items: Vec<_> = rows
        .into_iter()
        .map(|r| bf_seo::FeedItem {
            link: format!(
                "{base}{}",
                if r.number > 1 {
                    format!("{}#post-{}", topic_path(r.id, &r.slug), r.number)
                } else {
                    topic_path(r.id, &r.slug)
                }
            ),
            title: r.title,
            description_html: r
                .cooked
                .replace("src=\"/", &format!("src=\"{base}/"))
                .replace("href=\"/", &format!("href=\"{base}/")),
            published: r.created_at,
        })
        .collect();
    let s = state.settings();
    let (home, me) = (format!("{base}{link}"), format!("{base}{self_path}"));
    let (body, ctype) = match fmt {
        FeedFormat::Rss => {
            (bf_seo::rss(title, &home, s.str("site_description"), &items), "application/rss+xml; charset=utf-8")
        }
        FeedFormat::Atom => {
            (bf_seo::atom(title, &home, &me, s.str("site_name"), &items), "application/atom+xml; charset=utf-8")
        }
        FeedFormat::Json => (
            bf_seo::json_feed(title, &home, &me, s.str("site_description"), &items).to_string(),
            "application/feed+json; charset=utf-8",
        ),
    };
    let mut res = xml(body);
    res.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(ctype));
    res
}

async fn latest_rss(State(state): State<AppState>, uri: axum::http::Uri) -> Result<Response, AppError> {
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    let rows = feeds::feed_topics(pool, &tree.readable(&guest()), None).await.db()?;
    let name = state.settings().str("site_name").to_owned();
    Ok(rss_response(&state, FeedFormat::of(&uri), uri.path(), &name, "/", rows))
}

/// Feeds are public, so they show only what guests can read, whoever asks.
async fn forum_rss(
    State(state): State<AppState>,
    uri: axum::http::Uri,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    let node = tree.node(id).filter(|_| tree.readable(&guest()).contains(&id)).ok_or(AppError::NotFound)?;
    let rows = feeds::feed_topics(pool, &[id], None).await.db()?;
    let title = format!("{} - {}", node.title, state.settings().str("site_name"));
    Ok(rss_response(&state, FeedFormat::of(&uri), uri.path(), &title, &forum_path(id, &node.slug), rows))
}

async fn tag_rss(
    State(state): State<AppState>,
    uri: axum::http::Uri,
    Path(slug): Path<String>,
) -> Result<Response, AppError> {
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    let rows = feeds::feed_topics(pool, &tree.readable(&guest()), Some(&slug)).await.db()?;
    let title = format!("{slug} - {}", state.settings().str("site_name"));
    Ok(rss_response(&state, FeedFormat::of(&uri), uri.path(), &title, &format!("/tag/{slug}"), rows))
}

async fn topic_rss(
    State(state): State<AppState>,
    uri: axum::http::Uri,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    let rows = feeds::feed_posts(pool, id, &tree.readable(&guest())).await.db()?;
    let first = rows.first().ok_or(AppError::NotFound)?;
    let (title, link) = (first.title.clone(), topic_path(first.id, &first.slug));
    Ok(rss_response(&state, FeedFormat::of(&uri), uri.path(), &title, &link, rows))
}
