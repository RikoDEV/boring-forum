//! Watched words: block, censor, require approval or replace words in
//! titles and posts. Matching is whole-word and case-insensitive; `*` is a
//! wildcard for any run of word characters (`spam*` matches "spammer").

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Block,
    Censor,
    RequireApproval,
    Replace,
    /// Turns the word into a link to `replacement`.
    Link,
    /// Adds the tag `replacement` to the topic.
    Tag,
    /// The post is published, and reported to the moderators as spam.
    Flag,
    /// The post is held for review and its author silenced.
    Silence,
}

#[derive(Debug, Clone)]
pub struct Rule {
    pub action: Action,
    pub pattern: String,
    pub replacement: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// Text to store (possibly censored or replaced).
    Allow { text: String, needs_approval: bool, tags: Vec<String>, flag: bool, silence: bool },
    /// Rejected; carries the matched word for the error message.
    Blocked(String),
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '\''
}

/// Matches `pattern` (lowercase, may contain `*`) against one lowercase word.
fn glob(pattern: &str, word: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == word,
        Some((head, tail)) => {
            word.len() >= head.len()
                && word.starts_with(head)
                && (0..=word.len() - head.len())
                    .filter(|i| word.is_char_boundary(head.len() + i))
                    .any(|i| glob(tail, &word[head.len() + i..]))
        }
    }
}

/// Splits text into (is_word, slice) runs, keeping every character.
fn runs(text: &str) -> Vec<(bool, &str)> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut cur: Option<bool> = None;
    for (i, c) in text.char_indices() {
        let w = is_word(c);
        if let Some(previous) = cur.filter(|&k| k != w) {
            out.push((previous, &text[start..i]));
            start = i;
        }
        cur = Some(w);
    }
    if let Some(k) = cur {
        out.push((k, &text[start..]));
    }
    out
}

/// How firmly a rule acts, so that when several match one word the strictest wins.
const fn severity(a: Action) -> u8 {
    match a {
        Action::Block => 8,
        Action::Silence => 7,
        Action::Censor => 6,
        Action::RequireApproval => 5,
        Action::Flag => 3,
        Action::Replace => 2,
        Action::Link => 1,
        Action::Tag => 0,
    }
}

pub fn apply(rules: &[Rule], text: &str) -> Outcome {
    let mut needs_approval = false;
    let (mut flag, mut silence) = (false, false);
    let mut tags = Vec::new();
    let mut out = String::with_capacity(text.len());
    for (is_w, run) in runs(text) {
        if !is_w {
            out.push_str(run);
            continue;
        }
        // Quotes around a word ('casino') are punctuation, not part of it.
        let core = run.trim_matches('\'');
        let lead = &run[..run.len() - run.trim_start_matches('\'').len()];
        let trail = &run[lead.len() + core.len()..];
        if core.is_empty() {
            out.push_str(run);
            continue;
        }
        out.push_str(lead);
        let run = core;
        let lower = run.to_lowercase();
        let hit = rules.iter().filter(|r| glob(&r.pattern.to_lowercase(), &lower)).max_by_key(|r| severity(r.action));
        match hit.map(|r| (r.action, r)) {
            Some((Action::Block, _)) => return Outcome::Blocked(run.to_owned()),
            Some((Action::Censor, _)) => out.extend(std::iter::repeat_n('■', run.chars().count())),
            Some((Action::Replace, r)) => out.push_str(r.replacement.as_deref().unwrap_or("")),
            Some((Action::RequireApproval, _)) => {
                needs_approval = true;
                out.push_str(run);
            }
            // Silencing holds the post as well: nothing a silenced member wrote is published unseen.
            Some((Action::Silence, _)) => {
                needs_approval = true;
                silence = true;
                out.push_str(run);
            }
            Some((Action::Flag, _)) => {
                flag = true;
                out.push_str(run);
            }
            Some((Action::Link, r)) => out.push_str(&format!("[{run}]({})", r.replacement.as_deref().unwrap_or(""))),
            Some((Action::Tag, r)) => {
                tags.extend(r.replacement.clone());
                out.push_str(run);
            }
            None => out.push_str(run),
        }
        out.push_str(trail);
    }
    Outcome::Allow { text: out, needs_approval, tags, flag, silence }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(action: Action, pattern: &str, replacement: Option<&str>) -> Rule {
        Rule { action, pattern: pattern.into(), replacement: replacement.map(Into::into) }
    }

    #[test]
    fn whole_words_only() {
        let r = [rule(Action::Censor, "ass", None)];
        assert_eq!(
            apply(&r, "a class ASS."),
            Outcome::Allow { text: "a class ■■■.".into(), needs_approval: false, tags: vec![], flag: false, silence: false }
        );
    }

    #[test]
    fn wildcard_block() {
        let r = [rule(Action::Block, "spam*", None)];
        assert_eq!(apply(&r, "I am a Spammer!"), Outcome::Blocked("Spammer".into()));
        assert!(matches!(apply(&r, "no spa here"), Outcome::Allow { .. }));
    }

    #[test]
    fn replace_and_approval() {
        let r = [rule(Action::Replace, "teh", Some("the")), rule(Action::RequireApproval, "casino", None)];
        assert_eq!(
            apply(&r, "teh best casino"),
            Outcome::Allow { text: "the best casino".into(), needs_approval: true, tags: vec![], flag: false, silence: false }
        );
    }

    #[test]
    fn quoted_word_is_still_blocked() {
        let r = [rule(Action::Block, "casino", None)];
        assert_eq!(apply(&r, "try 'casino' now"), Outcome::Blocked("casino".into()));
        let c = [rule(Action::Censor, "casino", None)];
        assert_eq!(
            apply(&c, "'casino' don't"),
            Outcome::Allow { text: "'■■■■■■' don't".into(), needs_approval: false, tags: vec![], flag: false, silence: false }
        );
    }

    #[test]
    fn strictest_rule_wins() {
        let r = [rule(Action::Censor, "cas*", None), rule(Action::Block, "casino", None)];
        assert_eq!(apply(&r, "casino"), Outcome::Blocked("casino".into()));
    }

    #[test]
    fn unicode_is_kept_intact() {
        let r = [rule(Action::Censor, "zażółć", None)];
        assert_eq!(
            apply(&r, "— Zażółć gęślą"),
            Outcome::Allow { text: "— ■■■■■■ gęślą".into(), needs_approval: false, tags: vec![], flag: false, silence: false }
        );
    }

    #[test]
    fn wildcard_in_middle() {
        assert!(glob("f*k", "fork"));
        assert!(!glob("f*k", "forks"));
        assert!(glob("*ing", "spamming"));
    }

    #[test]
    fn link_and_tag() {
        let r = [rule(Action::Link, "rust", Some("https://rust-lang.org")), rule(Action::Tag, "bug", Some("bugs"))];
        assert_eq!(
            apply(&r, "rust bug"),
            Outcome::Allow {
                text: "[rust](https://rust-lang.org) bug".into(),
                needs_approval: false,
                tags: vec!["bugs".into()],
                flag: false,
                silence: false,
            }
        );
    }

    #[test]
    fn flag_publishes_and_silence_holds() {
        let flag = [rule(Action::Flag, "casino", None)];
        assert_eq!(
            apply(&flag, "visit the Casino"),
            Outcome::Allow {
                text: "visit the Casino".into(),
                needs_approval: false,
                tags: vec![],
                flag: true,
                silence: false
            }
        );
        let both = [rule(Action::Flag, "pills", None), rule(Action::Silence, "pills", None)];
        assert_eq!(
            apply(&both, "cheap pills"),
            Outcome::Allow {
                text: "cheap pills".into(),
                needs_approval: true,
                tags: vec![],
                flag: false,
                silence: true
            }
        );
    }
}
