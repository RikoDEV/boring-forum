//! Which links in a post carry `rel="nofollow ugc"` (ACP → Settings → Posts and topics).
//!
//! The sanitizer puts `nofollow ugc noopener` on every link when a post is cooked, and the cooked
//! text is stored. This relaxes it when the page is built, so a change of setting applies to every
//! post at once, with no recook. A link is *followed* (it keeps `noopener` only) when:
//!
//! - links are set to `none` (`nofollow_links`), or
//! - they are set to `external` and the link points at this site, or
//! - its post's author is at or above `nofollow_exempt_trust_level`, or
//! - its site is in `nofollow_exempt_domains` (the domain and its subdomains).
//!
//! `noopener` stays on every link: it is about safety, not ranking.

use bf_core::settings::Settings;
use std::borrow::Cow;

const STOCK: &str = "rel=\"nofollow ugc noopener\"";
const FOLLOWED: &str = "rel=\"noopener\"";

#[derive(Debug, PartialEq, Eq)]
enum Mode {
    All,
    External,
    None,
}

pub(crate) struct Policy {
    mode: Mode,
    /// Authors at or above this trust level have their links followed; above 4 nobody does.
    exempt_level: i64,
    domains: Vec<String>,
    own_host: String,
}

/// The sites in a setting: whitespace or commas between them, each a domain or an address, with
/// or without `www.` or `*.`.
fn domains(text: &str) -> Vec<String> {
    text.split(|c: char| c.is_whitespace() || c == ',')
        .filter(|t| !t.is_empty())
        .filter_map(|t| {
            let t = t.trim_start_matches("*.");
            let host = if t.contains("://") {
                url::Url::parse(t).ok()?.host_str()?.to_owned()
            } else {
                t.split(['/', '?', '#', ':']).next()?.to_owned()
            };
            let host = host.to_ascii_lowercase();
            let host = host.trim_start_matches("www.").trim_end_matches('.').to_owned();
            (host.contains('.') || host == "localhost").then_some(host)
        })
        .collect()
}

fn host_of(link: &str) -> Option<String> {
    let full = if let Some(rest) = link.strip_prefix("//") { format!("https://{rest}") } else { link.to_owned() };
    let url = url::Url::parse(&full).ok()?;
    matches!(url.scheme(), "http" | "https").then(|| url.host_str().map(str::to_ascii_lowercase))?
}

impl Policy {
    pub(crate) fn from_settings(s: &Settings, site_url: &str) -> Policy {
        Policy {
            mode: match s.str("nofollow_links") {
                "external" => Mode::External,
                "none" => Mode::None,
                _ => Mode::All,
            },
            exempt_level: s.int("nofollow_exempt_trust_level"),
            domains: domains(s.str("nofollow_exempt_domains")),
            own_host: url::Url::parse(site_url)
                .ok()
                .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
                .unwrap_or_default(),
        }
    }

    /// Nothing changes: every link keeps nofollow.
    fn is_stock(&self) -> bool {
        self.mode == Mode::All && self.exempt_level > 4 && self.domains.is_empty()
    }

    fn is_own(&self, host: &str) -> bool {
        !self.own_host.is_empty()
            && (host == self.own_host || host.trim_start_matches("www.") == self.own_host.trim_start_matches("www."))
    }

    /// Whether this link, in a post by an author of this trust level, is followed.
    fn follows(&self, href: &str, author_level: Option<i64>) -> bool {
        if self.mode == Mode::None || author_level.is_some_and(|l| l >= self.exempt_level) {
            return true;
        }
        let host = host_of(href);
        match &host {
            // A link on this page or site: no address, or ours.
            None if href.starts_with('/') || href.starts_with('#') => return self.mode == Mode::External,
            Some(h) if self.is_own(h) => return self.mode == Mode::External,
            _ => {}
        }
        host.is_some_and(|h| {
            let h = h.trim_start_matches("www.");
            self.domains.iter().any(|d| h == d || h.ends_with(&format!(".{d}")))
        })
    }

    /// `html` (a cooked post) with the links this policy follows relaxed.
    pub(crate) fn apply<'a>(&self, html: &'a str, author_level: Option<i64>) -> Cow<'a, str> {
        if self.is_stock() || !html.contains(STOCK) {
            return Cow::Borrowed(html);
        }
        let mut out = String::with_capacity(html.len());
        let mut rest = html;
        let mut changed = false;
        while let Some(at) = rest.find("<a ") {
            let end = rest[at..].find('>').map_or(rest.len(), |e| at + e);
            let tag = &rest[at..end];
            out.push_str(&rest[..at]);
            let href = tag.find(" href=\"").map(|i| i + 7).and_then(|i| tag[i..].find('"').map(|e| &tag[i..i + e]));
            match href {
                Some(h) if tag.contains(STOCK) && self.follows(&h.replace("&amp;", "&"), author_level) => {
                    out.push_str(&tag.replacen(STOCK, FOLLOWED, 1));
                    changed = true;
                }
                _ => out.push_str(tag),
            }
            rest = &rest[end..];
        }
        out.push_str(rest);
        if changed { Cow::Owned(out) } else { Cow::Borrowed(html) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(mode: Mode, level: i64, domains_text: &str) -> Policy {
        Policy { mode, exempt_level: level, domains: domains(domains_text), own_host: "forum.example".into() }
    }

    const PAGE: &str = r#"<p><a href="https://forum.example/t/1" rel="nofollow ugc noopener">ours</a> <a href="/u/alice" rel="nofollow ugc noopener">@alice</a> <a href="https://good.example/x" rel="nofollow ugc noopener">good</a> <a href="https://sub.good.example/x" rel="nofollow ugc noopener">sub</a> <a href="https://evilgood.example/x" rel="nofollow ugc noopener">other</a> <a href="mailto:a@b.c" rel="nofollow ugc noopener">mail</a></p>"#;

    fn followed(p: &Policy, level: Option<i64>) -> Vec<&'static str> {
        let out = p.apply(PAGE, level).into_owned();
        [">ours<", ">@alice<", ">good<", ">sub<", ">other<", ">mail<"]
            .into_iter()
            .filter(|label| {
                let i = out.find(label).unwrap();
                out[..i].rfind("<a ").map(|a| &out[a..i]).is_some_and(|tag| tag.contains(FOLLOWED))
            })
            .collect()
    }

    #[test]
    fn the_stock_setting_changes_nothing() {
        let p = policy(Mode::All, 5, "");
        assert!(matches!(p.apply(PAGE, Some(4)), Cow::Borrowed(_)));
    }

    #[test]
    fn external_only_follows_links_inside_the_site() {
        assert_eq!(followed(&policy(Mode::External, 5, ""), None), vec![">ours<", ">@alice<"]);
    }

    #[test]
    fn none_follows_every_web_link_and_all_keeps_the_listed_sites_apart() {
        assert_eq!(followed(&policy(Mode::None, 5, ""), None).len(), 6);
        // Listed sites: the domain and its subdomains, not a name that merely ends the same.
        assert_eq!(
            followed(&policy(Mode::All, 5, "https://www.good.example/, other.test"), None),
            vec![">good<", ">sub<"]
        );
        assert_eq!(followed(&policy(Mode::All, 5, "*.GOOD.example"), None), vec![">good<", ">sub<"]);
    }

    #[test]
    fn a_trusted_author_has_their_links_followed() {
        let p = policy(Mode::All, 3, "");
        assert!(followed(&p, Some(2)).is_empty());
        assert_eq!(followed(&p, Some(3)).len(), 6);
        assert!(followed(&p, None).is_empty(), "an unknown author is not trusted");
        // Level 5 is above every level there is.
        assert!(followed(&policy(Mode::All, 5, ""), Some(4)).is_empty());
    }

    #[test]
    fn domains_are_read_from_a_loose_list() {
        assert_eq!(
            domains("a.example, https://B.example/x\n*.c.example www.d.example nonsense"),
            vec!["a.example", "b.example", "c.example", "d.example"]
        );
    }
}
