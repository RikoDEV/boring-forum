//! Advanced search syntax. Everything that is not an operator is the text query.
//!
//! | operator | meaning |
//! | --- | --- |
//! | `@user`, `author:user` | posts by a member |
//! | `#forum`, `forum:name`, `category:name` | inside a forum or category (and what is below it) |
//! | `tags:a,b`, `tag:a` | has one of these tags; `-tag:a` leaves a tag out |
//! | `before:2026-01-01`, `after:7d`, `before:2w` | dates, or relative (days, weeks, months, years) |
//! | `status:closed`, `is:open` | topic state |
//! | `is:solved`, `is:unsolved` | has an accepted answer, or not |
//! | `in:first`, `in:mine`, `in:bookmarks`, `in:title` | where to look |
//! | `has:image`, `has:link`, `has:poll`, `has:code`, `has:attachment` | the post has one |
//! | `likes:3` | at least that many likes |
//! | `replies:>5`, `replies:<5`, `replies:5` | topic reply count |
//! | `order:latest`, `order:oldest`, `order:likes`, `order:replies` | sort (default: relevance) |
//!
//! A word inside double quotes is never an operator. An unknown operator stays a plain word.

#[derive(Debug, Default, PartialEq)]
pub struct Parsed {
    pub text: String,
    pub author: Option<String>,
    pub forum: Option<String>,
    pub tags: Vec<String>,
    /// `-tag:x`: topics with one of these tags are left out.
    pub not_tags: Vec<String>,
    /// A `YYYY-MM-DD` date or a relative one such as `7d`; see [`relative_days`].
    pub before: Option<String>,
    pub after: Option<String>,
    pub closed: Option<bool>,
    pub first_only: bool,
    pub mine: bool,
    pub bookmarks: bool,
    pub min_likes: Option<i32>,
    /// `has:` values, each of `image`, `link`, `poll`, `code`, `attachment`, once.
    pub has: Vec<String>,
    /// `is:solved` (true) or `is:unsolved` (false).
    pub solved: Option<bool>,
    pub min_replies: Option<i32>,
    pub max_replies: Option<i32>,
    /// `order:` as `newest`, `oldest`, `likes` or `replies`.
    pub order: Option<&'static str>,
    /// `in:title`: only topic titles are searched.
    pub title_only: bool,
}

/// Words of `q`, with whether each is inside double quotes. A quote opens at the start of a
/// word (after an optional `-`) and closes at the end of a later one; an unclosed quote runs to
/// the end.
fn words(q: &str) -> Vec<(&str, bool)> {
    let mut out = Vec::new();
    let mut quoted = false;
    for w in q.split_whitespace() {
        let opens = !quoted && (w.starts_with('"') || w.starts_with("-\""));
        if opens {
            quoted = true;
        }
        out.push((w, quoted));
        if quoted {
            let body = if opens { w.trim_start_matches('-').strip_prefix('"').unwrap_or(w) } else { w };
            if body.ends_with('"') {
                quoted = false;
            }
        }
    }
    out
}

pub fn parse(q: &str) -> Parsed {
    let mut p = Parsed::default();
    let mut text = Vec::new();
    for (word, quoted) in words(q) {
        if quoted {
            text.push(word);
            continue;
        }
        if let Some(u) = word.strip_prefix('@').filter(|u| !u.is_empty()) {
            p.author = Some(u.to_owned());
        } else if let Some(f) = word.strip_prefix('#').filter(|f| !f.is_empty()) {
            p.forum = Some(f.to_lowercase());
        } else if let Some((k, v)) = word.split_once(':').filter(|(k, v)| !v.is_empty() && !k.is_empty()) {
            let (neg, key) = k.strip_prefix('-').map_or((false, k), |k| (true, k));
            if !operator(&mut p, neg, &key.to_lowercase(), v) {
                text.push(word);
            }
        } else {
            text.push(word);
        }
    }
    p.text = text.join(" ");
    p
}

/// Applies one `key:value`; false when it is not an operator (it stays text).
fn operator(p: &mut Parsed, neg: bool, k: &str, v: &str) -> bool {
    let lv = v.to_lowercase();
    if neg {
        return match k {
            "tag" | "tags" => {
                p.not_tags.extend(lv.split(',').filter(|t| !t.is_empty()).map(str::to_owned));
                true
            }
            _ => false,
        };
    }
    match k {
        "tags" | "tag" => p.tags.extend(lv.split(',').filter(|t| !t.is_empty()).map(str::to_owned)),
        "author" | "from" | "user" => p.author = Some(v.trim_start_matches('@').to_owned()),
        "forum" | "category" | "cat" => p.forum = Some(lv),
        "before" if is_date(v) || relative_days(v).is_some() => p.before = Some(v.to_owned()),
        "after" if is_date(v) || relative_days(v).is_some() => p.after = Some(v.to_owned()),
        "status" | "is" if lv == "closed" => p.closed = Some(true),
        "status" | "is" if lv == "open" => p.closed = Some(false),
        "is" if lv == "solved" => p.solved = Some(true),
        "is" if lv == "unsolved" => p.solved = Some(false),
        "in" if lv == "first" => p.first_only = true,
        "in" if lv == "mine" => p.mine = true,
        "in" if lv == "bookmarks" => p.bookmarks = true,
        "in" if lv == "title" => p.title_only = true,
        "has" if matches!(lv.as_str(), "image" | "link" | "poll" | "code" | "attachment") => {
            if !p.has.contains(&lv) {
                p.has.push(lv);
            }
        }
        "likes" if v.parse::<i32>().is_ok_and(|n| n >= 0) => p.min_likes = v.parse().ok(),
        "replies" => {
            if let Some(n) = v.strip_prefix('>').and_then(count) {
                p.min_replies = Some(n.saturating_add(1));
            } else if let Some(n) = v.strip_prefix('<').and_then(count) {
                p.max_replies = Some(n.saturating_sub(1));
            } else if let Some(n) = count(v) {
                p.min_replies = Some(n);
                p.max_replies = Some(n);
            } else {
                return false;
            }
        }
        "order" | "sort" => {
            p.order = match lv.as_str() {
                "latest" | "newest" | "new" => Some("newest"),
                "oldest" | "old" => Some("oldest"),
                "likes" | "liked" => Some("likes"),
                "replies" | "active" => Some("replies"),
                _ => return false,
            }
        }
        _ => return false,
    }
    true
}

fn count(v: &str) -> Option<i32> {
    v.parse::<i32>().ok().filter(|n| (0..=1_000_000).contains(n))
}

/// A plain `YYYY-MM-DD`; anything else stays ordinary search text.
fn is_date(v: &str) -> bool {
    v.len() == 10
        && v.bytes().enumerate().all(|(i, b)| match i {
            4 | 7 => b == b'-',
            _ => b.is_ascii_digit(),
        })
}

/// `7d`, `2w`, `3m`, `1y`: how many days back (a month is 30 days, a year 365), at most ten years.
pub fn relative_days(v: &str) -> Option<i64> {
    let unit = v.chars().last()?;
    let digits = &v[..v.len() - unit.len_utf8()];
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: i64 = digits.parse().ok().filter(|n| (0..=3650).contains(n))?;
    let days = n * match unit.to_ascii_lowercase() {
        'd' => 1,
        'w' => 7,
        'm' => 30,
        'y' => 365,
        _ => return None,
    };
    (days <= 3650).then_some(days)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_operators_from_text() {
        let p = parse("rust @alice #general tags:a,B before:2026-01-01 status:closed in:first likes:3 hello");
        assert_eq!(p.text, "rust hello");
        assert_eq!(p.author.as_deref(), Some("alice"));
        assert_eq!(p.forum.as_deref(), Some("general"));
        assert_eq!(p.tags, ["a", "b"]);
        assert_eq!(p.before.as_deref(), Some("2026-01-01"));
        assert_eq!(p.closed, Some(true));
        assert!(p.first_only);
        assert_eq!(p.min_likes, Some(3));
    }

    #[test]
    fn unknown_operators_stay_text() {
        assert_eq!(parse("foo:bar http://x.example").text, "foo:bar http://x.example");
        assert_eq!(parse("likes:many").text, "likes:many");
        assert_eq!(
            parse("has:nothing replies:lots order:random after:soon").text,
            "has:nothing replies:lots order:random after:soon"
        );
        assert_eq!(parse("-foo:bar").text, "-foo:bar");
    }

    #[test]
    fn has_and_solved() {
        let p = parse("has:image has:Link has:image is:solved cats");
        assert_eq!(p.has, ["image", "link"]);
        assert_eq!(p.solved, Some(true));
        assert_eq!(p.text, "cats");
        assert_eq!(parse("is:unsolved").solved, Some(false));
        assert_eq!(parse("is:open").closed, Some(false));
    }

    #[test]
    fn replies_ranges() {
        let p = parse("replies:>5");
        assert_eq!((p.min_replies, p.max_replies), (Some(6), None));
        let p = parse("replies:<3");
        assert_eq!((p.min_replies, p.max_replies), (None, Some(2)));
        let p = parse("replies:4");
        assert_eq!((p.min_replies, p.max_replies), (Some(4), Some(4)));
    }

    #[test]
    fn order_and_title() {
        assert_eq!(parse("order:latest x").order, Some("newest"));
        assert_eq!(parse("order:likes").order, Some("likes"));
        let p = parse("in:title cats");
        assert!(p.title_only);
        assert_eq!(p.text, "cats");
    }

    #[test]
    fn relative_dates() {
        assert_eq!(relative_days("7d"), Some(7));
        assert_eq!(relative_days("2w"), Some(14));
        assert_eq!(relative_days("3m"), Some(90));
        assert_eq!(relative_days("1y"), Some(365));
        assert_eq!(relative_days("d"), None);
        assert_eq!(relative_days("7x"), None);
        assert_eq!(relative_days("-3d"), None);
        assert_eq!(relative_days("99999d"), None);
        let p = parse("after:7d before:2w");
        assert_eq!(p.after.as_deref(), Some("7d"));
        assert_eq!(p.before.as_deref(), Some("2w"));
        assert_eq!(parse("after:7q").text, "after:7q");
    }

    #[test]
    fn exclusions() {
        let p = parse("-tag:Spam -tags:a,b rust -java");
        assert_eq!(p.not_tags, ["spam", "a", "b"]);
        assert_eq!(p.text, "rust -java");
    }

    #[test]
    fn forum_and_category() {
        assert_eq!(parse("forum:General x").forum.as_deref(), Some("general"));
        assert_eq!(parse("category:news").forum.as_deref(), Some("news"));
        assert_eq!(parse("author:bob").author.as_deref(), Some("bob"));
    }

    #[test]
    fn quoted_phrases_keep_their_words() {
        let p = parse("\"in:title tag:x\" real @bob");
        assert_eq!(p.text, "\"in:title tag:x\" real");
        assert!(!p.title_only && p.tags.is_empty());
        assert_eq!(p.author.as_deref(), Some("bob"));
        let p = parse("\"hello world\" has:code");
        assert_eq!(p.text, "\"hello world\"");
        assert_eq!(p.has, ["code"]);
        let p = parse("-\"bad phrase\" ok");
        assert_eq!(p.text, "-\"bad phrase\" ok");
        let p = parse("\"one\" in:title");
        assert!(p.title_only);
        let p = parse("\"unclosed in:title");
        assert!(!p.title_only);
    }
}
