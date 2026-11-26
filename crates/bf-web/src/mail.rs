//! The HTML version of a mail, made from its plain text as it is sent.
//!
//! The text stays the one source: what an administrator rewrites in Site text, in any language,
//! comes out in the same layout, and nothing that queues a mail knows about HTML. The text is read
//! into a few parts by its shape:
//!
//! - a blank line ends a paragraph;
//! - a line holding only an address **on this site** becomes the button (the first one). An address
//!   elsewhere never does, so a link inside somebody's post cannot pose as the mail's own action;
//! - lines starting `>` are a quote, with the author named by a first line `> @name:`;
//! - `- title (note)` followed by an indented address is a list entry (digests);
//! - two or more `Label: value` lines together are a table of facts;
//! - the `— Site` sign-off is left out, and a line with the mail's unsubscribe link moves to the
//!   footer.
//!
//! Everything is escaped by the template: no text, from a catalog or a member, is ever markup.

mod summary;

pub use summary::{Card, Line, Pictures, Summary, render_summary};

use askama::Template;
use bf_core::i18n::T;

/// How the site's mail looks.
pub struct Brand<'a> {
    pub site: &'a str,
    /// The site's address, without a trailing slash.
    pub base: &'a str,
    /// The button and link colour, `#rrggbb`; anything else falls back to [`ACCENT`].
    pub accent: &'a str,
    /// The logo's address as the mail names it (`cid:logo`, or a `data:` URL in a preview), with
    /// its width and height as shown. Without one the site's name stands in its place.
    pub logo: Option<(&'a str, u32, u32)>,
}

pub const ACCENT: &str = "#0b64c0";

/// A mail as queued.
pub struct Mail<'a> {
    pub subject: &'a str,
    pub body: &'a str,
    pub unsubscribe_url: Option<&'a str>,
}

/// A run of text, or a link within it.
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub text: String,
    pub link: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub title: String,
    pub note: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// Lines of one paragraph.
    Text(Vec<Vec<Span>>),
    /// The heading of the list under it.
    Label(String),
    Button(String),
    Quote {
        author: Option<String>,
        lines: Vec<String>,
    },
    Facts(Vec<(String, String)>),
    List(Vec<Entry>),
}

/// An address on this site, and nothing else on the line.
fn own_address<'a>(line: &'a str, base: &str) -> Option<&'a str> {
    let line = line.trim();
    let rest = line.strip_prefix(base)?;
    let ends_the_host = rest.is_empty() || rest.starts_with(['/', '?', '#']);
    (!base.is_empty() && ends_the_host && !line.contains(char::is_whitespace)).then_some(line)
}

/// Splits a line at its web addresses, so they can be clicked.
fn spans(line: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(start) = ["https://", "http://"].iter().filter_map(|s| rest.find(s)).min() {
        let tail = &rest[start..];
        let end = tail.find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"')).unwrap_or(tail.len());
        // Punctuation after an address belongs to the sentence.
        let url = tail[..end].trim_end_matches(['.', ',', ':', ';', ')', ']', '!', '?', '،']);
        if start > 0 {
            out.push(Span { text: rest[..start].to_owned(), link: false });
        }
        out.push(Span { text: url.to_owned(), link: true });
        rest = &rest[start + url.len()..];
    }
    if !rest.is_empty() {
        out.push(Span { text: rest.to_owned(), link: false });
    }
    out
}

/// `- Title (3 replies)` and, on the next line, its address.
fn list_entry(first: &str, second: Option<&&str>, base: &str) -> Option<Entry> {
    let item = first.strip_prefix("- ")?;
    let second = second?;
    if !second.starts_with("  ") {
        return None;
    }
    let url = own_address(second, base)?.to_owned();
    let (title, note) = match item.strip_suffix(')').and_then(|i| i.rsplit_once(" (")) {
        Some((title, note)) => (title.to_owned(), note.to_owned()),
        None => (item.to_owned(), String::new()),
    };
    Some(Entry { title, note, url })
}

/// `Label: value`, short enough to be a label.
fn fact(line: &str) -> Option<(String, String)> {
    let (label, value) = line.split_once(": ")?;
    let plain = !label.is_empty() && label.chars().count() <= 30 && !label.contains(['.', '?', '!', '>', '/']);
    (plain && !value.trim().is_empty() && !value.starts_with("http"))
        .then(|| (label.to_owned(), value.trim().to_owned()))
}

fn quote(lines: &[&str]) -> Block {
    let mut lines: Vec<String> =
        lines.iter().map(|l| l.trim_start().trim_start_matches('>').trim().to_owned()).collect();
    let author = lines
        .first()
        .and_then(|l| l.strip_prefix('@')?.strip_suffix(':'))
        .filter(|a| !a.contains(char::is_whitespace))
        .map(str::to_owned);
    if author.is_some() {
        lines.remove(0);
    }
    Block::Quote { author, lines }
}

/// The body read into the parts of the layout.
pub fn blocks(body: &str, site: &str, base: &str, unsubscribe_url: Option<&str>) -> Vec<Block> {
    let sign_off = format!("— {site}");
    let lines: Vec<&str> = body
        .lines()
        .map(str::trim_end)
        .filter(|l| !unsubscribe_url.is_some_and(|u| !u.is_empty() && l.contains(u)))
        // The sign-off and the old signature marker: the header and footer name the site.
        .filter(|l| l.trim() != sign_off && l.trim() != "--")
        .collect();
    let mut out = Vec::new();
    let mut buttoned = false;
    for paragraph in lines.split(|l| l.trim().is_empty()) {
        if paragraph.is_empty() {
            continue;
        }
        if paragraph.iter().all(|l| l.trim_start().starts_with('>')) {
            out.push(quote(paragraph));
            continue;
        }
        let facts: Vec<_> = paragraph.iter().filter_map(|l| fact(l)).collect();
        if paragraph.len() >= 2 && facts.len() == paragraph.len() {
            out.push(Block::Facts(facts));
            continue;
        }
        let mut text: Vec<Vec<Span>> = Vec::new();
        let mut list: Vec<Entry> = Vec::new();
        let mut i = 0;
        while i < paragraph.len() {
            let line = paragraph[i];
            if let Some(entry) = list_entry(line, paragraph.get(i + 1), base) {
                if !text.is_empty() {
                    out.push(Block::Text(std::mem::take(&mut text)));
                }
                list.push(entry);
                i += 2;
                continue;
            }
            if !list.is_empty() {
                out.push(Block::List(std::mem::take(&mut list)));
            }
            match own_address(line, base) {
                Some(url) if !buttoned => {
                    if !text.is_empty() {
                        out.push(Block::Text(std::mem::take(&mut text)));
                    }
                    out.push(Block::Button(url.to_owned()));
                    buttoned = true;
                }
                _ => text.push(spans(line)),
            }
            i += 1;
        }
        if !text.is_empty() {
            out.push(Block::Text(text));
        }
        if !list.is_empty() {
            out.push(Block::List(list));
        }
    }
    label_lists(out)
}

/// A one-line paragraph ending in `:` just above a list is that list's heading.
fn label_lists(mut blocks: Vec<Block>) -> Vec<Block> {
    for i in 1..blocks.len() {
        if !matches!(blocks[i], Block::List(_)) {
            continue;
        }
        if let Block::Text(lines) = &blocks[i - 1]
            && let [line] = lines.as_slice()
            && let [span] = line.as_slice()
            && let Some(heading) = span.text.strip_suffix(':')
        {
            blocks[i - 1] = Block::Label(heading.to_owned());
        }
    }
    blocks
}

/// What the button says, from where it leads.
fn button_label(t: &T, url: &str, base: &str) -> String {
    let path = url.strip_prefix(base).unwrap_or(url);
    let starts = |p: &str| path.starts_with(p);
    if starts("/verify/") {
        t.msg("Confirm email address")
    } else if starts("/reset/") {
        t.msg("Choose a new password")
    } else if starts("/login/magic/") {
        t.msg("Log in to your account")
    } else if starts("/confirm-email/") {
        t.msg("Confirm the new address")
    } else if starts("/settings") {
        t.msg("Review account security")
    } else if starts("/forgot") {
        t.msg("Reset your password")
    } else if starts("/t/") || starts("/p/") {
        t.msg("Open the topic")
    } else if starts("/messages") {
        t.msg("Open your messages")
    } else if starts("/notifications") {
        t.msg("See your notifications")
    } else if starts("/review") {
        t.msg("Open the review queue")
    } else if path.contains("/backups") {
        t.msg("Download the backup")
    } else if path.is_empty() || path == "/" {
        t.msg("Visit the site")
    } else {
        t.msg("Open the page")
    }
}

/// `ar` when the text is mostly Arabic, whatever language the recipient picked: the layout must
/// run the way the words do. Only the mail's own wording counts: quotes, lists of topic titles
/// and addresses are in whatever language their writers used.
pub fn language<'a>(body: &str, preferred: &'a str) -> &'a str {
    let letters = || {
        body.lines()
            .filter(|l| !l.starts_with('>') && !l.starts_with("- ") && !l.contains("://"))
            .flat_map(str::chars)
            .filter(|c| c.is_alphabetic())
    };
    let arabic = letters().filter(|c| ('\u{0600}'..='\u{06FF}').contains(c)).count();
    if arabic * 2 > letters().count() {
        "ar"
    } else if preferred == "ar" {
        "en"
    } else {
        preferred
    }
}

fn rgb(colour: &str) -> Option<[u8; 3]> {
    let hex = colour.strip_prefix('#').filter(|h| h.len() == 6 && h.bytes().all(|b| b.is_ascii_hexdigit()))?;
    let part = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some([part(0)?, part(2)?, part(4)?])
}

/// (accent, the text that reads on it, the accent as a link on a dark ground).
fn colours(accent: &str) -> (String, &'static str, String) {
    let (accent, [r, g, b]) = match rgb(accent) {
        Some(parts) => (accent.to_owned(), parts),
        None => (ACCENT.to_owned(), [0x0b, 0x64, 0xc0]),
    };
    // Perceived brightness, to pick black or white text.
    let bright = u32::from(r) * 299 + u32::from(g) * 587 + u32::from(b) * 114 > 150_000;
    let lift = |c: u8| c + ((255 - u16::from(c)) / 2) as u8;
    let dark_link = format!("#{:02x}{:02x}{:02x}", lift(r), lift(g), lift(b));
    (accent, if bright { "#10151c" } else { "#ffffff" }, dark_link)
}

/// The colour behind a quoted author's initial, the same for a name every time.
fn initial(name: &str) -> (String, &'static str) {
    const PALETTE: [&str; 8] = ["#2f5d8a", "#6b4c9a", "#a23b5f", "#9a5b13", "#2d7a4f", "#3d6b7a", "#7a4a3d", "#55606e"];
    let hash = name.to_lowercase().bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
    let letter = name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect()).unwrap_or_default();
    (letter, PALETTE[(hash as usize) % PALETTE.len()])
}

/// The first words an inbox shows beside the subject: the quote, or the first sentence that is
/// not a greeting.
fn preview_line(blocks: &[Block]) -> String {
    let text = blocks.iter().find_map(|b| match b {
        Block::Quote { lines, .. } => Some(lines.join(" ")),
        Block::Text(lines) => {
            let joined = lines.iter().flatten().filter(|s| !s.link).map(|s| s.text.as_str()).collect::<String>();
            (!joined.trim_end().ends_with([',', '،'])).then_some(joined)
        }
        _ => None,
    });
    text.unwrap_or_default().chars().take(140).collect()
}

/// A paragraph that only says the heading again, as notification mail does before its link.
fn repeats(block: &Block, heading: &str) -> bool {
    let Block::Text(lines) = block else { return false };
    let said: String = lines.iter().flatten().map(|s| s.text.as_str()).collect();
    said.trim_end_matches(':') == heading
}

struct Part {
    block: Block,
    /// The button's label.
    label: String,
    initial: (String, &'static str),
}

#[derive(Template)]
#[template(path = "mail.html")]
struct Page<'a> {
    lang: &'a str,
    dir: &'static str,
    /// Where text starts: `left`, or `right` for right-to-left languages (Outlook needs it said).
    start: &'static str,
    site: &'a str,
    base: &'a str,
    logo: Option<(&'a str, u32, u32)>,
    accent: String,
    on_accent: &'static str,
    dark_link: String,
    /// Previews only: the dark colours whatever the viewer's setting.
    force_dark: bool,
    preview: String,
    heading: &'a str,
    parts: Vec<Part>,
    fallback: Option<(String, String)>,
    reason: String,
    /// (label, address) of the footer's links.
    links: Vec<(String, String)>,
}

/// Why the reader got the mail, and the footer's links as (label, address). A mail with no
/// unsubscribe link is a notice: there is nothing to switch off.
fn footer(t: &T, brand: &Brand, mail: &Mail) -> (String, Vec<(String, String)>) {
    let mut links = Vec::new();
    if let Some(url) = mail.unsubscribe_url.filter(|u| !u.is_empty()) {
        links.push((t.msg("Email settings"), format!("{}/settings#s-notifications", brand.base)));
        let mute = format!("{url}/topic");
        if mail.body.contains(&mute) {
            links.push((t.msg("Mute this topic"), mute));
        }
        links.push((t.msg("Unsubscribe"), url.to_owned()));
    }
    let site = [("site", brand.site)];
    let reason = if links.is_empty() {
        t.msgf("This email was sent by {site}.", &site)
    } else {
        t.msgf("You get this because of your email settings on {site}.", &site)
    };
    (reason, links)
}

/// The HTML version of `mail`, with labels in `lang`.
pub fn render(brand: &Brand, mail: &Mail, lang: &str, force_dark: bool) -> Result<String, askama::Error> {
    let lang = language(mail.body, lang);
    let t = T::new(lang);
    let found = blocks(mail.body, brand.site, brand.base, mail.unsubscribe_url);
    let suffix = format!(" - {}", brand.site);
    let heading = mail.subject.strip_suffix(&suffix).unwrap_or(mail.subject);
    // A client that blocks buttons still shows the address, for the links that cannot be done
    // without (they hold a one-time token, so there is no other way to the page).
    let fallback = found.iter().find_map(|b| match b {
        Block::Button(url)
            if ["/verify/", "/reset/", "/login/magic/", "/confirm-email/", "/backups"]
                .iter()
                .any(|p| url.contains(p)) =>
        {
            Some((t.msg("Button not working? Copy this address:"), url.clone()))
        }
        _ => None,
    });
    let (reason, links) = footer(&t, brand, mail);
    // The inbox shows this beside the subject: it must not be the subject again.
    let found: Vec<Block> = found.into_iter().filter(|b| !repeats(b, heading)).collect();
    let preview = preview_line(&found);
    let parts = found
        .into_iter()
        .map(|block| {
            let label = match &block {
                Block::Button(url) => button_label(&t, url, brand.base),
                _ => String::new(),
            };
            let initial = match &block {
                Block::Quote { author: Some(a), .. } => initial(a),
                _ => (String::new(), ""),
            };
            Part { block, label, initial }
        })
        .collect();
    let (accent, on_accent, dark_link) = colours(brand.accent);
    let rtl = lang == "ar";
    Page {
        lang,
        dir: if rtl { "rtl" } else { "ltr" },
        start: if rtl { "right" } else { "left" },
        site: brand.site,
        base: brand.base,
        logo: brand.logo,
        accent,
        on_accent,
        dark_link,
        force_dark,
        preview,
        heading,
        parts,
        fallback,
        reason,
        links,
    }
    .render()
}

/// `body` with a quote of a post set before the line that links to it, as notification mail
/// carries an excerpt. `text` is plain text; it is kept to one quoted line.
pub fn with_quote(body: &str, link: &str, author: &str, text: &str) -> String {
    if text.is_empty() {
        return body.to_owned();
    }
    let quote = format!("> @{author}:\n> {text}");
    let mut lines: Vec<&str> = body.lines().collect();
    let at = lines.iter().position(|l| l.trim() == link).unwrap_or(lines.len());
    lines.insert(at, "");
    lines.insert(at, &quote);
    let mut out = lines.join("\n");
    if body.ends_with('\n') {
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "https://f.test";

    fn brand() -> Brand<'static> {
        Brand { site: "Forum", base: BASE, accent: ACCENT, logo: None }
    }

    fn html(body: &str, unsubscribe: Option<&str>, lang: &str) -> String {
        let mail = Mail { subject: "Hello - Forum", body, unsubscribe_url: unsubscribe };
        render(&brand(), &mail, lang, false).unwrap()
    }

    #[test]
    fn only_an_address_on_this_site_becomes_the_button() {
        let found = blocks(
            "A post:\n\nhttps://evil.test/reset/x\n\nhttps://f.test.evil.test/a\n\nhttps://f.test/t/a/1\n\nhttps://f.test/t/b/2",
            "Forum",
            BASE,
            None,
        );
        let buttons: Vec<_> = found.iter().filter(|b| matches!(b, Block::Button(_))).collect();
        assert_eq!(buttons, [&Block::Button("https://f.test/t/a/1".into())]);
    }

    #[test]
    fn nothing_in_the_text_is_markup() {
        let out = html(
            "Hi <img src=x onerror=a()>,\n\n> @<b>:\n> <script>x</script>\n\nsee https://x.test/\"><i>",
            None,
            "en",
        );
        for markup in ["<img src=x", "<script>", "<b>:", "\"><i>"] {
            assert!(!out.contains(markup), "{markup}\n{out}");
        }
        assert!(out.contains("&#60;script&#62;") || out.contains("&lt;script&gt;"), "{out}");
    }

    #[test]
    fn the_sign_off_goes_and_the_unsubscribe_lines_move_to_the_footer() {
        let url = "https://f.test/unsubscribe/abc";
        let body = format!("Hi,\n\nNews.\n\n— Forum\n\nStop: {url}\nMute: {url}/topic");
        let found = blocks(&body, "Forum", BASE, Some(url));
        assert_eq!(found.len(), 2, "{found:?}");
        let out = html(&body, Some(url), "en");
        assert!(!out.contains("— Forum") && !out.contains("Stop: "), "{out}");
        assert!(out.contains(&format!("href=\"{url}\"")) && out.contains(&format!("href=\"{url}/topic\"")));
        // An account notice has no link to stop it.
        assert!(!html("Hi,\n\nNews.", None, "en").contains("Unsubscribe"));
    }

    #[test]
    fn quotes_facts_and_digest_lists_are_recognised() {
        let body = "New topics:\n\n- Best (really) settings? (4 replies)\n  https://f.test/t/best/1\n- Other (1 reply)\n  https://f.test/t/other/2\n\n> @ann:\n> Hello there\n\nWhen: today\nIP address: 203.0.113.9";
        let found = blocks(body, "Forum", BASE, None);
        assert_eq!(found[0], Block::Label("New topics".into()));
        let Block::List(items) = &found[1] else { panic!("{found:?}") };
        assert_eq!(items[0].title, "Best (really) settings?");
        assert_eq!((items[1].note.as_str(), items[1].url.as_str()), ("1 reply", "https://f.test/t/other/2"));
        assert_eq!(found[2], Block::Quote { author: Some("ann".into()), lines: vec!["Hello there".into()] });
        assert_eq!(
            found[3],
            Block::Facts(vec![("When".into(), "today".into()), ("IP address".into(), "203.0.113.9".into())])
        );
        // One such line is a sentence, not a table.
        assert!(matches!(blocks("Reason: spam", "Forum", BASE, None)[0], Block::Text(_)));
    }

    #[test]
    fn the_layout_runs_the_way_the_words_do() {
        assert!(html("مرحبًا بكم في المنتدى", None, "ar").contains("dir=\"rtl\""));
        assert!(html("مرحبًا بكم في المنتدى", None, "en").contains("dir=\"rtl\""));
        assert!(html("Welcome to the forum", None, "ar").contains("dir=\"ltr\""));
        // English titles, quotes and addresses do not turn an Arabic mail around.
        let mixed = "مرحبًا بكم\n\n- A very long English topic title about servers (3)\n  https://f.test/t/a-long-english-slug/1\n\n> @ann:\n> a long quote written in English words\n\nhttps://f.test/notifications";
        assert!(html(mixed, None, "ar").contains("dir=\"rtl\""));
    }

    #[test]
    fn a_colour_that_is_not_one_falls_back() {
        assert_eq!(colours("red;}body{display:none").0, ACCENT);
        assert_eq!(colours("#ffee88").1, "#10151c");
        assert_eq!(colours("#0b64c0").1, "#ffffff");
    }

    #[test]
    fn a_paragraph_repeating_the_heading_is_shown_once() {
        let mail = Mail { subject: "Ann replied - Forum", body: "Ann replied:\n\nMore.", unsubscribe_url: None };
        let out = render(&brand(), &mail, "en", false).unwrap();
        // Once in the title, once as the heading.
        assert!(!out.contains(">Ann replied:<") && out.contains(">More.<"), "{out}");
    }

    #[test]
    fn a_quote_sits_before_the_link() {
        let body = "Hi,\n\nann posted:\n\nhttps://f.test/t/a/1\n\n— Forum\n";
        assert_eq!(
            with_quote(body, "https://f.test/t/a/1", "ann", "Text"),
            "Hi,\n\nann posted:\n\n> @ann:\n> Text\n\nhttps://f.test/t/a/1\n\n— Forum\n"
        );
        assert_eq!(with_quote(body, "https://f.test/t/a/1", "ann", ""), body);
    }

    #[test]
    fn every_catalog_mail_gets_its_layout_in_every_language() {
        let vars = [
            ("site", "Forum"),
            ("username", "jane"),
            ("by", "ann"),
            ("title", "A topic"),
            ("what", "Your password was changed"),
            ("name", "backup.tar"),
            ("topics", "New topics:\n\n- A topic (1 reply)\n  https://f.test/t/a/1\n"),
            ("link", "https://f.test/reset/tok"),
            ("unsubscribe", "https://f.test/unsubscribe/u"),
        ];
        for (lang, _) in bf_core::i18n::locales() {
            let t = T::new(lang);
            for (kind, _, required) in bf_core::mail::TEMPLATE_KINDS {
                let subject = t.f(&format!("mail.{kind}.subject"), &vars);
                let body = t.f(&format!("mail.{kind}.body"), &vars);
                let unsubscribe = (*kind == "digest").then_some("https://f.test/unsubscribe/u");
                let mail = Mail { subject: &subject, body: &body, unsubscribe_url: unsubscribe };
                let out = render(&brand(), &mail, lang, false).unwrap();
                assert!(out.contains("<html") && !out.contains("— Forum<"), "{lang} {kind}");
                if *required == "link" {
                    assert!(out.contains("href=\"https://f.test/reset/tok\""), "{lang} {kind} lost its link\n{body}");
                }
                if *kind == "digest" {
                    assert!(out.contains("href=\"https://f.test/t/a/1\""), "{lang} digest lost its list");
                }
            }
        }
    }
}
