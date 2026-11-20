//! Groups, members, badges, leaderboard and the about page.

use super::Layout;
use askama::Template;

#[derive(Template)]
#[template(path = "group.html")]
pub struct GroupPage {
    pub layout: Layout,
    pub slug: String,
    pub name: String,
    pub members: Vec<String>,
}

#[derive(Template)]
#[template(path = "users.html")]
pub struct UsersPage {
    pub layout: Layout,
    pub q: String,
    pub sort: String,
    /// (label, url, current).
    pub sorts: Vec<(String, String, bool)>,
    /// (name, group, posts, reputation, joined, trust level and its name when shown).
    pub rows: Vec<(String, String, i32, i64, crate::ago::Ago, Option<(i16, String)>)>,
    pub prev_url: Option<String>,
    pub next_url: Option<String>,
    pub page: i64,
}

#[derive(Template)]
#[template(path = "groups.html")]
pub struct GroupsPage {
    pub layout: Layout,
    /// (name, slug, members).
    pub groups: Vec<(String, String, i64)>,
}

/// One badge in the list, in the columns `bf_db::community::badges` returns.
pub type BadgeRow = (i64, String, String, i32, i64, String, String, i16, i32);

#[derive(Template)]
#[template(path = "badges.html")]
pub struct BadgesPage {
    pub layout: Layout,
    /// Group name, then its badges.
    pub groups: Vec<(String, Vec<BadgeRow>)>,
}

#[derive(Template)]
#[template(path = "badge.html")]
pub struct BadgePage {
    pub tier: String,
    pub icon: String,
    pub color: i16,
    pub level: i32,
    pub layout: Layout,
    pub name: String,
    pub description: String,
    pub points: i32,
    pub total: i64,
    /// (name, earned).
    pub holders: Vec<(String, crate::ago::Ago)>,
}

#[derive(Template)]
#[template(path = "leaderboard.html")]
pub struct LeaderboardPage {
    pub layout: Layout,
    /// (place, name, reputation, posts).
    pub rows: Vec<(usize, String, i64, i32)>,
}

#[derive(Template)]
#[template(path = "about.html")]
pub struct AboutPage {
    pub layout: Layout,
    pub contact: bool,
    pub members: i64,
    pub topics: i64,
    pub posts: i64,
    pub online: i64,
    pub since: crate::ago::Ago,
    /// (name, group).
    pub staff: Vec<(String, String)>,
}

/// One requirement on the trust level page: how far the member is.
pub struct TrustRow {
    pub label: String,
    pub current: i64,
    pub needed: i64,
    /// 0 to 100.
    pub percent: i64,
    pub met: bool,
}

/// One level in the table of what each level needs and unlocks.
pub struct TrustLevelRow {
    pub level: i16,
    pub name: String,
    pub is_current: bool,
    pub needs: Vec<String>,
    pub unlocks: Vec<String>,
}

#[derive(Template)]
#[template(path = "trust.html")]
pub struct TrustPage {
    pub layout: Layout,
    pub level: i16,
    pub name: String,
    pub note: Option<String>,
    /// The next level, if there is one to earn: (number, name).
    pub next: Option<(i16, String)>,
    pub progress: Vec<TrustRow>,
    /// Level 3: what keeps it.
    pub keep: Vec<TrustRow>,
    pub at_risk: bool,
    pub levels: Vec<TrustLevelRow>,
    /// Limits that apply to new members.
    pub limits: Vec<String>,
}
