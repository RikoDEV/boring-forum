//! Forum index, forum, topic, search and topic-fragment pages.

use super::Layout;
use askama::Template;

pub struct ForumCard {
    pub id: i64,
    pub icon: Option<String>,
    pub icon_image: Option<String>,
    /// Link forums: how often the link was followed.
    pub clicks: Option<i32>,
    pub url: String,
    pub title: String,
    pub description_html: String,
    pub topic_count: i32,
    pub post_count: i32,
    pub children: Vec<(String, String)>,
    /// Latest topic: (url, title, poster, when).
    pub last: Option<(String, String, String, crate::ago::Ago)>,
    /// One of the index's latest topics in this forum has unread posts.
    pub unread: bool,
}

pub struct CategoryBlock {
    /// A box of forums that sit at the top level, outside any category: no heading.
    pub headless: bool,
    pub id: i64,
    pub icon: Option<String>,
    pub icon_image: Option<String>,
    pub title: String,
    pub forums: Vec<ForumCard>,
}

pub struct TopicLine {
    /// Votes in an ideas forum, when it has any.
    pub votes: Option<i32>,
    /// In an ideas forum's own list, the row has a vote button: (votes, the viewer voted).
    pub vote_button: Option<(i32, bool)>,
    /// The forum, where a list mixes several: (title, url).
    pub forum: Option<(String, String)>,
    pub solved: bool,
    pub url: String,
    pub title: String,
    pub prefix: Option<String>,
    pub author: String,
    pub pinned: bool,
    pub locked: bool,
    pub id: i64,
    pub replies: i32,
    pub views: i32,
    pub last_poster: String,
    pub last_posted: crate::ago::Ago,
    pub started: crate::ago::Ago,
    /// Recent posters (Discourse "posters" column).
    pub posters: Vec<String>,
    /// Unread posts for members, with a link to the first unread one.
    pub unread: Option<(i32, String)>,
    /// (name, slug)
    pub tags: Vec<(String, String)>,
}

/// Forum index totals: (topics, posts, members, newest member as (name, profile url)).
pub type IndexStats = (i64, i64, i64, Option<(String, String)>);

#[derive(Template)]
#[template(path = "home.html")]
pub struct HomePage {
    pub layout: Layout,
    pub categories: Vec<CategoryBlock>,
    pub latest: Vec<TopicLine>,
    pub member: bool,
    pub stats: Option<IndexStats>,
    /// Rendered widgets, with title: (title, html).
    pub widgets_top: Vec<(i64, String, String, String)>,
    pub widgets_main: Vec<(i64, String, String, String)>,
    pub widgets_bottom: Vec<(i64, String, String, String)>,
    pub can_edit_layout: bool,
}

/// Widget areas `layout.html` draws around a page's content: `{name}_top` above it, `_side` beside
/// it and `_bottom` under it (the page builder). Pages with their own arrangement (home, forum,
/// topic, lists, search and the shared page shell) draw their areas themselves instead.
pub struct PageArea {
    pub name: String,
    pub widgets: PageWidgets,
    /// The layout editor is open (admins only).
    pub editing: bool,
}

impl PageArea {
    pub fn has_side(&self) -> bool {
        self.editing || !self.widgets.side.is_empty()
    }
}

/// Layout blocks around a page: (id, title, html, kind).
#[derive(Default)]
pub struct PageWidgets {
    pub top: Vec<(i64, String, String, String)>,
    pub side: Vec<(i64, String, String, String)>,
    pub bottom: Vec<(i64, String, String, String)>,
    /// A custom page's body areas: one main column, or up to three side by side.
    pub main: Vec<(i64, String, String, String)>,
    pub col1: Vec<(i64, String, String, String)>,
    pub col2: Vec<(i64, String, String, String)>,
    pub col3: Vec<(i64, String, String, String)>,
}

#[derive(Template)]
#[template(path = "forum.html")]
pub struct ForumPage {
    pub widgets: PageWidgets,
    /// Ideas forum: (left, limit) of the viewer's votes; None when unlimited or not voting.
    pub votes_left: Option<(i64, i64)>,
    pub can_edit_layout: bool,
    pub editing_layout: bool,
    pub composer: Option<TopicComposer>,
    /// Why the start-topic button is disabled for this member, if it is.
    pub composer_blocked: Option<String>,
    /// Topics moved away that left a link: (url, title).
    pub moved: Vec<(String, String)>,
    pub layout: Layout,
    pub list_style: String,
    pub forum_id: i64,
    /// The name as stored, before translation: what the Site text page is searched for.
    pub stored_title: String,
    pub title: String,
    pub description_html: String,
    pub subforums: Vec<ForumCard>,
    pub topics: Vec<TopicLine>,
    pub page: i64,
    pub prev_url: Option<String>,
    pub next_url: Option<String>,
    pub can_create: bool,
    /// Cooked forum rules.
    pub rules_html: Option<String>,
    pub member: bool,
    /// Shows the bulk action bar and topic checkboxes.
    pub moderator: bool,
    pub move_targets: Vec<(i64, String)>,
    /// (label, url, active) for the sort and filter bar.
    pub sorts: Vec<(String, String, bool)>,
    /// Prefix filter links: (prefix, url, active).
    pub prefixes: Vec<(String, String, bool)>,
    pub follow: &'static str,
    pub filters: Vec<(String, String, bool)>,
}

pub struct PostView {
    pub signature: Option<String>,
    pub wiki: bool,
    pub reaction_summary: String,
    /// (emoji, count, is the viewer's)
    pub reaction_pills: Vec<(String, String, bool)>,
    pub my_reaction: i64,
    pub author_reputation: i64,
    pub author_posts: i32,
    /// The author's trust level and its name, when the site shows levels.
    pub author_trust: Option<(i16, String)>,
    /// The author's reputation rank as an achievement: (name, medal tier, medal colour).
    pub author_rank: Option<(String, String, i16)>,
    pub author_group: String,
    /// In a group other than plain members: gets a badge on the post.
    pub author_special: bool,
    /// The group's style: a `#rrggbb` colour and an icon name.
    pub author_group_colour: Option<String>,
    pub author_group_icon: Option<String>,
    /// An uploaded badge picture, shown in place of the icon.
    pub author_group_image: Option<String>,
    /// Wrote the topic: gets a badge on later posts.
    pub author_is_op: bool,
    /// (replies to this post, number of the first reply)
    pub replies: Option<(i64, i32)>,
    pub id: i64,
    pub number: i32,
    pub author: String,
    pub author_url: String,
    pub created: crate::ago::Ago,
    pub created_iso: String,
    /// Already sanitized by bf-cook.
    pub cooked: String,
    /// "1 month later", where the topic went quiet before this post.
    pub time_gap: Option<String>,
    /// What this post means for its author: first post, or back after a long time.
    pub author_note: Option<String>,
    /// The sentence of an action entry, such as "locked this topic".
    pub action: Option<String>,
    /// What it changed to, shown in italics.
    pub action_detail: Option<String>,
    pub edited: bool,
    pub can_edit: bool,
    pub likes: i32,
    pub liked: bool,
    pub bookmarked: bool,
    /// Moderators see hidden posts with the reason.
    pub hidden_reason: Option<String>,
    /// "In reply to": (post number, author).
    pub reply_to: Option<(i32, String)>,
    /// Members can like posts that are not their own.
    pub can_like: bool,
    /// The viewer ignores this author: the post is collapsed.
    pub ignored: bool,
    /// Q&A answers: the score, the viewer's vote (1, -1, 0) and whether they may vote.
    pub vote_score: i32,
    pub my_vote: i16,
    pub can_vote: bool,
    pub show_votes: bool,
    /// Notes staff left on this post; empty for everyone else.
    pub staff_notes: Vec<StaffNote>,
}

/// A staff note as the post shows it to staff.
pub struct StaffNote {
    pub id: i64,
    pub author: String,
    pub when: crate::ago::Ago,
    pub body: String,
}

#[derive(Template)]
#[template(path = "topic.html")]
pub struct TopicPage {
    pub widgets: PageWidgets,
    pub can_edit_layout: bool,
    pub editing_layout: bool,
    pub layout: Layout,
    pub author: String,
    pub author_url: String,
    pub started: crate::ago::Ago,
    /// Ends of the timeline rail.
    pub first_month: String,
    pub last_ago: crate::ago::Ago,
    /// Built from escaped parts by `builtin::poll_html`.
    pub poll_html: String,
    /// The fields of the new-poll form, built by `community::polls::poll_fields`.
    pub poll_form: String,
    /// The calendar event with its RSVPs, built from escaped parts by `events::event_html`.
    pub event_html: String,
    /// Q&A forum: answers are voted and shown best first, on one page.
    pub qa: bool,
    /// Ideas forum: members vote for topics.
    pub ideas: bool,
    pub topic_votes: i32,
    pub voted: bool,
    pub can_vote_topic: bool,
    /// (left, limit) of the viewer's votes; None when unlimited or not voting.
    pub votes_left: Option<(i64, i64)>,
    /// The latest voters as (profile url, name), and how many more there are.
    pub voters: Vec<(String, String)>,
    pub more_voters: i64,
    pub assigned: bool,
    pub mod_actions: Vec<(i64, String)>,
    pub solved: Option<i32>,
    pub can_solve: bool,
    pub id: i64,
    /// For the timeline scrubber.
    pub highest: i32,
    pub base_url: String,
    /// (name, slug)
    pub tags: Vec<(String, String)>,
    pub can_tag: bool,
    pub tags_csv: String,
    pub title: String,
    pub prefix: Option<String>,
    pub forum_title: String,
    pub forum_url: String,
    pub locked: bool,
    pub archived: bool,
    pub featured: bool,
    pub pinned: bool,
    pub listed: bool,
    pub posts: Vec<PostView>,
    pub prev_url: Option<String>,
    pub next_url: Option<String>,
    /// Numbered page links: (number, url, current).
    pub pages: Vec<(i64, String, bool)>,
    /// Suggested topics shown after the last page.
    pub can_reply: bool,
    pub can_post_anonymously: bool,
    pub can_moderate: bool,
    /// Forums a moderator can move this topic to: (id, title).
    pub move_targets: Vec<(i64, String)>,
    /// Viewer's follow level: watch, track, normal or mute.
    pub follow: &'static str,
}

/// The new-topic form on a forum page.
pub struct TopicComposer {
    /// (category, [(forum id, label)]); subforum labels are indented.
    pub forums: Vec<(String, Vec<(i64, String)>)>,
    pub prefixes: Vec<String>,
    /// The forum's prefilled text.
    pub body: String,
    /// Open the "add to the calendar" part (the calendar's Add event button sends people here),
    /// with this date (YYYY-MM-DD) or none.
    pub event_open: bool,
    pub event_date: String,
}

pub struct SearchResult {
    pub url: String,
    pub title: String,
    pub snippet: String,
    /// The same text cut at matches: (text, is a match).
    pub title_parts: Vec<(String, bool)>,
    pub snippet_parts: Vec<(String, bool)>,
    pub author: String,
    pub forum: String,
    pub forum_url: String,
    pub when: crate::ago::Ago,
    pub number: i32,
    /// Found by the AI because it is about the same thing, not because it has the words.
    pub by_meaning: bool,
    /// Other posts of this topic that match too (0 when results are not grouped by topic).
    pub more_in_topic: i64,
    /// Search inside this topic for the same words (for "N more in this topic").
    pub more_url: String,
    pub likes: i32,
    /// Replies in the topic.
    pub replies: i32,
}

/// One facet value with its count, and the search that adds (or removes) it.
pub struct FacetLink {
    pub label: String,
    pub count: i64,
    pub url: String,
    pub active: bool,
}

/// Counts for the whole result set, top values first; empty when the search has no results.
#[derive(Default)]
pub struct SearchFacets {
    pub forums: Vec<FacetLink>,
    pub tags: Vec<FacetLink>,
    /// Newest year first.
    pub years: Vec<FacetLink>,
    /// More topics matched than were counted.
    pub capped: bool,
}

#[derive(Template)]
#[template(path = "search.html")]
pub struct SearchPage {
    pub layout: Layout,
    pub q: String,
    pub author: String,
    pub after: String,
    pub before: String,
    pub total: i64,
    pub pages: Vec<(i64, String, bool)>,
    pub prev_url: Option<String>,
    pub next_url: Option<String>,
    pub topics_only: bool,
    /// Direct matches beside the posts: (kind, url, name).
    pub matches: Vec<(&'static str, String, String)>,
    pub filtered: bool,
    pub topic: Option<i64>,
    /// (id, title, selected) of forums to search in.
    pub forums: Vec<(i64, String, bool)>,
    pub results: Vec<SearchResult>,
    /// Sort choices: (key, label, address, current). Keys: relevance, newest, oldest, likes, replies.
    pub sorts: Vec<(&'static str, String, String, bool)>,
    pub sort: String,
    /// One result per topic (the default); `group_url` is the same search with it switched.
    pub grouped: bool,
    pub group_url: String,
    /// "Did you mean": the corrected query text and its address.
    pub suggestion: Option<(String, String)>,
    /// Nothing matched what was typed: `results` are for `suggestion`, and `original_url` is the
    /// search for exactly what was typed.
    pub corrected: bool,
    pub original_url: String,
    /// Nothing matched: `results` are topics whose titles are only similar.
    pub fuzzy: bool,
    pub facets: SearchFacets,
    /// Topics staff pinned to this query, shown above the results.
    pub best_bets: Vec<SearchResult>,
    /// Widget areas `search_top`, `_side` (under the filters) and `_bottom`.
    pub widgets: PageWidgets,
    pub can_edit_layout: bool,
    pub editing_layout: bool,
}

#[derive(Template)]
#[template(path = "stream.html")]
pub struct StreamPage {
    pub layout: Layout,
    pub heading: String,
    pub topics: Vec<TopicLine>,
    pub unread_view: bool,
    /// Period links for "top": (key, url, active). Empty on other streams.
    /// (label, address, current)
    pub periods: Vec<(String, String, bool)>,
    /// Widget areas: `latest` or `top`, then `_top`, `_side` and `_bottom`.
    pub area: &'static str,
    pub widgets: PageWidgets,
    pub can_edit_layout: bool,
    pub editing_layout: bool,
}

pub struct RevisionView {
    pub n: i32,
    pub raw: String,
    /// Word diff from this version to the next newer one (escaped HTML).
    pub diff: String,
    pub hidden: bool,
    pub editor: String,
    pub reason: Option<String>,
    pub when: crate::ago::Ago,
}

#[derive(Template)]
#[template(path = "history.html")]
pub struct HistoryPage {
    pub layout: Layout,
    pub post_id: i64,
    pub staff: bool,
    pub can_revert: bool,
    pub edit_locked: bool,
    pub post_url: String,
    pub current: String,
    pub revisions: Vec<RevisionView>,
}

pub struct DeletedView {
    pub post_id: i64,
    pub post_url: String,
    pub title: String,
    pub author: String,
    pub deleted_by: String,
    pub when: crate::ago::Ago,
    pub excerpt: String,
}

#[derive(Template)]
#[template(path = "deleted.html")]
pub struct DeletedPage {
    pub layout: Layout,
    pub items: Vec<DeletedView>,
}

#[derive(Template)]
#[template(path = "topic_summary.html")]
pub struct TopicSummaryPage {
    pub layout: Layout,
    pub title: String,
    pub topic_url: String,
    pub forum_title: String,
    pub forum_url: String,
    pub author: String,
    pub started: crate::ago::Ago,
    pub last_posted: crate::ago::Ago,
    pub last_poster: String,
    pub replies: i32,
    pub views: i32,
    pub participants: i64,
    pub reactions: i64,
    pub read_minutes: i64,
    /// (post number, author, cooked, likes, url)
    pub top: Vec<(i32, String, String, i32, String)>,
    /// (name, posts)
    pub posters: Vec<(String, i64)>,
    /// (url, clicks)
    pub links: Vec<(String, i32)>,
}

/// Some posts of a topic, exactly as the topic page shows them: live updates add new replies
/// and swap edited ones with this, without loading the page again.
#[derive(Template)]
#[template(path = "topic_posts.html")]
pub struct TopicPostsPart {
    pub layout: Layout,
    pub posts: Vec<PostView>,
    pub id: i64,
    pub solved: Option<i32>,
    pub can_solve: bool,
    pub can_reply: bool,
    pub can_moderate: bool,
}

/// Sent after the page's first part, once its query finishes.
#[derive(Template)]
#[template(path = "suggested.html")]
pub struct SuggestedPart {
    pub layout: Layout,
    pub topics: Vec<TopicLine>,
}

#[derive(Template)]
#[template(path = "chat.html")]
pub struct ChatPage {
    pub layout: Layout,
    pub props: String,
}

#[cfg(test)]
mod tests {
    use super::super::Layout;
    use super::*;
    use crate::assets::CSS;
    use bf_core::settings::Settings;

    #[test]
    fn home_page_escapes_and_links_assets() {
        let mut s = Settings::default();
        s.set("site_name", serde_json::json!("<b>Forum</b>")).unwrap();
        let html = HomePage {
            layout: Layout::new(&s, None),
            widgets_top: vec![],
            widgets_main: vec![],
            can_edit_layout: false,
            widgets_bottom: vec![],
            categories: vec![],
            latest: vec![],
            member: false,
            stats: None,
        }
        .render()
        .unwrap();
        assert!(html.contains("&#60;b&#62;Forum&#60;/b&#62;") || html.contains("&lt;b&gt;Forum&lt;/b&gt;"));
        assert!(!html.contains("<b>Forum</b>"));
        assert!(html.contains(&CSS.url()));
        assert!(html.contains("data-island=\"theme-toggle\""));
    }
}
