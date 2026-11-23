//! "Recently browsing": who was on a page lately: members by name, and guests and search or AI
//! crawlers as counts and names. Kept in memory, so it costs no query per view and is per server:
//! with several nodes each knows only its own visitors.

use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::http::{HeaderMap, header};
use bf_web::pages::Browsing;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const WINDOW: Duration = Duration::from_secs(15 * 60);
/// Bots shown by name; the rest are counted by the page's total.
const MAX_BOTS: usize = 12;

/// One page's visitors.
#[derive(Default)]
pub struct Seen {
    /// User id to (name, profile url, last seen).
    members: HashMap<i64, (String, String, Instant)>,
    /// Guests by a hash of who they are (address and browser, or a member who hides their
    /// presence), which never leaves memory.
    guests: HashMap<u64, Instant>,
    /// Crawlers by name.
    bots: HashMap<String, Instant>,
}

pub type Visitors = Arc<Mutex<Seen>>;

/// Crawlers known by name: what their user agent contains (lowercase), and what to call them.
const KNOWN: &[(&str, &str)] = &[
    ("googlebot", "Googlebot"),
    ("google-inspectiontool", "Google Inspection"),
    ("adsbot-google", "AdsBot"),
    ("bingbot", "Bingbot"),
    ("duckduckbot", "DuckDuckBot"),
    ("baiduspider", "Baiduspider"),
    ("yandex", "YandexBot"),
    ("applebot", "Applebot"),
    ("gptbot", "GPTBot"),
    ("chatgpt-user", "ChatGPT-User"),
    ("oai-searchbot", "OAI-SearchBot"),
    ("claudebot", "ClaudeBot"),
    ("claude-web", "Claude-Web"),
    ("perplexitybot", "PerplexityBot"),
    ("ccbot", "CCBot"),
    ("amazonbot", "Amazonbot"),
    ("bytespider", "Bytespider"),
    ("petalbot", "PetalBot"),
    ("semrushbot", "SemrushBot"),
    ("ahrefsbot", "AhrefsBot"),
    ("mj12bot", "MJ12bot"),
    ("facebookexternalhit", "Facebook"),
    ("twitterbot", "Twitterbot"),
    ("slackbot", "Slackbot"),
    ("discordbot", "Discordbot"),
    ("telegrambot", "TelegramBot"),
    ("whatsapp", "WhatsApp"),
    ("linkedinbot", "LinkedInBot"),
];

/// The crawler behind a user agent, or `None` for a browser. An empty agent and plain tools count
/// as crawlers too, as they do for view counting.
pub(crate) fn bot_name(user_agent: &str) -> Option<String> {
    let ua = user_agent.to_ascii_lowercase();
    if let Some((_, name)) = KNOWN.iter().find(|(needle, _)| ua.contains(needle)) {
        return Some((*name).to_owned());
    }
    // An unlisted crawler names itself in a token such as "FooBot/1.0".
    if ["bot", "crawl", "spider", "slurp"].iter().any(|b| ua.contains(b)) {
        let token = user_agent
            .split(|c: char| c.is_whitespace() || matches!(c, ';' | '(' | ')' | ',' | '/'))
            .find(|t| {
                let t = t.to_ascii_lowercase();
                ["bot", "crawl", "spider", "slurp"].iter().any(|b| t.contains(b))
            })
            .unwrap_or("");
        let clean: String =
            token.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).take(24).collect();
        return Some(if clean.is_empty() { "Other bot".to_owned() } else { clean });
    }
    if ua.is_empty() || ["curl", "wget", "python", "headless"].iter().any(|b| ua.contains(b)) {
        return Some("Other bot".to_owned());
    }
    None
}

impl Seen {
    /// A visitor who is not signed in: a crawler by name, or one more guest.
    fn note_anonymous(&mut self, agent: &str, ip: Option<std::net::IpAddr>, now: Instant) {
        if let Some(name) = bot_name(agent) {
            self.bots.insert(name, now);
        } else {
            let ip = ip.map(|i| i.to_string()).unwrap_or_default();
            self.guests.insert(hash_of(&["g", &ip, agent]), now);
        }
    }
}

/// The whole site's visitors, for the "Who's online" block, under a key no page has.
const SITE: &str = "site";

/// A path that is a page rather than a file, an API call, a picture or the admin panel.
fn is_page_view(path: &str) -> bool {
    !["/api/", "/assets/", "/avatar/", "/admin", "/uploads/", "/img/"].iter().any(|p| path.starts_with(p))
        && !path.rsplit('/').next().unwrap_or("").contains('.')
}

/// Visitors who are not signed in, site-wide: how many guests, how many crawlers, and the latest
/// few crawlers by name.
pub(crate) fn site_anonymous(state: &AppState) -> (usize, usize, Vec<String>) {
    let Some(visitors) = state.browsing.get(SITE) else { return (0, 0, Vec::new()) };
    let mut seen = visitors.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let now = Instant::now();
    seen.guests.retain(|_, at| now.duration_since(*at) < WINDOW);
    seen.bots.retain(|_, at| now.duration_since(*at) < WINDOW);
    let mut bots: Vec<_> = seen.bots.iter().map(|(n, at)| (n.clone(), *at)).collect();
    bots.sort_by_key(|b| std::cmp::Reverse(b.1));
    (seen.guests.len(), bots.len(), bots.into_iter().take(MAX_BOTS).map(|b| b.0).collect())
}

/// The page a path is a view of, as `visit` names it: "home", "forum:ID" or "topic:ID".
fn page_key(path: &str) -> Option<String> {
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    match parts.as_slice() {
        [""] => Some("home".to_owned()),
        ["t", _, id] if id.parse::<i64>().is_ok() => Some(format!("topic:{id}")),
        ["c", _, id] if id.parse::<i64>().is_ok() => Some(format!("forum:{id}")),
        _ => None,
    }
}

/// Guest pages are served from the microcache without running their handlers, so a guest or
/// crawler that hits the cache would go uncounted. This runs ahead of the cache and adds the
/// visit to a page that already has a visitor list (the first view builds it).
pub(crate) async fn guest_hit(
    axum::extract::State(state): axum::extract::State<AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let signed_in = req
        .headers()
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .any(|c| c.split(';').any(|kv| kv.trim_start().starts_with("__Host-bf_session")));
    if req.method() == axum::http::Method::GET && !signed_in {
        let path = req.uri().path();
        let page = if state.settings().bool("recently_browsing") {
            page_key(path).and_then(|k| state.browsing.get(&k))
        } else {
            None
        };
        let site = is_page_view(path);
        if page.is_some() || site {
            let peer = req
                .extensions()
                .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
                .map(|c| crate::http::ratelimit::client_ip(c.0.ip(), req.headers()));
            let agent = req.headers().get(header::USER_AGENT).and_then(|h| h.to_str().ok()).unwrap_or("");
            let now = Instant::now();
            let note = |v: &Visitors| {
                v.lock().unwrap_or_else(std::sync::PoisonError::into_inner).note_anonymous(agent, peer, now);
            };
            if let Some(visitors) = &page {
                note(visitors);
            }
            if site {
                note(&state.browsing.get_with(SITE.to_owned(), Visitors::default));
            }
        }
    }
    next.run(req).await
}

fn hash_of(parts: &[&str]) -> u64 {
    let mut h = DefaultHasher::new();
    parts.hash(&mut h);
    h.finish()
}

/// Records this view and returns who is here now: members (newest first), the number of guests and
/// the crawlers. None when switched off.
pub(crate) fn visit(state: &AppState, v: &Viewer, headers: &HeaderMap, page: &str) -> Option<Browsing> {
    if !state.settings().bool("recently_browsing") {
        return None;
    }
    let visitors: Visitors = state.browsing.get_with(page.to_owned(), Visitors::default);
    let mut seen = visitors.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let now = Instant::now();
    seen.members.retain(|_, (_, _, at)| now.duration_since(*at) < WINDOW);
    seen.guests.retain(|_, at| now.duration_since(*at) < WINDOW);
    seen.bots.retain(|_, at| now.duration_since(*at) < WINDOW);
    let agent = headers.get(header::USER_AGENT).and_then(|h| h.to_str().ok()).unwrap_or("");
    match v.user.as_ref() {
        // Members hiding their last-seen time stay out of the names, as in the online list, but
        // still count as someone being here.
        Some(u) if u.hide_last_seen => {
            seen.guests.insert(hash_of(&["u", &u.id.get().to_string()]), now);
        }
        Some(u) => {
            seen.members
                .insert(u.id.get(), (u.username.clone(), bf_core::slug::profile_path(u.id.get(), &u.username), now));
        }
        None => seen.note_anonymous(agent, v.ip, now),
    }
    let mut members: Vec<_> = seen.members.values().cloned().collect();
    members.sort_by_key(|r| std::cmp::Reverse(r.2));
    members.truncate(50);
    let mut bots: Vec<_> = seen.bots.iter().map(|(n, at)| (n.clone(), *at)).collect();
    bots.sort_by_key(|b| std::cmp::Reverse(b.1));
    Some(Browsing {
        members: members.into_iter().map(|(name, url, _)| (name, url)).collect(),
        guests: seen.guests.len(),
        bot_count: bots.len(),
        bots: bots.into_iter().take(MAX_BOTS).map(|b| b.0).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::{bot_name, page_key};

    #[test]
    fn names_the_page_a_path_views() {
        assert_eq!(page_key("/").as_deref(), Some("home"));
        assert_eq!(page_key("/t/some-title/67").as_deref(), Some("topic:67"));
        assert_eq!(page_key("/c/general/26").as_deref(), Some("forum:26"));
        assert_eq!(page_key("/u/someone"), None);
        assert_eq!(page_key("/t/some-title/x"), None);
    }

    #[test]
    fn names_crawlers_and_leaves_browsers() {
        assert_eq!(
            bot_name("Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)").as_deref(),
            Some("Googlebot")
        );
        assert_eq!(
            bot_name("Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; GPTBot/1.1)").as_deref(),
            Some("GPTBot")
        );
        assert_eq!(bot_name("Mozilla/5.0 (compatible; FooBot/1.0)").as_deref(), Some("FooBot"));
        assert_eq!(bot_name("curl/8.4.0").as_deref(), Some("Other bot"));
        assert_eq!(bot_name("").as_deref(), Some("Other bot"));
        assert_eq!(
            bot_name("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/126 Safari/537.36"),
            None
        );
    }
}
