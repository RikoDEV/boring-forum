//! Profile pages and notifications.

use super::{Layout, ProfileFieldView};
use askama::Template;

pub struct ActivityView {
    pub url: String,
    pub title: String,
    pub excerpt: String,
    pub when: crate::ago::Ago,
}

pub struct WarningView {
    pub points: i32,
    pub reason: String,
    pub by: String,
    pub when: crate::ago::Ago,
    pub active: bool,
}

pub struct WarningBox {
    pub points: i64,
    pub silenced: bool,
    pub suspended: bool,
    pub recent: Vec<WarningView>,
}

pub struct StatusReplyView {
    pub id: i64,
    pub author: String,
    pub author_url: String,
    pub body: String,
    pub when: crate::ago::Ago,
    pub can_delete: bool,
}

pub struct StatusView {
    pub id: i64,
    pub body: String,
    pub when: crate::ago::Ago,
    pub locked: bool,
    pub can_manage: bool,
    pub can_reply: bool,
    pub replies: Vec<StatusReplyView>,
}

#[derive(Template)]
#[template(path = "profile.html")]
pub struct ProfilePage {
    /// The site has an ideas forum, so profiles have a Votes tab.
    pub has_ideas: bool,
    /// The member and staff only.
    pub warning_box: Option<WarningBox>,
    pub followers_visible: bool,
    pub hide_followers: bool,
    pub follower_count: i64,
    pub follower_names: Vec<String>,
    /// `Some` when the viewer is another member.
    pub following: Option<bool>,
    pub profile_views: i64,
    pub keeps_visitors: bool,
    /// Only on the member's own profile: (name, when).
    pub recent_visitors: Vec<(String, crate::ago::Ago)>,
    pub statuses: Vec<StatusView>,
    /// This member may post a status here: their own profile, not silenced.
    pub can_post_status: bool,
    pub layout: Layout,
    /// Summary tab: (url, title, likes).
    pub top_posts: Vec<(String, String, String)>,
    /// Badges tab: (name, description).
    /// (name, description, tier, icon, colour, level)
    pub badges: Vec<(String, String, String, String, i16, i32)>,
    pub badge_points: i64,
    pub id: i64,
    pub display_name: Option<String>,
    pub cover_url: Option<String>,
    pub cover_y: i16,
    pub group_colour: Option<String>,
    pub group_icon: Option<String>,
    pub group_image: Option<String>,
    pub likes_received: i64,
    pub reputation: i64,
    pub solved_count: i64,
    pub rank: String,
    /// Trust level and its name, when shown to this viewer.
    pub trust: Option<(i16, String)>,
    /// Which rank this is, of how many: (number, total, tier, colour).
    pub rank_step: (usize, usize, String, i16),
    /// (id, name, tier, icon, colour, level, rare)
    pub recent_badges: Vec<(i64, String, String, String, i16, i32, bool)>,
    pub status_text: Option<String>,
    pub records_visitors: bool,
    /// Active tab: posts, topics or drafts.
    pub tab: &'static str,
    /// Own profile: show the private drafts tab.
    pub own: bool,
    /// Drafts tab: (label, url, when).
    pub drafts: Vec<(String, String, crate::ago::Ago)>,
    /// Staff only: (author, when, text).
    pub staff_notes: Option<Vec<(String, crate::ago::Ago, String)>>,
    /// Public custom fields, in the admin's order.
    pub fields: Vec<ProfileFieldView>,
    pub username: String,
    pub location: Option<String>,
    pub website: Option<String>,
    /// `Some(is_ignored)` when the viewer is another member.
    pub ignore: Option<bool>,
    pub title: Option<String>,
    pub group: String,
    /// The member's other groups: (name, colour, slug).
    pub extra_groups: Vec<(String, Option<String>, String)>,
    pub about_html: Option<String>,
    pub post_count: i32,
    pub joined: crate::ago::Ago,
    pub last_seen: Option<crate::ago::Ago>,
    pub activity: Vec<ActivityView>,
}

pub struct NotificationView {
    pub text: String,
    pub url: String,
    pub read: bool,
    pub when: crate::ago::Ago,
}

#[derive(Template)]
#[template(path = "notifications.html")]
pub struct NotificationsPage {
    pub layout: Layout,
    pub items: Vec<NotificationView>,
    /// (label, url, active)
    pub tabs: Vec<(&'static str, String, bool)>,
}

#[cfg(test)]
mod profile_render_tests {
    use super::*;
    use bf_core::settings::Settings;

    #[test]
    fn non_ascii_profile_fields_render_verbatim() {
        let p = ProfilePage {
            cover_y: 50,
            trust: None,
            warning_box: None,
            followers_visible: true,
            hide_followers: false,
            follower_count: 0,
            follower_names: vec![],
            following: None,
            profile_views: 0,
            keeps_visitors: false,
            recent_visitors: vec![],
            statuses: vec![],
            can_post_status: false,
            top_posts: vec![],
            badges: vec![],
            badge_points: 0,
            layout: Layout::new(&Settings::default(), None),
            id: 2,
            fields: vec![],
            staff_notes: None,
            display_name: None,
            cover_url: None,
            group_colour: None,
            group_icon: None,
            group_image: None,
            likes_received: 0,
            reputation: 0,
            solved_count: 0,
            rank: String::new(),
            rank_step: (0, 0, "bronze".into(), 1),
            recent_badges: vec![],
            status_text: None,
            records_visitors: false,
            tab: "posts",
            has_ideas: false,
            own: false,
            drafts: vec![],
            username: "bob".into(),
            location: Some("Kraków".into()),
            website: None,
            ignore: None,
            title: None,
            group: "Members".into(),
            extra_groups: vec![],
            about_html: None,
            post_count: 1,
            joined: crate::ago::Ago::default(),
            last_seen: None,
            activity: vec![],
        };
        let html = p.render().unwrap();
        assert!(html.contains("Kraków"), "{}", &html[html.find("Location").unwrap_or(0)..][..120]);
    }
}
