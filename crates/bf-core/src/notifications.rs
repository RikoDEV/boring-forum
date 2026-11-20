//! The notification catalogue: one place for each kind's label, icon, tab and default channels,
//! plus its text in the reader's language.

use crate::i18n::T;
use serde_json::Value;

/// Channel bits, as stored in `notification_prefs.channels`.
pub const APP: i16 = 1;
pub const EMAIL: i16 = 2;
/// Browser push, on unless the member turned it off for the kind.
pub const PUSH: i16 = 4;

#[derive(Debug, Clone, Copy)]
pub struct Kind {
    pub key: &'static str,
    /// Shown in the settings table. Empty for kinds a member cannot switch off.
    pub label: &'static str,
    pub icon: &'static str,
    /// The tab on the notifications page, besides "All".
    pub tab: &'static str,
    /// Emails by default. Everything else starts in-app only, so new kinds never surprise anyone.
    pub email: bool,
    /// About the member's own account: ignores preferences and reaches suspended members.
    pub mandatory: bool,
}

const fn kind(key: &'static str, label: &'static str, icon: &'static str, tab: &'static str, email: bool) -> Kind {
    Kind { key, label, icon, tab, email, mandatory: false }
}

const fn account(key: &'static str, icon: &'static str) -> Kind {
    Kind { key, label: "", icon, tab: "staff", email: true, mandatory: true }
}

pub const KINDS: &[Kind] = &[
    kind("mention", "Someone mentions you", "user", "mentions", true),
    kind("direct_reply", "Someone replies to your post", "reply", "replies", true),
    kind("quote", "Someone quotes your post", "quote", "replies", true),
    kind("reply", "New posts in topics you watch", "reply", "replies", false),
    kind("topic_invite", "Someone invites you to a topic", "mail", "mentions", true),
    kind("linked", "Someone links to your topic", "link", "replies", false),
    kind("like", "Someone likes your post", "heart", "likes", false),
    kind("message", "Private messages", "mail", "messages", true),
    kind("chat_message", "Direct chat messages", "message-square", "messages", false),
    kind("chat_mention", "Someone mentions you in chat", "message-square", "mentions", false),
    kind("chat_added", "Someone adds you to a chat channel", "message-square", "mentions", false),
    kind("follower", "Someone follows you", "user", "social", false),
    kind("followed_post", "Someone you follow posts", "user", "social", false),
    kind("status_reply", "Replies to your status", "reply", "social", false),
    kind("badge", "You earn a badge", "trophy", "social", false),
    kind("trust_level", "Your trust level goes up", "circle-check", "social", false),
    kind("trust_near", "You are close to the next trust level", "circle-check", "social", false),
    kind("trust_lowered", "Your trust level goes down", "circle-check", "social", false),
    kind("group_added", "You join a group", "user", "social", false),
    kind("invite_accepted", "Someone accepts your invitation", "user", "social", false),
    kind("club_invite", "Club invitations", "user", "social", false),
    kind("club_request", "Requests to join your club", "user", "social", false),
    kind("group_request_declined", "Your request to join a group is declined", "user", "social", false),
    kind("poll_vote", "Votes on your poll", "list", "social", false),
    kind("poll_closed", "A poll you voted in closes", "list", "social", false),
    kind("event_reminder", "Calendar events tomorrow", "calendar", "social", false),
    kind("event_changed", "An event you answered is changed", "calendar", "social", false),
    kind("event_rsvp", "Someone answers your event", "calendar", "social", false),
    kind("event_cancelled", "An event you answered is cancelled", "calendar", "social", false),
    kind("reminder", "Bookmark reminders", "bookmark", "bookmarks", false),
    kind("solved", "Your post is marked as the answer", "circle-check", "replies", false),
    kind("live_answered", "A host answers your live question", "message-square", "replies", false),
    kind("referral_joined", "Someone joins through your referral link", "user", "social", false),
    kind("bulk", "News and announcements from the site", "megaphone", "staff", true),
    kind("assigned", "A topic is assigned to you", "user", "staff", false),
    kind("edited", "Staff edit your post", "pencil", "staff", true),
    kind("moved", "Staff move your topic", "shield", "staff", true),
    kind("post_approved", "Your queued post is approved", "shield", "staff", true),
    kind("report_resolved", "Your report is handled", "shield", "staff", false),
    kind("review_queue", "New items to review (staff)", "flag", "staff", false),
    kind("admin_notice", "Site problems (administrators)", "settings", "staff", false),
    account("moderation", "shield"),
    account("post_rejected", "shield"),
    account("post_hidden", "shield"),
    account("warning", "shield"),
    account("suspended", "shield"),
    account("silenced", "shield"),
    account("restored", "shield"),
    account("account_approved", "circle-check"),
    account("content_approved", "circle-check"),
    account("content_rejected", "shield"),
    // EU DSA: statements of reasons and appeal decisions.
    account("statement", "shield"),
    account("statement_decided", "shield"),
    // Said once, when an account is made; the page it points to is the getting-started list.
    Kind { key: "welcome", label: "", icon: "star", tab: "social", email: false, mandatory: true },
    // The email already goes out from the sign-in itself.
    Kind { key: "new_login", label: "", icon: "lock", tab: "staff", email: false, mandatory: true },
];

pub const TABS: &[(&str, &str)] = &[
    ("all", "All"),
    ("mentions", "Mentions"),
    ("replies", "Replies"),
    ("likes", "Likes"),
    ("messages", "Messages"),
    ("bookmarks", "Bookmarks"),
    ("social", "Community"),
    ("staff", "From staff"),
];

/// Kinds that enabled plugins declare (`plugin:<id>:<key>`), set whenever plugins load.
static EXTRA: std::sync::RwLock<Vec<&'static Kind>> = std::sync::RwLock::new(Vec::new());

type Leaked = std::collections::HashMap<(String, String, String, String, bool), &'static Kind>;

/// Replaces the plugin kinds. A kind seen before is reused, so reloading plugins does not grow
/// memory: kinds are tiny and their text must live as long as the program.
pub fn set_extra(kinds: Vec<(String, String, String, String, bool)>) {
    static SEEN: std::sync::Mutex<Option<Leaked>> = std::sync::Mutex::new(None);
    let mut seen = SEEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let seen = seen.get_or_insert_with(Default::default);
    let mut out = Vec::new();
    for k in kinds {
        let kind = *seen.entry(k.clone()).or_insert_with(|| {
            let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
            Box::leak(Box::new(Kind {
                key: leak(k.0.clone()),
                label: leak(k.1.clone()),
                icon: leak(k.2.clone()),
                tab: leak(k.3.clone()),
                email: k.4,
                mandatory: false,
            }))
        });
        out.push(kind);
    }
    *EXTRA.write().unwrap_or_else(std::sync::PoisonError::into_inner) = out;
}

/// The forum's own kinds and the plugins'.
pub fn all() -> Vec<&'static Kind> {
    let extra = EXTRA.read().unwrap_or_else(std::sync::PoisonError::into_inner);
    KINDS.iter().chain(extra.iter().copied()).collect()
}

pub fn find(key: &str) -> Option<&'static Kind> {
    KINDS.iter().find(|k| k.key == key).or_else(|| {
        EXTRA.read().unwrap_or_else(std::sync::PoisonError::into_inner).iter().copied().find(|k| k.key == key)
    })
}

/// What a member gets until they choose otherwise.
pub fn default_channels(key: &str) -> i16 {
    if find(key).is_some_and(|k| k.email) { APP | EMAIL | PUSH } else { APP | PUSH }
}

pub fn is_mandatory(key: &str) -> bool {
    find(key).is_some_and(|k| k.mandatory)
}

pub fn icon(key: &str) -> &'static str {
    find(key).map_or("bell", |k| k.icon)
}

/// The kinds behind one tab of the notifications page.
pub fn in_tab(tab: &str) -> Vec<&'static str> {
    all().into_iter().filter(|k| k.tab == tab).map(|k| k.key).collect()
}

/// The sentence for one notification, in the reader's language.
pub fn text(t: &T, kind: &str, data: &Value) -> String {
    let s = |k: &str| data.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
    let (title, by, note, name) = (s("title"), s("by"), s("message"), s("name"));
    // A badge's name was stored in English; the built-in ones have translations.
    let name = if kind == "badge" { t.msg(&name) } else { name };
    let count = data.get("count").and_then(Value::as_i64).unwrap_or(1);
    let vars = [("title", title.as_str()), ("by", by.as_str()), ("name", name.as_str())];
    let base = match kind {
        "plugin" => s("text"),
        // A plugin's own kind: it wrote the sentence when it sent the notification.
        k if k.starts_with("plugin:") => s("text"),
        "bulk" => return note,
        "like" => {
            let first = data["users"].as_array().and_then(|a| a.first()).and_then(Value::as_str);
            let others = (count - 1).to_string();
            let v = [("title", title.as_str()), ("by", first.unwrap_or_default()), ("n", others.as_str())];
            match (first, count) {
                (Some(_), 1) => t.f("notify.like", &v),
                (Some(_), 2) => t.f("notify.like.two", &v),
                (Some(_), _) => t.f("notify.like.many", &v),
                (None, _) => t.f("notify.like.anon", &v),
            }
        }
        // Grouped kinds read differently once several are folded into one row.
        "reply" | "followed_post" | "poll_vote" | "review_queue" | "chat_message" if count > 1 => {
            let n = count.to_string();
            t.f(&format!("notify.{kind}.many"), &[("title", title.as_str()), ("by", by.as_str()), ("n", n.as_str())])
        }
        "welcome" => t.msgf("Welcome to {name}! A few quick steps will get you started.", &[("name", name.as_str())]),
        "event_changed" => t.msgf("The event “{title}” was changed", &[("title", title.as_str())]),
        "event_cancelled" => t.msgf("The event “{title}” was cancelled", &[("title", title.as_str())]),
        "event_rsvp" if count > 1 => {
            t.msgf("{n} people answered your event “{title}”", &[("n", &count.to_string()), ("title", title.as_str())])
        }
        "event_rsvp" => t.msgf("{by} answered your event “{title}”", &[("by", by.as_str()), ("title", title.as_str())]),
        k if find(k).is_some() => t.f(&format!("notify.{k}"), &vars),
        _ => t.f("notify.reply", &vars),
    };
    if note.is_empty() { base } else { format!("{base}: {note}") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_kind_has_text_in_english() {
        let t = T::new("en");
        for k in KINDS {
            let got = text(&t, k.key, &json!({ "by": "ann", "title": "Hi", "name": "x" }));
            assert!(!got.starts_with("notify."), "no text for {}", k.key);
        }
    }

    #[test]
    fn grouping_and_notes() {
        let t = T::new("en");
        assert_eq!(text(&t, "reply", &json!({ "by": "ann", "title": "Hi" })), "ann posted in Hi");
        assert_eq!(text(&t, "reply", &json!({ "by": "ann", "title": "Hi", "count": 3 })), "3 new posts in Hi");
        assert_eq!(text(&t, "warning", &json!({ "message": "be nice" })), "You received a warning: be nice");
    }

    #[test]
    fn only_replies_and_the_like_email_by_default() {
        assert_eq!(default_channels("direct_reply"), APP | EMAIL | PUSH);
        assert_eq!(default_channels("reply"), APP | PUSH);
        assert_eq!(default_channels("unknown"), APP | PUSH);
    }
}
