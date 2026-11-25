//! SEO output: sitemaps, RSS, JSON-LD and robots.txt. Pure functions over
//! plain data, so pages and jobs share them.

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

/// Escapes text for XML element content and attribute values.
pub fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // Control characters are invalid in XML 1.0.
            c if c.is_control() && !matches!(c, '\t' | '\n' | '\r') => {}
            '\u{fffe}' | '\u{ffff}' => {}
            c => out.push(c),
        }
    }
    out
}

/// One file holds at most this many URLs (sitemaps.org limit).
pub const SITEMAP_CHUNK: usize = 50_000;

pub struct SitemapUrl {
    pub loc: String,
    pub lastmod: Option<DateTime<Utc>>,
}

pub fn sitemap(urls: &[SitemapUrl]) -> String {
    let mut s = String::from(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    s.push_str(r#"<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">"#);
    for u in urls.iter().take(SITEMAP_CHUNK) {
        s.push_str("<url><loc>");
        s.push_str(&xml_escape(&u.loc));
        s.push_str("</loc>");
        if let Some(t) = u.lastmod {
            s.push_str(&format!("<lastmod>{}</lastmod>", t.format("%Y-%m-%dT%H:%M:%SZ")));
        }
        s.push_str("</url>");
    }
    s.push_str("</urlset>");
    s
}

/// How many sitemap files `total` URLs need (at least one).
pub fn sitemap_files(total: usize) -> usize {
    total.div_ceil(SITEMAP_CHUNK).max(1)
}

/// What file number `part` (from 1) of a sitemap holds when the URLs are `fixed` ones that are
/// always listed first (home page, forums) followed by `topics` newest first: which of the
/// fixed ones, and which run of topics (offset, count). `None` if there is no such file.
pub fn sitemap_slice(part: usize, fixed: usize, topics: usize) -> Option<(std::ops::Range<usize>, usize, usize)> {
    if part == 0 || part > sitemap_files(fixed.saturating_add(topics)) {
        return None;
    }
    let (start, end) = ((part - 1) * SITEMAP_CHUNK, (part * SITEMAP_CHUNK).min(fixed.saturating_add(topics)));
    let own = start.min(fixed)..end.min(fixed);
    let first_topic = start.saturating_sub(fixed);
    Some((own, first_topic, end.saturating_sub(fixed).saturating_sub(first_topic)))
}

pub fn sitemap_index(sitemaps: &[String]) -> String {
    let mut s = String::from(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    s.push_str(r#"<sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">"#);
    for loc in sitemaps {
        s.push_str(&format!("<sitemap><loc>{}</loc></sitemap>", xml_escape(loc)));
    }
    s.push_str("</sitemapindex>");
    s
}

pub struct FeedItem {
    pub title: String,
    pub link: String,
    pub description_html: String,
    pub published: DateTime<Utc>,
}

pub fn rss(title: &str, link: &str, description: &str, items: &[FeedItem]) -> String {
    let mut s = String::from(r#"<?xml version="1.0" encoding="UTF-8"?><rss version="2.0"><channel>"#);
    s.push_str(&format!(
        "<title>{}</title><link>{}</link><description>{}</description>",
        xml_escape(title),
        xml_escape(link),
        xml_escape(description)
    ));
    for i in items {
        s.push_str(&format!(
            "<item><title>{}</title><link>{}</link><guid isPermaLink=\"true\">{}</guid><pubDate>{}</pubDate><description>{}</description></item>",
            xml_escape(&i.title),
            xml_escape(&i.link),
            xml_escape(&i.link),
            i.published.to_rfc2822(),
            xml_escape(&i.description_html),
        ));
    }
    s.push_str("</channel></rss>");
    s
}

/// Atom 1.0 (RFC 4287) for the same items as `rss`.
pub fn atom(title: &str, link: &str, self_link: &str, author: &str, items: &[FeedItem]) -> String {
    let updated = items.iter().map(|i| i.published).max().unwrap_or_else(Utc::now);
    let mut s = String::from(r#"<?xml version="1.0" encoding="UTF-8"?><feed xmlns="http://www.w3.org/2005/Atom">"#);
    s.push_str(&format!(
        r#"<title>{}</title><id>{}</id><link rel="alternate" href="{}"/><link rel="self" href="{}"/><updated>{}</updated><author><name>{}</name></author>"#,
        xml_escape(title),
        xml_escape(link),
        xml_escape(link),
        xml_escape(self_link),
        updated.to_rfc3339(),
        xml_escape(author),
    ));
    for i in items {
        s.push_str(&format!(
            r#"<entry><title>{}</title><id>{}</id><link rel="alternate" href="{}"/><published>{}</published><updated>{}</updated><content type="html">{}</content></entry>"#,
            xml_escape(&i.title),
            xml_escape(&i.link),
            xml_escape(&i.link),
            i.published.to_rfc3339(),
            i.published.to_rfc3339(),
            xml_escape(&i.description_html),
        ));
    }
    s.push_str("</feed>");
    s
}

/// JSON Feed 1.1 (jsonfeed.org) for the same items as `rss`.
pub fn json_feed(title: &str, link: &str, self_link: &str, description: &str, items: &[FeedItem]) -> Value {
    json!({
        "version": "https://jsonfeed.org/version/1.1",
        "title": title,
        "home_page_url": link,
        "feed_url": self_link,
        "description": description,
        "items": items.iter().map(|i| json!({
            "id": i.link,
            "url": i.link,
            "title": i.title,
            "content_html": i.description_html,
            "date_published": i.published.to_rfc3339(),
        })).collect::<Vec<_>>(),
    })
}

/// Crawlers that collect text for AI models rather than for a search index. Ordinary
/// search crawlers are unaffected; `Google-Extended` and `Applebot-Extended` are the AI opt-out
/// tokens of those two companies.
pub const AI_CRAWLERS: &[&str] = &[
    "GPTBot",
    "ChatGPT-User",
    "OAI-SearchBot",
    "ClaudeBot",
    "Claude-User",
    "Claude-SearchBot",
    "anthropic-ai",
    "Google-Extended",
    "Applebot-Extended",
    "CCBot",
    "PerplexityBot",
    "Perplexity-User",
    "Bytespider",
    "Meta-ExternalAgent",
    "Amazonbot",
    "cohere-ai",
    "Diffbot",
    "DuckAssistBot",
    "MistralAI-User",
];

/// The rules robots.txt starts with: the setting's default, and what an empty setting serves.
pub const ROBOTS_RULES: &str =
    "User-agent: *\nDisallow: /admin\nDisallow: /api/\nDisallow: /search\nDisallow: /login\nDisallow: /register";

pub fn robots_txt(base_url: &str) -> String {
    robots_txt_with(base_url, false)
}

pub fn robots_txt_with(base_url: &str, block_ai: bool) -> String {
    robots_txt_custom(base_url, "", block_ai)
}

/// The admin's rules (the built-in ones when empty), then the AI-crawler block when it is on, then
/// the sitemap line, which is always there.
pub fn robots_txt_custom(base_url: &str, rules: &str, block_ai: bool) -> String {
    let rules = rules.trim();
    let mut s = if rules.is_empty() { ROBOTS_RULES.to_owned() } else { rules.replace("\r\n", "\n") };
    s.push('\n');
    if block_ai {
        s.push('\n');
        for bot in AI_CRAWLERS {
            s.push_str(&format!("User-agent: {bot}\n"));
        }
        s.push_str("Disallow: /\n");
    }
    let sitemap = format!("Sitemap: {}/sitemap.xml", base_url.trim_end_matches('/'));
    if !s.lines().any(|l| l.trim() == sitemap) {
        s.push_str(&format!("\n{sitemap}\n"));
    }
    s
}

/// First ~160 characters of plain text from cooked HTML, for meta descriptions.
pub fn excerpt(html: &str, max: usize) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                text.push(' ');
            }
            c if !in_tag => text.push(c),
            _ => {}
        }
    }
    let text = text
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    let words = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if words.chars().count() <= max {
        return words;
    }
    let cut: String = words.chars().take(max).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(a, _)| a);
    format!("{cut}…")
}

pub struct JsonLdPost<'a> {
    pub author: &'a str,
    pub author_url: &'a str,
    pub date: DateTime<Utc>,
    pub text: String,
    pub url: &'a str,
}

/// `DiscussionForumPosting`: the first post plus replies as `comment[]`.
pub fn discussion_posting(headline: &str, first: &JsonLdPost, replies: &[JsonLdPost], reply_count: i64) -> Value {
    let person = |p: &JsonLdPost| json!({ "@type": "Person", "name": p.author, "url": p.author_url });
    json!({
        "@context": "https://schema.org",
        "@type": "DiscussionForumPosting",
        "headline": headline,
        "url": first.url,
        "datePublished": first.date.to_rfc3339(),
        "author": person(first),
        "text": first.text,
        "interactionStatistic": {
            "@type": "InteractionCounter",
            "interactionType": "https://schema.org/CommentAction",
            "userInteractionCount": reply_count
        },
        "comment": replies.iter().map(|r| json!({
            "@type": "Comment",
            "url": r.url,
            "datePublished": r.date.to_rfc3339(),
            "author": person(r),
            "text": r.text,
        })).collect::<Vec<_>>(),
    })
}

/// `QAPage` for a topic with a marked answer. The rest are suggested answers.
pub fn qa_page(headline: &str, first: &JsonLdPost, answer: &JsonLdPost, replies: &[JsonLdPost], count: i64) -> Value {
    let person = |p: &JsonLdPost| json!({ "@type": "Person", "name": p.author, "url": p.author_url });
    let one = |p: &JsonLdPost| json!({ "@type": "Answer", "url": p.url, "text": p.text, "datePublished": p.date.to_rfc3339(), "author": person(p) });
    json!({
        "@context": "https://schema.org",
        "@type": "QAPage",
        "mainEntity": {
            "@type": "Question",
            "name": headline,
            "text": first.text,
            "datePublished": first.date.to_rfc3339(),
            "author": person(first),
            "answerCount": count,
            "acceptedAnswer": one(answer),
            "suggestedAnswer": replies.iter().filter(|r| r.url != answer.url).map(one).collect::<Vec<_>>(),
        },
    })
}

/// `WebSite` with a sitelinks search box: once on the home page, so Google may offer a
/// search field for this site right in its results.
pub fn website(name: &str, base_url: &str) -> Value {
    let base = base_url.trim_end_matches('/');
    json!({
        "@context": "https://schema.org",
        "@type": "WebSite",
        "name": name,
        "url": format!("{base}/"),
        "potentialAction": {
            "@type": "SearchAction",
            "target": format!("{base}/search?q={{search_term_string}}"),
            "query-input": "required name=search_term_string",
        },
    })
}

/// `Organization` for the home page: the site as a publisher, with its logo when it has one.
pub fn organization(name: &str, base_url: &str, logo: Option<&str>) -> Value {
    let base = base_url.trim_end_matches('/');
    let mut v = json!({
        "@context": "https://schema.org",
        "@type": "Organization",
        "name": name,
        "url": format!("{base}/"),
    });
    if let Some(logo) = logo.filter(|l| !l.is_empty()) {
        let abs = if logo.starts_with("http") { logo.to_owned() } else { format!("{base}{logo}") };
        v["logo"] = json!(abs);
    }
    v
}

/// What a public profile may say about its member (`ProfilePage` with a `Person`). Only fields
/// the member already shows to guests belong here.
pub struct JsonLdProfile<'a> {
    pub name: &'a str,
    pub url: &'a str,
    pub image: Option<&'a str>,
    pub joined: DateTime<Utc>,
    pub about: Option<&'a str>,
    pub post_count: i64,
}

/// `ProfilePage` whose main entity is the member as a `Person`.
pub fn profile_page(p: &JsonLdProfile) -> Value {
    let mut person = json!({
        "@type": "Person",
        "name": p.name,
        "url": p.url,
        "interactionStatistic": {
            "@type": "InteractionCounter",
            "interactionType": "https://schema.org/WriteAction",
            "userInteractionCount": p.post_count
        },
    });
    if let Some(i) = p.image {
        person["image"] = json!(i);
    }
    if let Some(a) = p.about.filter(|a| !a.is_empty()) {
        person["description"] = json!(a);
    }
    json!({
        "@context": "https://schema.org",
        "@type": "ProfilePage",
        "dateCreated": p.joined.to_rfc3339(),
        "mainEntity": person,
    })
}

/// An event the page is about.
pub struct JsonLdEvent<'a> {
    pub name: &'a str,
    pub url: &'a str,
    pub description: Option<&'a str>,
    pub starts: DateTime<Utc>,
    pub ends: Option<DateTime<Utc>>,
    /// A street address or a place name; `None` for an online event.
    pub location: Option<&'a str>,
    pub online_url: Option<&'a str>,
    pub organizer: Option<&'a str>,
    pub cancelled: bool,
}

/// `Event`, attendance mode and status included.
pub fn event(e: &JsonLdEvent) -> Value {
    let online = e.location.is_none() && e.online_url.is_some();
    let mut v = json!({
        "@context": "https://schema.org",
        "@type": "Event",
        "name": e.name,
        "url": e.url,
        "startDate": e.starts.to_rfc3339(),
        "eventStatus": if e.cancelled { "https://schema.org/EventCancelled" } else { "https://schema.org/EventScheduled" },
        "eventAttendanceMode": if online {
            "https://schema.org/OnlineEventAttendanceMode"
        } else {
            "https://schema.org/OfflineEventAttendanceMode"
        },
    });
    if let Some(t) = e.ends {
        v["endDate"] = json!(t.to_rfc3339());
    }
    if let Some(d) = e.description.filter(|d| !d.is_empty()) {
        v["description"] = json!(d);
    }
    if let Some(place) = e.location {
        v["location"] = json!({ "@type": "Place", "name": place });
    } else if let Some(u) = e.online_url {
        v["location"] = json!({ "@type": "VirtualLocation", "url": u });
    }
    if let Some(o) = e.organizer {
        v["organizer"] = json!({ "@type": "Person", "name": o });
    }
    v
}

/// `CollectionPage` with an `ItemList`: the topics a forum or tag list shows, in order.
/// `items` are (absolute url, title) pairs; `offset` is how many entries precede this page.
pub fn item_list(name: &str, url: &str, items: &[(String, String)], offset: usize) -> Value {
    json!({
        "@context": "https://schema.org",
        "@type": "CollectionPage",
        "name": name,
        "url": url,
        "mainEntity": {
            "@type": "ItemList",
            "itemListOrder": "https://schema.org/ItemListUnordered",
            "numberOfItems": items.len(),
            "itemListElement": items.iter().enumerate().map(|(i, (u, t))| json!({
                "@type": "ListItem", "position": offset + i + 1, "url": u, "name": t
            })).collect::<Vec<_>>(),
        },
    })
}

pub fn breadcrumbs(items: &[(&str, &str)]) -> Value {
    json!({
        "@context": "https://schema.org",
        "@type": "BreadcrumbList",
        "itemListElement": items.iter().enumerate().map(|(i, (name, url))| json!({
            "@type": "ListItem", "position": i + 1, "name": name, "item": url
        })).collect::<Vec<_>>(),
    })
}

/// Serializes JSON-LD for a `<script type="application/ld+json">` block. `<` is
/// escaped so user text can never close the script element.
pub fn json_ld_script(v: &Value) -> String {
    v.to_string().replace('<', "\\u003c")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 1, 12, 0, 0).unwrap()
    }

    #[test]
    fn big_sites_get_several_sitemap_files_that_cover_everything_once() {
        assert_eq!(sitemap_files(0), 1);
        assert_eq!(sitemap_files(SITEMAP_CHUNK), 1);
        assert_eq!(sitemap_files(SITEMAP_CHUNK + 1), 2);
        let (fixed, topics) = (10, 120_000);
        let files = sitemap_files(fixed + topics);
        assert_eq!(files, 3);
        let (mut seen_fixed, mut seen_topics, mut next_topic) = (0, 0, 0);
        for part in 1..=files {
            let (own, offset, count) = sitemap_slice(part, fixed, topics).unwrap();
            assert!(own.len() + count <= SITEMAP_CHUNK);
            assert_eq!(offset, next_topic, "topics follow on from the previous file");
            seen_fixed += own.len();
            seen_topics += count;
            next_topic += count;
        }
        assert_eq!((seen_fixed, seen_topics), (fixed, topics));
        assert!(sitemap_slice(0, fixed, topics).is_none() && sitemap_slice(4, fixed, topics).is_none());
        // A small site is one file with everything in it.
        assert_eq!(sitemap_slice(1, 3, 4), Some((0..3, 0, 4)));
    }

    #[test]
    fn sitemap_escapes_and_formats() {
        let s = sitemap(&[SitemapUrl { loc: "https://f.test/topic/1-a&b/".into(), lastmod: Some(t()) }]);
        assert!(s.contains("<loc>https://f.test/topic/1-a&amp;b/</loc>"));
        assert!(s.contains("<lastmod>2026-10-01T12:00:00Z</lastmod>"));
    }

    #[test]
    fn rss_escapes_html() {
        let s = rss(
            "F",
            "https://f.test/",
            "d",
            &[FeedItem {
                title: "<b>".into(),
                link: "https://f.test/t".into(),
                description_html: "<p>x</p>".into(),
                published: t(),
            }],
        );
        assert!(s.contains("<title>&lt;b&gt;</title>"));
        assert!(s.contains("&lt;p&gt;x&lt;/p&gt;"));
    }

    #[test]
    fn excerpt_strips_tags_and_truncates_on_word() {
        assert_eq!(excerpt("<p>Hello <b>big</b> world &amp; more</p>", 160), "Hello big world & more");
        assert_eq!(excerpt("<p>one two three</p>", 9), "one two…");
    }

    #[test]
    fn json_ld_cannot_break_out_of_script() {
        let p = JsonLdPost { author: "a", author_url: "/u", date: t(), text: "</script><script>x".into(), url: "/t" };
        let out = json_ld_script(&discussion_posting("h", &p, &[], 0));
        assert!(!out.contains("</script"));
    }

    #[test]
    fn website_has_a_search_action() {
        let w = website("Forum", "https://f.test/");
        assert_eq!(w["url"], "https://f.test/");
        assert_eq!(w["potentialAction"]["target"], "https://f.test/search?q={search_term_string}");
    }

    #[test]
    fn custom_robots_rules_keep_the_sitemap_and_the_ai_block() {
        let own = robots_txt_custom("https://f.test/", "User-agent: *\r\nDisallow: /x", false);
        assert_eq!(own, "User-agent: *\nDisallow: /x\n\nSitemap: https://f.test/sitemap.xml\n");
        // Another sitemap line does not stand in for this site's.
        let other = robots_txt_custom("https://f.test", "Sitemap: https://o.test/s.xml", true);
        assert!(other.contains("GPTBot") && other.ends_with("Sitemap: https://f.test/sitemap.xml\n"));
        // Its own line is not repeated.
        let same = robots_txt_custom("https://f.test", "Sitemap: https://f.test/sitemap.xml", false);
        assert_eq!(same.matches("Sitemap:").count(), 1);
        assert_eq!(robots_txt_custom("https://f.test", "  ", false), robots_txt("https://f.test"));
    }

    #[test]
    fn robots_points_at_sitemap() {
        assert!(robots_txt("https://f.test/").contains("Sitemap: https://f.test/sitemap.xml"));
        let blocked = robots_txt_with("https://f.test", true);
        assert!(blocked.contains("User-agent: GPTBot\n") && blocked.contains("User-agent: ClaudeBot\n"));
        assert!(!robots_txt("https://f.test").contains("GPTBot"));
    }

    #[test]
    fn atom_and_json_feed_escape_and_carry_items() {
        let items = [FeedItem {
            title: "a < b".into(),
            link: "https://f.test/t/1-x".into(),
            description_html: "<p>hi</p>".into(),
            published: chrono::DateTime::from_timestamp(0, 0).unwrap(),
        }];
        let a = atom("Site", "https://f.test/", "https://f.test/latest.atom", "Site", &items);
        assert!(a.contains("<title>a &lt; b</title>") && a.contains("&lt;p&gt;hi&lt;/p&gt;"));
        let j = json_feed("Site", "https://f.test/", "https://f.test/latest.json", "", &items);
        assert_eq!(j["items"][0]["content_html"], "<p>hi</p>");
    }
}
