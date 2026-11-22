//! Declarative registries: manifest sections that add rows to the core's own lists (profile
//! fields, groups, badges, ...). Each section needs a capability, is checked when the package is
//! read, is kept in the plugin's `schema` under `x-reg`, and is seeded into the core tables each
//! time plugins load, owned by the plugin (removed with it, read-only in the core admin panel).

use crate::state::AppState;
use bf_core::settings::SettingType;
use bf_db::accounts::fields::Field;
use bf_db::platform::registry_content::{self as rc, NodeRow};
use bf_db::platform::registry_groups::{self as rg, BadgeRow};
use bf_db::platform::registry_more::{self as rm, ReactionRow, ReasonRow};
use bf_db::platform::{plugins, registry};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::ops::failed;

const MAX_PER_SECTION: usize = 50;

fn yes() -> bool {
    true
}

fn text_kind() -> String {
    "text".into()
}

/// A registry key: lowercase letters, digits, dashes and underscores. It namespaces the row
/// (`plugin:<id>:<key>`), so two plugins never collide.
pub(super) fn good_key(k: &str) -> bool {
    !k.is_empty()
        && k.len() <= 40
        && k.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_'))
}

/// `[[profile_fields]]`: a field members fill in on their profile and, with `show_on_signup`, on
/// the sign-up form (checked there like any field, and offered to `signup.validate`).
#[derive(Clone, Deserialize, Serialize)]
pub(super) struct ProfileFieldDecl {
    pub(super) key: String,
    pub(super) label: String,
    #[serde(rename = "type", alias = "kind", default = "text_kind")]
    pub(super) kind: String,
    #[serde(default)]
    pub(super) options: Vec<String>,
    #[serde(default)]
    pub(super) required: bool,
    #[serde(default)]
    pub(super) show_on_signup: bool,
    /// False: only the plugin changes the answer (`profile.set`).
    #[serde(default = "yes")]
    pub(super) editable: bool,
    /// False: the answer is not shown on profiles.
    #[serde(default = "yes")]
    pub(super) visible: bool,
    #[serde(default)]
    pub(super) icon: Option<String>,
    #[serde(default)]
    pub(super) url_prefixes: Vec<String>,
    #[serde(default)]
    pub(super) url_shorthand: bool,
    #[serde(default)]
    pub(super) https_only: bool,
}

impl ProfileFieldDecl {
    pub(super) fn field(&self) -> Result<Field, String> {
        let mut f = Field {
            id: 0,
            label: self.label.clone(),
            kind: self.kind.clone(),
            options: self.options.clone(),
            required: self.required,
            show_on_register: self.show_on_signup,
            public: self.visible,
            position: 0,
            icon: self.icon.clone(),
            url_prefixes: self.url_prefixes.clone(),
            url_shorthand: self.url_shorthand,
            https_only: self.https_only,
            plugin: None,
            editable: self.editable,
        };
        f.normalise()?;
        Ok(f)
    }
}

/// `[[groups]]`: a member group the plugin manages (`group.add` / `group.remove`). The
/// administrator grants it forum permissions like any group. It can never be staff or admin.
#[derive(Clone, Deserialize, Serialize)]
pub(super) struct GroupDecl {
    pub(super) key: String,
    pub(super) name: String,
    #[serde(default)]
    pub(super) colour: Option<String>,
}

/// `[[permissions]]`: a right the plugin checks with `user.can` (`perm = "<key>"`); the
/// administrator picks the groups that hold it on the plugin's page.
#[derive(Clone, Deserialize, Serialize)]
pub(super) struct PermissionDecl {
    pub(super) key: String,
    pub(super) label: String,
}

fn bronze() -> String {
    "bronze".into()
}

fn trophy() -> String {
    "trophy".into()
}

fn one() -> i16 {
    1
}

/// `[[badges]]`: a badge only `badge.grant` awards.
#[derive(Clone, Deserialize, Serialize)]
pub(super) struct BadgeDecl {
    pub(super) key: String,
    pub(super) name: String,
    #[serde(default)]
    pub(super) description: String,
    #[serde(default = "bronze")]
    pub(super) tier: String,
    #[serde(default = "trophy")]
    pub(super) icon: String,
    #[serde(default = "one")]
    pub(super) color: i16,
    #[serde(default)]
    pub(super) level: i32,
    #[serde(default)]
    pub(super) points: i32,
}

fn bell() -> String {
    "bell".into()
}

fn social() -> String {
    "social".into()
}

fn weight() -> f32 {
    1.0
}

/// `[[notification_kinds]]`: a kind of notification the plugin sends with
/// `{"op": "notify", "kind": "<key>", ...}`. Members get a row for it in their notification
/// settings (in-app and email), and it shows on the chosen tab of the notifications page.
#[derive(Clone, Deserialize, Serialize)]
pub(super) struct NotificationKindDecl {
    pub(super) key: String,
    /// What the member sees in their settings.
    pub(super) label: String,
    #[serde(default = "bell")]
    pub(super) icon: String,
    /// A tab of the notifications page: mentions, replies, likes, messages, bookmarks or social.
    #[serde(default = "social")]
    pub(super) tab: String,
    /// Emails by default (members can switch it off).
    #[serde(default)]
    pub(super) email: bool,
}

/// `[[reactions]]`: an extra reaction on posts, counted like the forum's own.
#[derive(Clone, Deserialize, Serialize)]
pub(super) struct ReactionDecl {
    pub(super) key: String,
    pub(super) name: String,
    /// One emoji.
    pub(super) icon: String,
    /// -1, 0 or 1: what it does to the author's reputation.
    #[serde(default)]
    pub(super) score: i16,
}

/// `[[flag_reasons]]`: a reason members can give when reporting a post or member.
#[derive(Clone, Deserialize, Serialize)]
pub(super) struct FlagReasonDecl {
    pub(super) key: String,
    pub(super) label: String,
    #[serde(default)]
    pub(super) hint: String,
    #[serde(default = "weight")]
    pub(super) weight: f32,
    #[serde(default)]
    pub(super) needs_note: bool,
}

/// `[[settings]]`: a site setting in the administrator's Settings page, stored as
/// `plugin:<id>:<key>`. The plugin reads it with `settings.get`.
#[derive(Clone, Deserialize, Serialize)]
pub(super) struct SettingDecl {
    pub(super) key: String,
    #[serde(rename = "type", alias = "kind")]
    pub(super) ty: String,
    pub(super) default: Value,
    #[serde(default)]
    pub(super) min: Option<i64>,
    #[serde(default)]
    pub(super) max: Option<i64>,
    pub(super) description: String,
}

impl SettingDecl {
    fn kind(&self) -> Option<SettingType> {
        match (self.ty.as_str(), &self.default) {
            ("bool", Value::Bool(_)) => Some(SettingType::Bool),
            ("int", Value::Number(n)) if n.is_i64() => Some(SettingType::Int),
            ("string", Value::String(_)) => Some(SettingType::String),
            _ => None,
        }
    }
}

/// `[[api_scopes]]`: a scope a member may add to an API key (Settings, API keys). The plugin's
/// `api.request` handler is told which of its scopes the calling key holds (`scopes`).
#[derive(Clone, Deserialize, Serialize)]
pub(super) struct ApiScopeDecl {
    pub(super) key: String,
    pub(super) label: String,
}

/// `[[tags]]`: a tag the plugin puts on topics with `tags.set`.
#[derive(Clone, Deserialize, Serialize)]
pub(super) struct TagDecl {
    pub(super) key: String,
    pub(super) name: String,
}

fn forum_kind() -> String {
    "forum".into()
}

fn target_topic() -> String {
    "topic".into()
}

/// `[[nodes]]`: a forum or category the plugin owns. `parent` is the key of an earlier entry,
/// or the id of an existing forum; empty puts it at the top. `nodes.create` adds forums under it.
#[derive(Clone, Deserialize, Serialize)]
pub(super) struct NodeDecl {
    pub(super) key: String,
    pub(super) title: String,
    #[serde(default = "forum_kind")]
    pub(super) kind: String,
    #[serde(default)]
    pub(super) description: String,
    #[serde(default)]
    pub(super) parent: Value,
}

/// `[[meta]]`: JSON the plugin keeps on topics, posts or members (`meta.get` / `meta.set`). A `public`
/// one (the default) appears in API output under `meta.<plugin>.<key>`; all of it is exported.
#[derive(Clone, Deserialize, Serialize)]
pub(super) struct MetaDecl {
    pub(super) key: String,
    #[serde(default = "target_topic")]
    pub(super) target: String,
    #[serde(default = "yes")]
    pub(super) public: bool,
}

/// Every section a manifest may declare.
#[derive(Clone, Default, Deserialize, Serialize)]
pub(super) struct Sections {
    #[serde(default)]
    pub(super) profile_fields: Vec<ProfileFieldDecl>,
    #[serde(default)]
    pub(super) groups: Vec<GroupDecl>,
    #[serde(default)]
    pub(super) permissions: Vec<PermissionDecl>,
    #[serde(default)]
    pub(super) badges: Vec<BadgeDecl>,
    #[serde(default)]
    pub(super) notification_kinds: Vec<NotificationKindDecl>,
    #[serde(default)]
    pub(super) reactions: Vec<ReactionDecl>,
    #[serde(default)]
    pub(super) flag_reasons: Vec<FlagReasonDecl>,
    #[serde(default)]
    pub(super) settings: Vec<SettingDecl>,
    #[serde(default)]
    pub(super) api_scopes: Vec<ApiScopeDecl>,
    #[serde(default)]
    pub(super) tags: Vec<TagDecl>,
    #[serde(default)]
    pub(super) nodes: Vec<NodeDecl>,
    #[serde(default)]
    pub(super) meta: Vec<MetaDecl>,
}

impl Sections {
    /// Each section's name (also its capability) and entry count.
    fn lens(&self) -> [(&'static str, usize); 12] {
        [
            ("profile_fields", self.profile_fields.len()),
            ("groups", self.groups.len()),
            ("permissions", self.permissions.len()),
            ("badges", self.badges.len()),
            ("notification_kinds", self.notification_kinds.len()),
            ("reactions", self.reactions.len()),
            ("flag_reasons", self.flag_reasons.len()),
            ("settings", self.settings.len()),
            ("api_scopes", self.api_scopes.len()),
            ("tags", self.tags.len()),
            ("nodes", self.nodes.len()),
            ("meta", self.meta.len()),
        ]
    }

    pub(super) fn is_empty(&self) -> bool {
        self.lens().iter().all(|l| l.1 == 0)
    }

    /// Each non-empty section needs its capability; keys are unique and valid.
    pub(super) fn validate(&self, caps: &[String]) -> Result<(), String> {
        for (section, n) in self.lens() {
            if n > MAX_PER_SECTION {
                return Err(format!("Too many [[{section}]] entries."));
            }
            if n > 0 && !caps.iter().any(|c| c == section) {
                return Err(format!("A plugin that declares [[{section}]] must ask for the {section} capability."));
            }
        }
        keys_ok_list(&self.settings.iter().map(|k| k.key.as_str()).collect::<Vec<_>>(), "setting")?;
        keys_ok_list(&self.api_scopes.iter().map(|k| k.key.as_str()).collect::<Vec<_>>(), "API scope")?;
        keys_ok_list(&self.tags.iter().map(|k| k.key.as_str()).collect::<Vec<_>>(), "tag")?;
        keys_ok_list(&self.nodes.iter().map(|k| k.key.as_str()).collect::<Vec<_>>(), "forum")?;
        for s in &self.settings {
            let range_ok = match (s.kind(), s.default.as_i64()) {
                (Some(SettingType::Int), Some(d)) => s.min.is_none_or(|m| d >= m) && s.max.is_none_or(|m| d <= m),
                (Some(_), _) => true,
                (None, _) => false,
            };
            if !range_ok || s.description.trim().is_empty() || s.description.chars().count() > 200 {
                return Err(format!(
                    "Bad setting {}: a type (bool, int or string) with a default of that type, and a description.",
                    s.key
                ));
            }
        }
        if let Some(s) = self.api_scopes.iter().find(|s| s.label.trim().is_empty() || s.label.chars().count() > 100) {
            return Err(format!("Bad API scope {}.", s.key));
        }
        if let Some(t) = self.tags.iter().find(|t| t.name.trim().is_empty() || t.name.chars().count() > 40) {
            return Err(format!("Bad tag {}.", t.key));
        }
        for (i, n) in self.nodes.iter().enumerate() {
            let parent_ok = match &n.parent {
                Value::Null => true,
                Value::String(s) => s.is_empty() || self.nodes[..i].iter().any(|e| e.key == *s),
                Value::Number(x) => x.is_i64(),
                _ => false,
            };
            if !parent_ok
                || !["forum", "category"].contains(&n.kind.as_str())
                || n.title.trim().is_empty()
                || n.title.chars().count() > 100
                || n.description.chars().count() > 500
            {
                return Err(format!("Bad forum {}: a forum or category, with a parent declared before it.", n.key));
            }
        }
        let mut seen_meta = std::collections::HashSet::new();
        if let Some(m) = self.meta.iter().find(|m| !seen_meta.insert((m.target.as_str(), m.key.as_str()))) {
            return Err(format!("Repeated metadata {}.", m.key));
        }
        if let Some(m) =
            self.meta.iter().find(|m| !["topic", "post", "user"].contains(&m.target.as_str()) || !good_key(&m.key))
        {
            return Err(format!("Bad metadata {}: target topic or post.", m.key));
        }
        keys_ok_list(&self.notification_kinds.iter().map(|k| k.key.as_str()).collect::<Vec<_>>(), "notification kind")?;
        keys_ok_list(&self.reactions.iter().map(|k| k.key.as_str()).collect::<Vec<_>>(), "reaction")?;
        keys_ok_list(&self.flag_reasons.iter().map(|k| k.key.as_str()).collect::<Vec<_>>(), "flag reason")?;
        if let Some(k) = self.notification_kinds.iter().find(|k| {
            k.label.trim().is_empty()
                || k.label.chars().count() > 80
                || !["mentions", "replies", "likes", "messages", "bookmarks", "social"].contains(&k.tab.as_str())
                || k.icon.len() > 40
                || !k.icon.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        }) {
            return Err(format!("Bad notification kind {}.", k.key));
        }
        if let Some(r) = self.reactions.iter().find(|r| {
            r.name.trim().is_empty()
                || r.name.chars().count() > 40
                || r.icon.trim().is_empty()
                || r.icon.chars().count() > 8
                || !(-1..=1).contains(&r.score)
        }) {
            return Err(format!("Bad reaction {}.", r.key));
        }
        if let Some(r) = self
            .flag_reasons
            .iter()
            .find(|r| r.label.trim().is_empty() || r.label.chars().count() > 80 || !(0.0..=10.0).contains(&r.weight))
        {
            return Err(format!("Bad flag reason {}.", r.key));
        }
        keys_ok_list(&self.groups.iter().map(|g| g.key.as_str()).collect::<Vec<_>>(), "group")?;
        keys_ok_list(&self.permissions.iter().map(|g| g.key.as_str()).collect::<Vec<_>>(), "permission")?;
        keys_ok_list(&self.badges.iter().map(|g| g.key.as_str()).collect::<Vec<_>>(), "badge")?;
        let hex = |c: &str| c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|x| x.is_ascii_hexdigit());
        if let Some(g) = self.groups.iter().find(|g| {
            g.name.trim().is_empty() || g.name.chars().count() > 60 || g.colour.as_deref().is_some_and(|c| !hex(c))
        }) {
            return Err(format!("Bad group {}: a name up to 60 characters and a colour like #aabbcc.", g.key));
        }
        if let Some(p) = self.permissions.iter().find(|p| p.label.trim().is_empty() || p.label.chars().count() > 100) {
            return Err(format!("Bad permission {}.", p.key));
        }
        if let Some(b) = self.badges.iter().find(|b| {
            b.name.trim().is_empty()
                || b.name.chars().count() > 60
                || !["bronze", "silver", "gold"].contains(&b.tier.as_str())
                || !(0..=9).contains(&b.color)
                || b.icon.len() > 40
                || !b.icon.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        }) {
            return Err(format!("Bad badge {}.", b.key));
        }
        let mut seen = std::collections::HashSet::new();
        for f in &self.profile_fields {
            if !good_key(&f.key) || !seen.insert(f.key.as_str()) {
                return Err(format!("Bad or repeated profile field key {}.", f.key));
            }
            f.field().map_err(|e| format!("Profile field {}: {e}", f.key))?;
        }
        Ok(())
    }

    pub(super) fn store(&self, schema: &mut Value) {
        schema["x-reg"] = json!(self);
    }

    pub(super) fn load(schema: Option<&Value>) -> Sections {
        schema.and_then(|s| s.get("x-reg")).and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
    }
}

fn keys_ok_list(keys: &[&str], what: &str) -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    match keys.iter().find(|k| !good_key(k) || !seen.insert(**k)) {
        Some(k) => Err(format!("Bad or repeated {what} key {k}.")),
        None => Ok(()),
    }
}

/// Seeds every registry from the enabled, approved plugins. Called at the end of each load.
pub(super) async fn sync_all(state: &AppState) {
    super::gate::forget();
    let Some(pool) = state.pool.as_ref() else { return };
    let rows = match plugins::all(pool).await {
        Ok(r) => r,
        Err(e) => return tracing::warn!(error = %e, "plugin registries not synced"),
    };
    let active: Vec<_> = rows.into_iter().filter(|p| p.enabled && p.approved).collect();
    let ids: Vec<String> = active.iter().map(|p| p.id.clone()).collect();
    let mut fields = Vec::new();
    for p in &active {
        for d in Sections::load(p.schema.as_ref()).profile_fields {
            if let Ok(f) = d.field() {
                fields.push((p.id.clone(), d.key, f));
            }
        }
    }
    synced(registry::sync_profile_fields(pool, &fields, &ids).await, "plugin profile fields");
    let (mut groups, mut perms, mut badges) = (Vec::new(), Vec::new(), Vec::new());
    for p in &active {
        let s = Sections::load(p.schema.as_ref());
        groups.extend(s.groups.into_iter().map(|g| (p.id.clone(), g.key, g.name.trim().to_owned(), g.colour)));
        perms.extend(s.permissions.into_iter().map(|x| (p.id.clone(), x.key, x.label)));
        badges.extend(s.badges.into_iter().map(|b| BadgeRow {
            plugin: p.id.clone(),
            key: b.key,
            name: b.name,
            description: b.description,
            tier: b.tier,
            icon: b.icon,
            color: b.color,
            level: b.level,
            points: b.points,
            category: p.name.clone(),
        }));
    }
    synced(rg::sync_groups(pool, &groups, &ids).await, "plugin groups");
    synced(rg::sync_permissions(pool, &perms, &ids).await, "plugin permissions");
    synced(rg::sync_badges(pool, &badges, &ids).await, "plugin badges");
    let (mut kinds, mut reactions, mut reasons) = (Vec::new(), Vec::new(), Vec::new());
    for p in &active {
        let s = Sections::load(p.schema.as_ref());
        kinds.extend(
            s.notification_kinds
                .into_iter()
                .map(|k| (format!("plugin:{}:{}", p.id, k.key), k.label, k.icon, k.tab, k.email)),
        );
        reactions.extend(s.reactions.into_iter().map(|r| ReactionRow {
            plugin: p.id.clone(),
            key: r.key,
            name: r.name,
            icon: r.icon,
            score: r.score,
        }));
        reasons.extend(s.flag_reasons.into_iter().map(|r| ReasonRow {
            plugin: p.id.clone(),
            key: r.key,
            label: r.label,
            hint: r.hint,
            weight: r.weight,
            needs_note: r.needs_note,
            illegal: false,
        }));
    }
    bf_core::notifications::set_extra(kinds);
    let (mut settings, mut scopes, mut tags, mut nodes) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for p in &active {
        let s = Sections::load(p.schema.as_ref());
        for d in s.settings {
            if let Some(ty) = d.kind() {
                settings.push((format!("plugin:{}:{}", p.id, d.key), ty, d.default, d.min, d.max, d.description));
            }
        }
        scopes.extend(s.api_scopes.into_iter().map(|x| (p.id.clone(), x.key, x.label)));
        tags.extend(s.tags.into_iter().map(|x| (p.id.clone(), x.key, x.name)));
        nodes.extend(s.nodes.into_iter().map(|n| NodeRow {
            plugin: p.id.clone(),
            key: n.key,
            kind: n.kind,
            title: n.title,
            description: n.description,
            parent: match n.parent {
                Value::String(s) => s,
                Value::Number(x) => x.to_string(),
                _ => String::new(),
            },
        }));
    }
    // New settings take effect on a copy rebuilt from what is stored, so values saved before
    // the plugin was loaded (or before a restart) are picked up.
    bf_core::settings::set_extra(settings);
    // Through the guarded reload: one that read the rows before a setting changed here must not put
    // the older values back over it (the listener runs this after every settings notification).
    state.reload_settings().await;
    synced(rc::sync_api_scopes(pool, &scopes, &ids).await, "plugin API scopes");
    synced(rc::sync_tags(pool, &tags, &ids).await, "plugin tags");
    if synced(rc::sync_nodes(pool, &nodes, &ids).await, "plugin forums") == Some(true) {
        state.forum.invalidate();
    }
    if synced(rm::sync_reactions(pool, &reactions, &ids).await, "plugin reactions").is_some() {
        let _ = crate::forum::reactions::load_reactions(state).await;
    }
    if synced(rm::sync_reasons(pool, &reasons, &ids).await, "plugin flag reasons").is_some() {
        let _ = crate::moderation::report_reasons::load(state).await;
    }
}

/// The result of one registry sync; a failure is logged and turns into `None`.
fn synced<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> Option<T> {
    result.map_err(|e| tracing::warn!(error = %e, "{what} not synced")).ok()
}

/// `signup.validate`: plugins that listen see the sign-up and may refuse it. `answers` maps each
/// asked field to what was entered: a plugin's own fields by their `key`, others as `field:<id>`.
pub(crate) async fn signup_refused(
    state: &AppState,
    username: &str,
    email: &str,
    answers: &[(i64, String)],
) -> Option<String> {
    let listeners = super::listening("signup.validate");
    if listeners.is_empty() {
        return None;
    }
    let pool = state.pool.as_ref()?;
    let keys: Vec<(i64, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT id, plugin, plugin_key FROM profile_fields").fetch_all(pool).await.ok()?;
    let mut fields = serde_json::Map::new();
    for (id, value) in answers {
        let key = keys.iter().find(|k| k.0 == *id);
        let name = match key {
            Some((_, Some(plugin), Some(k))) => format!("{plugin}:{k}"),
            _ => format!("field:{id}"),
        };
        fields.insert(name, json!(value));
    }
    for p in listeners {
        let data = json!({ "username": username, "email": email, "fields": fields });
        if let Some(out) = super::call(state, p, "signup.validate", &data).await
            && let Some(reason) = out.get("reject").and_then(Value::as_str)
        {
            return Some(reason.chars().take(300).collect());
        }
    }
    None
}

/// `group.add` / `group.remove`: a member of one of the plugin's own groups.
pub(super) async fn group_member(state: &AppState, plugin: &str, req: &Value, member: bool) -> Result<Value, String> {
    let pool = crate::http::render::pool(state).map_err(failed)?;
    let key = req.get("key").and_then(Value::as_str).unwrap_or_default();
    let user = req.get("user_id").and_then(Value::as_i64).unwrap_or_default();
    let group =
        rg::plugin_group_id(pool, plugin, key).await.map_err(failed)?.ok_or(bf_core::i18n::en("No such group."))?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id = $1)")
        .bind(user)
        .fetch_one(pool)
        .await
        .map_err(failed)?;
    if !exists {
        return Err(bf_core::i18n::en("No such member.").into());
    }
    rg::set_plugin_group_member(pool, group, user, member).await.map_err(failed)?;
    state.microcache.clear();
    Ok(json!({ "ok": true }))
}

/// `badge.grant`: one of the plugin's own badges, to a member.
pub(super) async fn badge_grant(state: &AppState, plugin: &str, req: &Value) -> Result<Value, String> {
    let pool = crate::http::render::pool(state).map_err(failed)?;
    let key = req.get("key").and_then(Value::as_str).unwrap_or_default();
    let user = req.get("user_id").and_then(Value::as_i64).unwrap_or_default();
    let granted = rg::grant_badge(pool, plugin, key, user).await.map_err(failed)?;
    Ok(json!({ "granted": granted }))
}

/// The plugin permission check behind `user.can` with `perm = "<key>"`: None when the plugin
/// declares no such permission.
pub(super) async fn holds(state: &AppState, plugin: &str, perm: &str, user: i64) -> Option<bool> {
    let pool = state.pool.as_ref()?;
    let declared = rg::permissions_of(pool, plugin).await.ok()?;
    declared.iter().any(|(k, ..)| k == perm).then_some(())?;
    rg::holds_permission(pool, plugin, perm, user).await.ok()
}

/// `profile.set`: the answer to one of the plugin's own fields, checked like a member's entry.
pub(super) async fn profile_set(state: &AppState, plugin: &str, req: &Value) -> Result<Value, String> {
    let pool = crate::http::render::pool(state).map_err(failed)?;
    let key = req.get("key").and_then(Value::as_str).unwrap_or_default();
    let user = req.get("user_id").and_then(Value::as_i64).unwrap_or_default();
    let raw = req.get("value").and_then(Value::as_str).unwrap_or_default();
    let missing = bf_core::i18n::en("No such profile field.");
    let id = registry::plugin_field_id(pool, plugin, key).await.map_err(failed)?.ok_or(missing)?;
    let field =
        bf_db::accounts::fields::fields(pool).await.map_err(failed)?.into_iter().find(|f| f.id == id).ok_or(missing)?;
    let value = field.check(raw)?;
    bf_db::accounts::fields::save_values(pool, bf_core::ids::UserId::from(user), &[(id, value)])
        .await
        .map_err(failed)?;
    Ok(json!({ "saved": true }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sections(toml_src: &str) -> Sections {
        toml::from_str(toml_src).unwrap()
    }

    #[test]
    fn sections_need_their_capability() {
        let s = sections("[[profile_fields]]\nkey = \"pronouns\"\nlabel = \"Pronouns\"");
        assert!(s.validate(&[]).is_err());
        assert!(s.validate(&["profile_fields".into()]).is_ok());
    }

    #[test]
    fn keys_are_unique_and_definitions_valid() {
        let dup =
            sections("[[profile_fields]]\nkey = \"a\"\nlabel = \"A\"\n[[profile_fields]]\nkey = \"a\"\nlabel = \"B\"");
        assert!(dup.validate(&["profile_fields".into()]).is_err());
        let bad = sections("[[profile_fields]]\nkey = \"a\"\nlabel = \"A\"\ntype = \"select\"");
        assert!(bad.validate(&["profile_fields".into()]).is_err(), "a choice needs options");
        let ok = sections(
            "[[profile_fields]]\nkey = \"a\"\nlabel = \"A\"\ntype = \"select\"\noptions = [\"x\", \"y\"]\nshow_on_signup = true",
        );
        assert!(ok.validate(&["profile_fields".into()]).is_ok());
        assert!(ok.profile_fields[0].field().unwrap().show_on_register);
    }
}
