//! Review queue, moderation panel and EU DSA pages.

use super::Layout;
use askama::Template;

pub struct ReviewView {
    pub claimed_by: Option<String>,
    pub id: i64,
    pub pending_post: bool,
    pub reasons: String,
    pub score: f32,
    pub post_url: String,
    pub title: String,
    pub author: String,
    /// Already sanitized by bf-cook.
    pub cooked: String,
    /// The Markdown source, offered for "edit, then approve".
    pub raw: String,
    pub when: crate::ago::Ago,
}

#[derive(Template)]
#[template(path = "review.html")]
pub struct ReviewPage {
    /// Flagged chat messages, for site-wide staff: (message id, channel, author, flagged by, reason, text).
    pub chat_flags: Vec<(i64, i64, String, String, String, String)>,
    pub layout: Layout,
    pub items: Vec<ReviewView>,
    /// Accounts awaiting approval (staff): (id, username, email, joined).
    pub pending_users: Vec<(i64, String, String, crate::ago::Ago)>,
    /// Writes held by a "require approval" word outside posts: (id, what, author, text, filed).
    pub held: Vec<(i64, String, String, String, crate::ago::Ago)>,
    /// Open member reports (staff): (report id, profile url, member, reporter, reason, filed).
    pub member_reports: Vec<(i64, String, String, String, String, crate::ago::Ago)>,
    /// Open reports on private conversations (staff), handled on their own page.
    pub message_reports: i64,
    /// (id, title, selected) for the filter.
    pub forums: Vec<(i64, String, bool)>,
    pub kind: String,
    pub sort: String,
    /// Site-wide staff may also act on an author from the queue.
    pub staff: bool,
}

#[derive(Template)]
#[template(path = "modcp.html")]
pub struct ModCpPage {
    pub layout: Layout,
    pub tab: String,
    /// (key, label, url, count): the tabs, and the review queue beside them.
    pub tabs: Vec<(String, String, String, i64)>,
    pub queue: i64,
    pub hidden: i64,
    pub deleted: i64,
    pub unapproved: i64,
    /// How long the oldest pending item has waited.
    pub oldest: Option<crate::ago::Ago>,
    /// Members and warnings, the log and the AI's doubts show to site-wide staff only.
    pub staff: bool,
    /// (url, title, author, reasons, when)
    pub reports: Vec<(String, String, String, String, crate::ago::Ago)>,
    pub members: Vec<ModMember>,
    pub find: String,
    pub warnings: Vec<ModWarning>,
    pub log: Vec<ModLogRow>,
    pub prev_url: Option<String>,
    pub next_url: Option<String>,
    /// (url, title, author, spam, abuse)
    pub doubted: Vec<(String, String, String, i16, i16)>,
    /// (url, title, author, reason)
    pub hidden_posts: Vec<(String, String, String, String)>,
    pub ip: String,
    /// (url, username, posts, last used)
    pub ip_members: Vec<(String, String, i64, crate::ago::Ago)>,
    /// What a moderator's last action did, shown once.
    pub notice: String,
    /// Where an action form returns to.
    pub back: String,
    pub bans: Vec<ModBan>,
    /// An administrator may also add addresses that bans never apply to.
    pub admin: bool,
}

pub struct ModBan {
    pub id: i64,
    pub kind: String,
    pub value: String,
    pub reason: String,
    pub when: crate::ago::Ago,
    /// When the ban lapses; `None` for a ban until lifted.
    pub expires: Option<crate::ago::Ago>,
}

pub struct ModMember {
    pub id: i64,
    /// The viewer may warn, restrict or clean up after this member.
    pub can_act: bool,
    /// Silenced or suspended now.
    pub restricted: bool,
    /// Recent enough and low enough to count as a possible spammer.
    pub new_account: bool,
    pub name: String,
    pub joined: crate::ago::Ago,
    pub seen: crate::ago::Ago,
    pub posts: i32,
    pub trust: i16,
    pub warning_points: i64,
    /// Words for what is wrong with the account: suspended, silenced, waiting for approval.
    pub flags: Vec<String>,
}

pub struct ModWarning {
    pub member: String,
    pub moderator: String,
    pub points: i32,
    pub reason: String,
    pub when: crate::ago::Ago,
    pub expires: crate::ago::Ago,
}

pub struct ModLogRow {
    pub actor: String,
    pub what: String,
    pub note: String,
    /// Where it happened, as a link when there is one: (url, text).
    pub target: (String, String),
    pub when: crate::ago::Ago,
}

// EU Digital Services Act pages.

#[derive(Template)]
#[template(path = "dsa_notice.html")]
pub struct DsaNoticePage {
    pub layout: Layout,
    pub categories: Vec<(&'static str, String)>,
    pub url: String,
    pub error: Option<String>,
    pub done: Option<String>,
    pub altcha: bool,
    pub contact: String,
}

pub struct StatementView {
    pub id: i64,
    pub ground: String,
    pub facts: String,
    pub automated_detection: bool,
    pub automated_decision: bool,
    pub notice_id: Option<i64>,
    pub appealed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub appeal_outcome: Option<String>,
}

#[derive(Template)]
#[template(path = "statement.html")]
pub struct StatementPage {
    pub layout: Layout,
    pub s: StatementView,
    pub created: crate::ago::Ago,
    pub action_text: String,
    pub target_text: String,
    pub target_link: Option<String>,
    pub category_text: String,
    pub can_appeal: bool,
}

pub struct NoticeView {
    pub id: i64,
    pub url: String,
    pub category: String,
    pub explanation: String,
    pub legal_ground: String,
    pub notifier_name: String,
    pub notifier_email: String,
    pub created_at: crate::ago::Ago,
}

pub struct AppealView {
    pub id: i64,
    pub action: String,
    pub target_type: String,
    pub username: Option<String>,
    pub category: String,
    pub facts: String,
    pub appeal_text: Option<String>,
    pub moderator_id: Option<i64>,
}

#[derive(Template)]
#[template(path = "dsa_review.html")]
pub struct DsaReviewPage {
    pub layout: Layout,
    pub notices: Vec<NoticeView>,
    pub appeals: Vec<AppealView>,
    pub me: Option<i64>,
}

pub struct TransparencyView {
    pub notices: i64,
    pub notices_by_category: Vec<(String, i64)>,
    pub notices_actioned: i64,
    pub notices_rejected: i64,
    pub statements: i64,
    pub by_action: Vec<(String, i64)>,
    pub by_ground: Vec<(String, i64)>,
    pub by_category: Vec<(String, i64)>,
    pub automated_detection: i64,
    pub automated_decision: i64,
    pub appeals: i64,
    pub appeals_reversed: i64,
    pub appeals_upheld: i64,
}

#[derive(Template)]
#[template(path = "transparency.html")]
pub struct TransparencyPage {
    pub layout: Layout,
    pub r: TransparencyView,
    pub from: String,
    pub to: String,
    pub period: String,
    pub periods: Vec<(String, String)>,
    pub notice_hours: Option<String>,
    pub appeal_hours: Option<String>,
    pub contact: String,
}
