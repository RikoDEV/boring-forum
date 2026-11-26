//! The activity summary (the digest) as email: a band of counts since the last one, then a card
//! for each of the busiest topics, then what else happened.
//!
//! Unlike other mail this cannot be read back from the text, so the job that writes the digest
//! keeps a [`Summary`] with the queued mail. The text version, and the words an administrator
//! wrote around its list of topics, stay as they are.

use super::{Block, Brand, Mail, Span, blocks, colours, footer, initial, language};
use askama::Template;
use bf_core::i18n::T;
use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A topic shown in full.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Card {
    pub forum: String,
    pub title: String,
    pub url: String,
    pub created: DateTime<Utc>,
    /// Empty for a deleted account.
    pub author: String,
    /// The first post's opening paragraphs; none when the site does not quote posts in mail.
    pub excerpt: Vec<String>,
    /// The first post's first picture, as an `/uploads/…` path.
    pub image: Option<String>,
    pub likes: i64,
    pub replies: i64,
    /// A few of those who replied.
    pub participants: Vec<String>,
}

/// A topic shown as one line.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Line {
    pub title: String,
    pub url: String,
    pub replies: i64,
}

/// Everything the summary shows, as the member could read it when it was written.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Summary {
    pub new_topics: i64,
    pub unread: i64,
    pub likes: i64,
    /// Members who joined, in the monthly summary only.
    pub new_members: i64,
    pub cards: Vec<Card>,
    /// New replies in topics the member watches.
    pub watched: Vec<Line>,
    /// Further topics, past those with a card.
    pub more: Vec<Line>,
}

/// The pictures the renderer may use, already made ready: how the mail names each (`cid:…`, or a
/// `data:` URL in a preview). A member with none gets their initial; a card with none, nothing.
#[derive(Debug, Default)]
pub struct Pictures {
    /// By username.
    pub avatars: HashMap<String, String>,
    /// By the `/uploads/…` path a card names.
    pub images: HashMap<String, String>,
}

struct Face {
    name: String,
    picture: Option<String>,
    letter: String,
    colour: &'static str,
}

fn face(name: &str, pictures: &Pictures) -> Face {
    let (letter, colour) = initial(name);
    Face { name: name.to_owned(), picture: pictures.avatars.get(name).cloned(), letter, colour }
}

struct CardView {
    forum: String,
    /// The square beside the forum's name: a steady colour for each name.
    swatch: &'static str,
    title: String,
    url: String,
    date: String,
    author: Option<Face>,
    excerpt: Vec<String>,
    image: Option<String>,
    likes: i64,
    replies: i64,
    faces: Vec<Face>,
}

struct Row {
    title: String,
    url: String,
    note: String,
}

#[derive(Template)]
#[template(path = "mail_summary.html")]
struct Page<'a> {
    lang: &'a str,
    dir: &'static str,
    start: &'static str,
    end: &'static str,
    site: &'a str,
    base: &'a str,
    logo: Option<(&'a str, u32, u32)>,
    heading: &'a str,
    preview: String,
    accent: String,
    on_accent: &'static str,
    dark_link: String,
    force_dark: bool,
    since: String,
    stats: Vec<(i64, String)>,
    popular: String,
    read_more: String,
    likes_label: String,
    replies_label: String,
    cards: Vec<CardView>,
    /// (heading, topics) of the plain lists under the cards.
    lists: Vec<(String, Vec<Row>)>,
    joined: Option<String>,
    /// What the text says above and below its list of topics, as paragraphs of lines.
    intro: Vec<Vec<Vec<Span>>>,
    outro: Vec<Vec<Vec<Span>>>,
    reason: String,
    links: Vec<(String, String)>,
}

/// "November 2", in the reader's language.
fn day(t: &T, at: DateTime<Utc>) -> String {
    let number = at.day().to_string();
    let day = [("day", number.as_str())];
    match at.month() {
        1 => t.msgf("January {day}", &day),
        2 => t.msgf("February {day}", &day),
        3 => t.msgf("March {day}", &day),
        4 => t.msgf("April {day}", &day),
        5 => t.msgf("May {day}", &day),
        6 => t.msgf("June {day}", &day),
        7 => t.msgf("July {day}", &day),
        8 => t.msgf("August {day}", &day),
        9 => t.msgf("September {day}", &day),
        10 => t.msgf("October {day}", &day),
        11 => t.msgf("November {day}", &day),
        _ => t.msgf("December {day}", &day),
    }
}

/// The text's paragraphs outside its list of topics. The list is found by the topic addresses the
/// summary shows as cards and lines: everything from the first paragraph that names one to the
/// last is replaced by them.
fn around(body: &str, brand: &Brand, unsubscribe: Option<&str>, summary: &Summary) -> [Vec<Vec<Vec<Span>>>; 2] {
    let shown: Vec<&str> = summary
        .cards
        .iter()
        .map(|c| c.url.as_str())
        .chain(summary.watched.iter().chain(&summary.more).map(|l| l.url.as_str()))
        .collect();
    let found = blocks(body, brand.site, brand.base, unsubscribe);
    let listed =
        |b: &Block| matches!(b, Block::List(entries) if entries.iter().any(|e| shown.contains(&e.url.as_str())));
    let first = found.iter().position(listed).unwrap_or(found.len());
    let last = found.iter().rposition(listed).map_or(first, |i| i + 1);
    let mut before = Vec::new();
    let mut after = Vec::new();
    // The counts above say what the sentence over the link to the notifications says.
    let notifications = format!("{}/notifications", brand.base);
    let mut pointing: Vec<usize> = Vec::new();
    for (i, pair) in found.windows(2).enumerate() {
        if matches!(&pair[1], Block::Button(url) if *url == notifications) {
            pointing.push(i);
        }
    }
    for (i, block) in found.into_iter().enumerate() {
        let Block::Text(lines) = block else { continue };
        if pointing.contains(&i) || lines.iter().flatten().any(|s| s.text == notifications) {
            continue;
        }
        // A greeting alone ("Hi Marta,"): the layout opens with the counts instead.
        let greeting = matches!(lines.as_slice(), [line] if line.iter().all(|s| !s.link)
            && line.last().is_some_and(|s| s.text.ends_with([',', '،'])));
        if greeting {
            continue;
        }
        if i < first {
            before.push(lines);
        } else if i >= last {
            after.push(lines);
        }
    }
    [before, after]
}

/// The HTML version of a digest.
pub fn render_summary(
    brand: &Brand,
    mail: &Mail,
    summary: &Summary,
    pictures: &Pictures,
    lang: &str,
    force_dark: bool,
) -> Result<String, askama::Error> {
    let lang = language(mail.body, lang);
    let t = T::new(lang);
    let [intro, outro] = around(mail.body, brand, mail.unsubscribe_url, summary);
    let cards: Vec<CardView> = summary
        .cards
        .iter()
        .map(|c| CardView {
            forum: c.forum.clone(),
            swatch: initial(&c.forum).1,
            title: c.title.clone(),
            url: c.url.clone(),
            date: day(&t, c.created),
            author: (!c.author.is_empty()).then(|| face(&c.author, pictures)),
            excerpt: c.excerpt.clone(),
            image: c.image.as_ref().and_then(|path| pictures.images.get(path).cloned()),
            likes: c.likes,
            replies: c.replies,
            faces: c.participants.iter().map(|name| face(name, pictures)).collect(),
        })
        .collect();
    let rows = |lines: &[Line]| -> Vec<Row> {
        lines
            .iter()
            .map(|l| Row { title: l.title.clone(), url: l.url.clone(), note: t.n("count.replies", l.replies) })
            .collect()
    };
    let lists: Vec<(String, Vec<Row>)> =
        [(t.msg("Replies in topics you follow"), rows(&summary.watched)), (t.msg("More topics"), rows(&summary.more))]
            .into_iter()
            .filter(|(_, rows)| !rows.is_empty())
            .collect();
    let stats = vec![
        (summary.new_topics, t.msg("New topics")),
        (summary.unread, t.msg("Unread notifications")),
        (summary.likes, t.msg("Likes received")),
    ];
    let joined = (summary.new_members > 0)
        .then(|| t.msgf("New members this month: {count}", &[("count", &summary.new_members.to_string())]));
    let (reason, links) = footer(&t, brand, mail);
    let preview = summary.cards.iter().map(|c| c.title.as_str()).collect::<Vec<_>>().join(" · ");
    let suffix = format!(" - {}", brand.site);
    let (accent, on_accent, dark_link) = colours(brand.accent);
    let rtl = lang == "ar";
    Page {
        lang,
        dir: if rtl { "rtl" } else { "ltr" },
        start: if rtl { "right" } else { "left" },
        end: if rtl { "left" } else { "right" },
        site: brand.site,
        base: brand.base,
        logo: brand.logo,
        heading: mail.subject.strip_suffix(&suffix).unwrap_or(mail.subject),
        preview: preview.chars().take(140).collect(),
        accent,
        on_accent,
        dark_link,
        force_dark,
        since: t.msg("Since your last summary"),
        stats,
        popular: t.msg("Popular topics"),
        read_more: t.msg("Read more"),
        likes_label: t.msg("Likes"),
        replies_label: t.msg("Replies"),
        cards,
        lists,
        joined,
        intro,
        outro,
        reason,
        links,
    }
    .render()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "https://f.test";

    fn brand() -> Brand<'static> {
        Brand { site: "Forum", base: BASE, accent: "#4a6da0", logo: None }
    }

    fn summary() -> Summary {
        Summary {
            new_topics: 87,
            unread: 1,
            likes: 22,
            cards: vec![Card {
                forum: "Support <b>".into(),
                title: "A <script> title".into(),
                url: format!("{BASE}/t/a/1"),
                created: "2026-10-02T10:00:00Z".parse().unwrap(),
                author: "stephen".into(),
                excerpt: vec!["First <img src=x>".into(), "Second".into()],
                image: Some("/uploads/pic.png".into()),
                likes: 28,
                replies: 10,
                participants: vec!["scot".into(), "marco".into()],
            }],
            more: vec![Line { title: "Another".into(), url: format!("{BASE}/t/b/2"), replies: 3 }],
            ..Summary::default()
        }
    }

    const BODY: &str = "Hi jane,\n\nNew topics since your last digest:\n\n- A title (10 replies)\n  https://f.test/t/a/1\n- Another (3 replies)\n  https://f.test/t/b/2\n\nYou have 1 unread notifications.\nhttps://f.test/notifications\n\nOur own closing words.\nUnsubscribe from digests: https://f.test/unsubscribe/u\n\n— Forum\n";

    fn render(pictures: &Pictures, lang: &str) -> String {
        let mail =
            Mail { subject: "What is new - Forum", body: BODY, unsubscribe_url: Some("https://f.test/unsubscribe/u") };
        render_summary(&brand(), &mail, &summary(), pictures, lang, false).unwrap()
    }

    #[test]
    fn the_cards_and_counts_replace_the_list_and_keep_the_words_around_it() {
        let out = render(&Pictures::default(), "en");
        for part in [
            "Since your last summary",
            ">87<",
            "Likes received",
            "Popular topics",
            "October 2",
            "Read more",
            "Our own closing words.",
            "More topics",
            "href=\"https://f.test/t/b/2\"",
            "href=\"https://f.test/unsubscribe/u\"",
        ] {
            assert!(out.contains(part), "{part}\n{out}");
        }
        for gone in ["New topics since your last digest", "- A title", "unread notifications.", "— Forum", "Hi jane,"]
        {
            assert!(!out.contains(gone), "{gone}\n{out}");
        }
    }

    #[test]
    fn nothing_a_member_wrote_is_markup() {
        let out = render(&Pictures::default(), "en");
        for markup in ["<script>", "<img src=x>", "Support <b>"] {
            assert!(!out.contains(markup), "{markup}\n{out}");
        }
    }

    #[test]
    fn pictures_show_when_there_are_any_and_initials_when_not() {
        // Without pictures there is no image at all: nothing a client would have to fetch.
        let bare = render(&Pictures::default(), "en");
        assert!(!bare.contains("<img"), "{bare}");
        assert!(bare.contains(">S</td>"), "an initial for stephen");
        let pictures = Pictures {
            avatars: HashMap::from([("stephen".to_owned(), "cid:a1".to_owned())]),
            images: HashMap::from([("/uploads/pic.png".to_owned(), "cid:p1".to_owned())]),
        };
        let out = render(&pictures, "en");
        assert!(out.contains("src=\"cid:a1\"") && out.contains("src=\"cid:p1\""), "{out}");
    }

    #[test]
    fn dates_and_labels_are_translated() {
        for (lang, _) in bf_core::i18n::locales() {
            let t = T::new(lang);
            for month in 1..=12 {
                let at = format!("2026-{month:02}-05T00:00:00Z").parse().unwrap();
                let shown = day(&t, at);
                assert!(shown.contains('5') && !shown.contains('{'), "{lang} {month}: {shown}");
            }
        }
    }
}
