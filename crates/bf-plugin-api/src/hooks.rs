//! Typed payloads of the hooks (the `data` of a [`crate::Request`]). Fields the host may leave
//! out are `Option` or have a default. Hooks not listed here pass plain JSON: read them with
//! `req.data`.

use serde::Deserialize;
use serde_json::Value;

/// `post.before_create` and `post.before_cook`. Answer [`crate::Reply::data`] with
/// `{"raw": "new text"}` to change the text, or [`crate::Reply::reject`].
#[derive(Debug, Clone, Deserialize)]
pub struct PostBeforeCreate {
    pub raw: String,
    #[serde(default)]
    pub author_id: i64,
}

/// `post.after_cook`. Answer [`crate::Reply::data`] with `{"html": "..."}`; the host sanitizes it again.
#[derive(Debug, Clone, Deserialize)]
pub struct PostAfterCook {
    pub html: String,
}

/// `item.before_create` (a topic title) answers like `post.before_create` with `{"title": ...}`.
#[derive(Debug, Clone, Deserialize)]
pub struct ItemBeforeCreate {
    pub title: String,
}

/// `search.query`: answer [`crate::Reply::data`] with `{"q": ...}`.
#[derive(Debug, Clone, Deserialize)]
pub struct SearchQuery {
    pub q: String,
}

/// `user.before_register`; answer [`crate::Reply::reject`] to refuse it.
#[derive(Debug, Clone, Deserialize)]
pub struct UserBeforeRegister {
    pub username: String,
    pub email: String,
}

/// `panel.render` (a page the plugin adds to settings, the moderation area, profiles, the admin
/// panel or the public site). Answer [`crate::Reply::html`].
#[derive(Debug, Clone, Deserialize)]
pub struct PanelRender {
    pub area: String,
    pub key: String,
    pub viewer_id: Option<i64>,
    /// On a profile: the member whose profile it is.
    pub profile_id: Option<i64>,
    /// The viewer's language (`en`, `pl`, `ar`), and `ltr` or `rtl`.
    #[serde(default)]
    pub locale: String,
    #[serde(default)]
    pub dir: String,
    /// The plugin's `locales/<code>.json` for that language, over the English one.
    #[serde(default)]
    pub strings: Value,
}

/// The signed-in member of a `page.request`.
#[derive(Debug, Clone, Deserialize)]
pub struct PageViewer {
    pub id: i64,
    pub username: String,
    #[serde(default)]
    pub is_admin: bool,
    /// A moderator or administrator.
    #[serde(default)]
    pub staff: bool,
}

/// `page.request`: a visitor opened (GET) or submitted (POST) one of the plugin's `[[routes]]`
/// under `/app/<id>/…`. Answer [`crate::Reply::page`] (or [`crate::Reply::feed`] for a feed
/// route). The host has already checked who may open the route, the CSRF token and the rate of
/// posting; `form` holds the submitted fields as text (a name sent twice becomes a list).
#[derive(Debug, Clone, Deserialize)]
pub struct PageRequest {
    /// The `key` of the matched route.
    pub route: String,
    pub method: String,
    /// The path asked for, such as `/app/notes/entries/hello`.
    #[serde(default)]
    pub path: String,
    /// `{slug}` and other parameters of the route's path.
    #[serde(default)]
    pub params: Value,
    #[serde(default)]
    pub query: Value,
    #[serde(default)]
    pub form: Value,
    pub viewer_id: Option<i64>,
    pub viewer: Option<PageViewer>,
    /// The plugin's own `[[permissions]]` keys this member holds (all of them for an administrator).
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub locale: String,
    #[serde(default)]
    pub dir: String,
    #[serde(default)]
    pub strings: Value,
    #[serde(default)]
    pub base_url: String,
}

impl PageRequest {
    /// A path parameter.
    pub fn param(&self, name: &str) -> &str {
        self.params.get(name).and_then(Value::as_str).unwrap_or_default()
    }

    /// A submitted field (the first value, if it was sent several times).
    pub fn field(&self, name: &str) -> &str {
        match self.form.get(name) {
            Some(Value::String(s)) => s,
            Some(Value::Array(list)) => list.first().and_then(Value::as_str).unwrap_or_default(),
            _ => "",
        }
    }

    pub fn query_param(&self, name: &str) -> &str {
        self.query.get(name).and_then(Value::as_str).unwrap_or_default()
    }

    pub fn can(&self, permission: &str) -> bool {
        self.permissions.iter().any(|p| p == permission)
    }
}

/// `content.review`: a moderator decided an entry the plugin held with `review.hold`.
#[derive(Debug, Clone, Deserialize)]
pub struct ContentReview {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(rename = "ref")]
    pub reference: String,
    /// `approve` or `reject`.
    pub decision: String,
    pub by: i64,
}

/// `widget.render`: a block in the page builder. Cached for everyone, so there is no viewer.
/// `settings` are the values an administrator filled in from the widget's declared fields.
#[derive(Debug, Clone, Deserialize)]
pub struct WidgetRender {
    pub key: String,
    #[serde(default)]
    pub settings: Value,
}

/// `slot.render`: draws a slot for the member looking at the page (declare it in `dynamic_slots`).
#[derive(Debug, Clone, Deserialize)]
pub struct SlotRender {
    pub slot: String,
    /// The address of the page, like `/t/hello/12`.
    #[serde(default)]
    pub page: String,
    /// Set in `post_footer`: the post the slot sits in.
    pub post_id: Option<i64>,
    pub viewer_id: Option<i64>,
    /// What the slot sits on (see [`Subject`]); `None` when the page is none of those.
    pub subject: Option<Subject>,
    #[serde(default)]
    pub locale: String,
    #[serde(default)]
    pub dir: String,
    #[serde(default)]
    pub strings: Value,
}

/// `action.run`: a button from `[[post_actions]]` or `[[composer_buttons]]` was pressed.
#[derive(Debug, Clone, Deserialize)]
pub struct ActionRun {
    pub key: String,
    /// `post` or `composer`.
    pub kind: String,
    pub post_id: Option<i64>,
    /// What was selected in the composer (up to 2000 characters).
    #[serde(default)]
    pub selection: String,
    pub viewer_id: i64,
    #[serde(default)]
    pub locale: String,
    #[serde(default)]
    pub dir: String,
    #[serde(default)]
    pub strings: Value,
}

/// `onebox.resolve`: asked only for addresses on a host the plugin holds `http:<host>` for.
/// Answer [`crate::Reply::card`], or [`crate::Reply::none`] to let the next provider try.
#[derive(Debug, Clone, Deserialize)]
pub struct OneboxResolve {
    pub url: String,
}

/// `api.request`: `/api/p/<plugin-id>/<path>`.
#[derive(Debug, Clone, Deserialize)]
pub struct ApiRequest {
    pub method: String,
    pub path: String,
    #[serde(default)]
    pub body: Value,
    pub user_id: Option<i64>,
}

/// `job.run`: a job the plugin queued with [`crate::host::enqueue_job`].
#[derive(Debug, Clone, Deserialize)]
pub struct JobRun {
    #[serde(default)]
    pub payload: Value,
}

/// What a `slot.render` slot sits on, when the host could tell from the page: the member of a
/// profile or card (with their latest badges and public profile fields, the plugins' own
/// included), a topic, or a forum. Only what a guest could see, apart from members-only profiles.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Subject {
    /// `user`, `topic` or `forum`.
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub id: i64,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub title: Option<String>,
    pub group: Option<String>,
    pub slug: Option<String>,
    /// For a topic: the forum it is in.
    pub forum_id: Option<i64>,
    #[serde(default)]
    pub badges: Vec<SubjectBadge>,
    #[serde(default)]
    pub fields: Vec<SubjectField>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SubjectBadge {
    pub name: String,
    #[serde(default)]
    pub tier: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub level: i64,
}

/// A public profile field with an answer. `plugin` names the plugin that declared it.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SubjectField {
    pub label: String,
    #[serde(default)]
    pub kind: String,
    pub value: String,
    pub plugin: Option<String>,
}

/// `cook.rule`: the text a `callback` cook rule matched; answer [`crate::Reply::html`]
/// (sanitized like a post) or nothing to leave the text as written. At most 20 a post.
#[derive(Debug, Clone, Deserialize)]
pub struct CookRule {
    /// The rule's `callback` name from the manifest.
    pub rule: String,
    pub text: String,
}

/// `admin.form.save`: the values an administrator saved on one of the plugin's settings or admin
/// pages, already checked against the declared fields. Answer [`crate::Reply::reject`] to refuse.
#[derive(Debug, Clone, Deserialize)]
pub struct AdminFormSave {
    #[serde(default)]
    pub values: Value,
}

/// `user.can`: asked once per member. Answer with [`UserCanAnswer`] to take actions away (never to
/// grant): `{"deny": ["reply", "chat"], "deny_nodes": {"12": ["read"]}}`.
#[derive(Debug, Clone, Deserialize)]
pub struct UserCan {
    pub user_id: i64,
    #[serde(default)]
    pub groups: Vec<i64>,
}

/// The answer to `user.can`. Actions: `view`, `read`, `create`, `reply`, `attach`, `moderate`,
/// `edit`, `delete`, `react`, `flag`, `report`, `vote`, `bookmark`, `follow`, `comment`, `chat`,
/// `pm`, `upload` and `write` (every state-changing
/// request).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct UserCanAnswer {
    pub deny: Vec<String>,
    pub deny_nodes: std::collections::BTreeMap<String, Vec<String>>,
}

/// `signup.validate`: the sign-up being submitted. `fields` maps the plugin's own profile fields
/// (`<plugin>:<key>`) and others (`field:<id>`) to what was entered. Answer `reject` to refuse.
#[derive(Debug, Clone, Deserialize)]
pub struct SignupValidate {
    pub username: String,
    pub email: String,
    #[serde(default)]
    pub fields: Value,
}

/// `post.created` and `topic.created`.
#[derive(Debug, Clone, Deserialize)]
pub struct PostCreated {
    pub topic_id: i64,
    pub post_number: i64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub author: String,
    pub node_id: Option<i64>,
}

/// `post.edited`.
#[derive(Debug, Clone, Deserialize)]
pub struct PostEdited {
    pub topic_id: i64,
    pub post_number: i64,
    #[serde(default)]
    pub editor: String,
    #[serde(default)]
    pub reverted: bool,
    #[serde(default)]
    pub remote: bool,
}

/// `topic.moved`.
#[derive(Debug, Clone, Deserialize)]
pub struct TopicMoved {
    pub topic_id: Option<i64>,
    pub from: Option<i64>,
    pub to: Option<i64>,
}

/// `post.deleted` and `topic.deleted` (`post_number` is absent for a topic).
#[derive(Debug, Clone, Deserialize)]
pub struct Deleted {
    pub topic_id: i64,
    pub post_number: Option<i64>,
    #[serde(default)]
    pub actor: Value,
}

/// `user.registered`, `user.login`, `user.logout`, `user.password_changed`, `user.email_changed`,
/// `profile.updated` and `user.erased`: a member, and how where it applies.
#[derive(Debug, Clone, Deserialize)]
pub struct UserEvent {
    pub user_id: i64,
    pub username: Option<String>,
    /// The sign-up or sign-in method (`password`, `magic_link`, `2fa`, `passkey`, a provider key).
    pub method: Option<String>,
}

/// `reaction.changed`.
#[derive(Debug, Clone, Deserialize)]
pub struct ReactionChanged {
    pub user_id: i64,
    pub post_id: i64,
    pub reaction: String,
    pub on: bool,
}

/// `vote.cast`.
#[derive(Debug, Clone, Deserialize)]
pub struct VoteCast {
    pub user_id: i64,
    /// `post` or `topic`.
    pub target_type: String,
    pub target_id: i64,
    pub topic_id: i64,
    pub value: i64,
}

/// `poll.voted`.
#[derive(Debug, Clone, Deserialize)]
pub struct PollVoted {
    pub user_id: i64,
    pub poll_id: i64,
    pub topic_id: i64,
    #[serde(default)]
    pub options: Vec<Value>,
    #[serde(default)]
    pub withdrawn: bool,
}

/// `flag.created`.
#[derive(Debug, Clone, Deserialize)]
pub struct FlagCreated {
    pub user_id: i64,
    pub post_id: i64,
    pub topic_id: i64,
    pub reason: String,
    #[serde(default)]
    pub score: f64,
}

/// `moderation.action`.
#[derive(Debug, Clone, Deserialize)]
pub struct ModerationAction {
    pub action: String,
    #[serde(default)]
    pub actor: Value,
    pub target_type: String,
    pub target_id: Value,
    #[serde(default)]
    pub detail: Value,
}

/// `upload.created`.
#[derive(Debug, Clone, Deserialize)]
pub struct UploadCreated {
    pub user_id: i64,
    pub name: String,
    pub key: String,
    pub size: i64,
    #[serde(default)]
    pub is_image: bool,
}

/// `follow.changed`.
#[derive(Debug, Clone, Deserialize)]
pub struct FollowChanged {
    pub user_id: i64,
    /// `topic`, `forum` or `user`.
    pub target_type: String,
    pub target_id: i64,
    pub level: Option<String>,
    pub on: Option<bool>,
}

/// `badge.awarded` (`badge` is its name) and `trust.changed` (`to` is the new level, `by` who).
#[derive(Debug, Clone, Deserialize)]
pub struct Awarded {
    pub user_id: i64,
    pub badge: Option<String>,
    pub to: Option<i64>,
    pub by: Option<Value>,
}

/// `chat.message`.
#[derive(Debug, Clone, Deserialize)]
pub struct ChatMessage {
    pub channel: Value,
    pub user_id: i64,
    pub message_id: i64,
    #[serde(default)]
    pub kind: String,
}

/// `message.created`.
#[derive(Debug, Clone, Deserialize)]
pub struct MessageCreated {
    pub from_id: i64,
    #[serde(default)]
    pub to_ids: Vec<i64>,
    pub conversation_id: i64,
    #[serde(default)]
    pub new: bool,
}
