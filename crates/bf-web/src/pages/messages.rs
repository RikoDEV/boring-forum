//! Private-message pages.

use super::Layout;
use askama::Template;

pub struct InboxLine {
    /// The other participants: (profile url, name).
    pub people: Vec<(String, String)>,
    pub preview: String,
    pub avatar: String,
    pub count: i64,
    pub url: String,
    pub subject: String,
    pub others: String,
    pub when: crate::ago::Ago,
    pub unread: bool,
}

#[derive(Template)]
#[template(path = "messages.html")]
pub struct InboxPage {
    pub layout: Layout,
    pub folders: Vec<String>,
    pub current_folder: String,
    pub archived: bool,
    pub query: String,
    pub to: String,
    /// Why the new-message button is disabled for this member, if it is.
    pub new_blocked: Option<String>,
    pub lines: Vec<InboxLine>,
}

pub struct ParticipantView {
    pub url: String,
    pub username: String,
    pub is_self: bool,
    pub seen_latest: bool,
}

pub struct MessageView {
    pub author: String,
    pub when: crate::ago::Ago,
    /// Cooked by bf-cook.
    pub cooked: String,
    pub new: bool,
}

#[derive(Template)]
#[template(path = "conversation.html")]
pub struct ConversationPage {
    pub layout: Layout,
    pub id: i64,
    pub subject: String,
    pub archived: bool,
    pub can_manage: bool,
    pub participants: Vec<ParticipantView>,
    pub messages: Vec<MessageView>,
}
