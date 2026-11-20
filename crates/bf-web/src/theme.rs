//! Active themes: compiled stylesheets and sandboxed slot templates, kept in memory so
//! pages pick them up without a query. Rebuilt whenever an admin changes something.

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, LazyLock, RwLock};

/// Places a theme can fill. Output is sanitized, so a slot can add markup but no scripts.
pub const SLOTS: [&str; 13] = [
    "header_after",
    "header_icons",
    "home_top",
    "topic_after",
    "post_after_content",
    "topic_row_after_title",
    "profile_block",
    "profile_sidebar",
    "footer",
    "profile_header",
    "topic_header",
    "post_footer",
    "category_header",
];

/// A slot with no place in the page templates: the body of a member's hover card, drawn by plugins
/// through `slot.render` only.
pub const CARD_SLOT: &str = "user_card";

/// Core partials a theme can replace. Unlike slots they get per-use context, documented here:
/// `footer` {site_name}; `topic_title` {title, url}; `post_author` {author, url}.
/// Only replaced partials go through the template engine; the rest stay compiled.
pub const PARTIALS: [&str; 3] = ["footer", "topic_title", "post_author"];

/// Source form of one theme or component, as stored.
pub struct Source {
    pub id: i64,
    pub name: String,
    pub component: bool,
    pub enabled: bool,
    pub selectable: bool,
    pub tokens: Value,
    pub css: String,
    pub slots: Value,
    /// The light and dark palettes the theme shows, when it chose its own schemes.
    pub palettes: Option<(Value, Value)>,
    /// Groups the theme is limited to; empty means everyone.
    pub groups: Vec<i64>,
}

#[derive(Default)]
pub struct Compiled {
    pub url: String,
    pub css: String,
    pub slots: HashMap<String, String>,
    /// Partial name → template source.
    pub partials: HashMap<String, String>,
    /// Extra colour schemes members can pick.
    pub schemes: Vec<String>,
}

#[derive(Default)]
pub struct ThemeSet {
    /// By theme id; each includes the enabled components.
    themes: HashMap<i64, Arc<Compiled>>,
    /// Components alone, for "no theme chosen".
    base: Option<Arc<Compiled>>,
    by_hash: HashMap<String, Arc<Compiled>>,
    default: Option<i64>,
    choices: Vec<(i64, String)>,
    /// Compiled for admin previews, but never served to anyone else.
    disabled: std::collections::HashSet<i64>,
    /// Themes limited to groups, by theme id.
    groups: HashMap<i64, Vec<i64>>,
    /// Enabled themes in the order the admin keeps them: what the site falls back to when the
    /// built-in look is off and the chosen theme is not available.
    order: Vec<i64>,
    /// The built-in look is off: some theme always applies.
    builtin_off: bool,
}

/// What enabled plugins add to pages: slot HTML (already sanitized) and client modules.
#[derive(Default)]
pub struct PluginOutput {
    pub slots: HashMap<String, String>,
    /// (module URL, islands it serves; empty means every page)
    pub scripts: Vec<(String, Vec<String>)>,
    /// Header links from manifests: (url, label).
    pub nav: Vec<(String, String)>,
    /// Pages plugins add: (area, url, label).
    pub panels: Vec<(String, String, String)>,
    pub extras: PluginExtras,
}

/// A header link a plugin declared with more than a label: an icon, a menu it is grouped under
/// (`menu` is its heading), and an `order` (lower first). `item` carries the address, label and
/// who may see it.
#[derive(Clone)]
pub struct PluginNav {
    pub item: bf_core::nav::NavItem,
    pub icon: String,
    pub menu: String,
    pub order: i64,
}

/// What else plugins add to pages, set by `set_plugin_extras` after `set_plugin_output`.
#[derive(Default)]
pub struct PluginExtras {
    /// Style sheets (URLs), already wrapped in the `plugin` cascade layer when served.
    pub css: Vec<String>,
    /// Plugin text by locale: English label to translated label. Plugin labels are written in
    /// English and translated like the core's, by the viewer's language.
    pub translations: HashMap<String, HashMap<String, String>>,
    /// Slots that some plugin draws per viewer with `slot.render`: the page leaves a placeholder.
    pub dynamic_slots: std::collections::HashSet<String>,
    /// Header links with their visibility (group and admin), instead of the plain `nav` pairs.
    pub nav_items: Vec<bf_core::nav::NavItem>,
    /// Links with an icon, a menu to sit under, or an order.
    pub nav_rich: Vec<PluginNav>,
    /// Buttons plugins declare in their manifests: `{kind, plugin, key, label, icon, ...}`.
    pub actions: Vec<Value>,
}

static PLUGINS: LazyLock<RwLock<Arc<PluginOutput>>> = LazyLock::new(Default::default);

pub fn set_plugin_output(
    slots: HashMap<String, String>,
    scripts: Vec<(String, Vec<String>)>,
    nav: Vec<(String, String)>,
    panels: Vec<(String, String, String)>,
) {
    *PLUGINS.write().unwrap_or_else(std::sync::PoisonError::into_inner) =
        Arc::new(PluginOutput { slots, scripts, nav, panels, extras: PluginExtras::default() });
}

/// Sets the rest of what plugins add; call after `set_plugin_output`.
pub fn set_plugin_extras(extras: PluginExtras) {
    let mut w = PLUGINS.write().unwrap_or_else(std::sync::PoisonError::into_inner);
    let old = w.clone();
    *w = Arc::new(PluginOutput {
        slots: old.slots.clone(),
        scripts: old.scripts.clone(),
        nav: old.nav.clone(),
        panels: old.panels.clone(),
        extras,
    });
}

/// A plugin's English label in the viewer's language, or as written.
pub fn plugin_label(locale: &str, english: &str) -> String {
    plugin_output()
        .extras
        .translations
        .get(locale)
        .and_then(|m| m.get(english))
        .map_or_else(|| english.to_owned(), Clone::clone)
}

pub fn plugin_output() -> Arc<PluginOutput> {
    PLUGINS.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

#[derive(Clone)]
pub struct Announcement {
    pub id: i64,
    pub title: String,
    /// The Markdown it was written in: what a translation of it is looked up by.
    pub body: String,
    /// Cooked by bf-cook.
    pub html: String,
    pub starts_at: Option<chrono::DateTime<chrono::Utc>>,
    pub ends_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Text cooked from a translated Markdown source, keyed by that source: a translated announcement
/// or forum description is cooked once, not on every page.
pub fn cook_cached(markdown: &str) -> String {
    static CACHE: LazyLock<RwLock<std::collections::HashMap<String, String>>> = LazyLock::new(Default::default);
    if let Some(html) = CACHE.read().unwrap_or_else(std::sync::PoisonError::into_inner).get(markdown) {
        return html.clone();
    }
    let html = bf_cook::cook(markdown).html;
    let mut cache = CACHE.write().unwrap_or_else(std::sync::PoisonError::into_inner);
    if cache.len() > 500 {
        cache.clear();
    }
    cache.insert(markdown.to_owned(), html.clone());
    html
}

static ANNOUNCEMENTS: LazyLock<RwLock<Arc<Vec<Announcement>>>> = LazyLock::new(Default::default);

pub fn set_announcements(list: Vec<Announcement>) {
    *ANNOUNCEMENTS.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(list);
}

/// Those running now. Dates are checked at render, so they start and end without a reload.
pub fn current_announcements() -> Vec<Announcement> {
    let now = chrono::Utc::now();
    let all = ANNOUNCEMENTS.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    all.iter().filter(|a| a.starts_at.is_none_or(|t| t <= now) && a.ends_at.is_none_or(|t| t > now)).cloned().collect()
}

#[allow(clippy::type_complexity)]
static MOD_ACTIONS: LazyLock<RwLock<Arc<Vec<(i64, String)>>>> = LazyLock::new(Default::default);

/// Saved moderator actions, kept here so topic pages stay within their query budget.
pub fn set_mod_actions(list: Vec<(i64, String)>) {
    *MOD_ACTIONS.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(list);
}

pub fn mod_actions() -> Vec<(i64, String)> {
    MOD_ACTIONS.read().unwrap_or_else(std::sync::PoisonError::into_inner).to_vec()
}

#[allow(clippy::type_complexity)]
static REACTIONS: LazyLock<RwLock<Arc<Vec<(i64, String, String)>>>> = LazyLock::new(Default::default);

/// The enabled reactions, kept here so topic pages need no query for them.
pub fn set_reactions(list: Vec<(i64, String, String)>) {
    *REACTIONS.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(list);
}

pub fn reactions() -> Arc<Vec<(i64, String, String)>> {
    REACTIONS.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

/// A report reason: (key, label, needs a note).
type ReportReason = (String, String, bool);

/// Enabled report reasons for the report form.
static REPORT_REASONS: LazyLock<RwLock<Arc<Vec<ReportReason>>>> = LazyLock::new(Default::default);

pub fn set_report_reasons(list: Vec<(String, String, bool)>) {
    *REPORT_REASONS.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(list);
}

pub fn report_reasons() -> Arc<Vec<(String, String, bool)>> {
    REPORT_REASONS.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

static CURRENT: LazyLock<RwLock<Arc<ThemeSet>>> = LazyLock::new(Default::default);

/// Validates a design token name and value; returns a problem if there is one.
fn check_token(name: &str, value: &str) -> Result<(), String> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(format!("Token name {name:?} may only have letters, digits and dashes."));
    }
    if value.contains([';', '{', '}', '<', '\\']) || value.contains("url(") || value.contains("expression") {
        return Err(format!("Token {name} has characters that are not allowed."));
    }
    Ok(())
}

/// Style sheets may not break out into markup or pull anything in from elsewhere.
pub fn check_css(css: &str) -> Result<(), String> {
    let lower = css.to_ascii_lowercase();
    for bad in ["</style", "@import", "expression(", "javascript:", "behavior:"] {
        if lower.contains(bad) {
            return Err(format!("CSS may not contain {bad}"));
        }
    }
    Ok(())
}

fn renamed_token(old: &str) -> Option<&'static str> {
    Some(match old {
        "bg" => "color-bg",
        "surface" => "color-surface",
        "text" => "color-text",
        "muted" => "color-muted",
        "border" => "color-border",
        "accent" => "color-accent",
        "accent-text" => "color-accent-text",
        "header-bg" => "color-header-bg",
        "header-text" => "color-header-text",
        "focus" => "color-focus",
        "font" => "font-body",
        "wrap" => "content-width",
        _ => return None,
    })
}

fn vars(map: Option<&Value>) -> String {
    let mut out = String::new();
    for (k, v) in map.and_then(Value::as_object).into_iter().flatten() {
        if let Some(v) = v.as_str()
            && check_token(k, v).is_ok()
        {
            out.push_str(&format!("--bf-{k}: {v}; "));
            // Tokens saved under the old names keep working: components read the new ones.
            if let Some(new) = renamed_token(k) {
                out.push_str(&format!("--bf-{new}: {v}; "));
            }
        }
    }
    out
}

pub fn check_tokens(tokens: &Value) -> Result<(), String> {
    let extra = tokens.get("schemes").and_then(Value::as_object);
    let named = extra.into_iter().flatten().map(|(name, map)| (name.as_str(), map));
    for (name, map) in [("light", tokens.get("light")), ("dark", tokens.get("dark"))]
        .into_iter()
        .filter_map(|(n, m)| Some((n, m?)))
        .chain(named)
    {
        check_token(name, "x")?;
        for (k, v) in map.as_object().into_iter().flatten() {
            check_token(k, v.as_str().ok_or_else(|| format!("Token {k} must be text."))?)?;
        }
    }
    Ok(())
}

fn css_of(s: &Source) -> String {
    let mut css = String::new();
    // The schemes the theme chose, under its own tokens, which still win.
    if let Some((light, dark)) = &s.palettes {
        css.push_str(&format!(":root {{ {}}}\n", crate::schemes::declarations(light, dark)));
    }
    let light = vars(s.tokens.get("light"));
    if !light.is_empty() {
        css.push_str(&format!(":root {{ {light}}}\n"));
    }
    let dark = vars(s.tokens.get("dark"));
    if !dark.is_empty() {
        css.push_str(&format!(
            "@media (prefers-color-scheme: dark) {{ :root:not([data-theme=\"light\"]) {{ {dark}}} }}\n:root[data-theme=\"dark\"] {{ {dark}}}\n"
        ));
    }
    for (name, map) in s.tokens.get("schemes").and_then(Value::as_object).into_iter().flatten() {
        if check_token(name, "x").is_ok() {
            css.push_str(&format!(
                ":root[data-scheme=\"{name}\"], [data-scheme=\"{name}\"] {{ {}}}\n",
                vars(Some(map))
            ));
        }
    }
    if check_css(&s.css).is_ok() {
        css.push_str(&s.css);
    }
    css
}

/// Renders a slot template in a sandbox: no file access, bounded work, and sanitized output.
pub fn render_slot(source: &str, ctx: &Value) -> Result<String, String> {
    let mut env = minijinja::Environment::new();
    env.set_fuel(Some(50_000));
    env.set_recursion_limit(16);
    // Values are escaped however the template is named; the sanitizer below is the second wall.
    env.set_auto_escape_callback(|_| minijinja::AutoEscape::Html);
    env.add_template("slot", source).map_err(|e| e.to_string())?;
    let html = env
        .get_template("slot")
        .map_err(|e| e.to_string())?
        .render(minijinja::value::Serde(ctx))
        .map_err(|e| e.to_string())?;
    if html.len() > 64 * 1024 {
        return Err("slot output too large".into());
    }
    Ok(clean_html(&html))
}

/// What slots and plugin pages may contain: markup and classes, no scripts.
pub fn clean_html(html: &str) -> String {
    // `dir` and `lang` let a plugin mark text in another script (Arabic inside English) in a slot.
    ammonia::Builder::default().add_generic_attributes(["class", "dir"]).clean(html).to_string()
}

/// Input types a plugin page may use. No `file` (a plugin form cannot upload), no `password`, no `image`.
const PAGE_INPUT_TYPES: [&str; 17] = [
    "text", "search", "email", "url", "tel", "number", "range", "date", "datetime-local", "time", "month", "week",
    "color", "checkbox", "radio", "hidden", "submit",
];

/// Field names that would shadow a property of the form (`form.action`, `form.submit`) or the
/// host's own `_csrf` field.
fn clobbers_form(name: &str) -> bool {
    name.starts_with('_')
        || ["submit", "action", "method", "enctype", "target", "elements", "length", "reset", "id", "name", "attributes", "children", "style"]
            .contains(&name.to_ascii_lowercase().as_str())
}

/// What a plugin's own page (`/app/<plugin>/…`) may contain: everything a slot may, plus forms that post
/// back to the plugin's own pages. Never scripts, styles, file inputs or passwords; `data-app-*`
/// attributes only (the site's own `data-*` hooks drive its scripts and must not be forgeable);
/// `id`s and the `for` that points at them are prefixed `pp-`, so they cannot take over the site's.
pub fn clean_page_html(html: &str, plugin: &str) -> String {
    let own = format!("/app/{plugin}");
    let mut b = ammonia::Builder::default();
    b.add_tags(["form", "input", "textarea", "select", "option", "optgroup", "label", "button", "fieldset", "legend"]);
    b.add_generic_attributes(["class", "dir", "id"]);
    b.add_generic_attribute_prefixes(["data-app-", "aria-"]);
    b.add_tag_attributes("form", ["method", "action"]);
    b.add_tag_attributes(
        "input",
        [
            "type", "name", "value", "placeholder", "required", "disabled", "readonly", "checked", "maxlength", "minlength",
            "min", "max", "step", "pattern", "autocomplete", "size",
        ],
    );
    b.add_tag_attributes(
        "textarea",
        ["name", "rows", "cols", "placeholder", "required", "disabled", "readonly", "maxlength", "minlength", "wrap"],
    );
    b.add_tag_attributes("select", ["name", "required", "disabled", "multiple", "size"]);
    b.add_tag_attributes("option", ["value", "selected", "disabled", "label"]);
    b.add_tag_attributes("optgroup", ["label", "disabled"]);
    b.add_tag_attributes("label", ["for"]);
    b.add_tag_attributes("button", ["type", "name", "value", "disabled"]);
    b.add_tag_attributes("fieldset", ["disabled"]);
    b.attribute_filter(move |el, attr, value| {
        use std::borrow::Cow;
        match (el, attr) {
            (_, "id") | ("label", "for") => {
                let ok = !value.is_empty() && value.len() <= 60 && value.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_');
                ok.then(|| Cow::Owned(format!("pp-{value}")))
            }
            ("form", "method") => match value.to_ascii_lowercase().as_str() {
                "post" => Some(Cow::Borrowed("post")),
                "get" => Some(Cow::Borrowed("get")),
                _ => None,
            },
            ("form", "action") => {
                let rest = value.strip_prefix(own.as_str());
                let ok = rest.is_some_and(|r| r.is_empty() || r.starts_with('/') || r.starts_with('?')) && !value.contains("//") && !value.contains('\\');
                ok.then_some(Cow::Borrowed(value))
            }
            ("input", "type") => PAGE_INPUT_TYPES.contains(&value.to_ascii_lowercase().as_str()).then_some(Cow::Borrowed(value)),
            ("button", "type") => ["submit", "button", "reset"].contains(&value.to_ascii_lowercase().as_str()).then_some(Cow::Borrowed(value)),
            ("input" | "textarea" | "select" | "button", "name") => {
                let ok = !value.is_empty() && value.len() <= 60 && !clobbers_form(value)
                    && value.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b'[' | b']'));
                ok.then_some(Cow::Borrowed(value))
            }
            (_, a) if a.starts_with("data-app-") || a.starts_with("aria-") => (value.len() <= 500).then_some(Cow::Borrowed(value)),
            _ => Some(Cow::Borrowed(value)),
        }
    });
    b.clean(html).to_string()
}

/// Adds the host's CSRF token to every form that posts, just before it closes. The token is never
/// the plugin's to write: a field named `_csrf` is refused by `clean_page_html`.
pub fn add_csrf_fields(html: &str, token: &str) -> String {
    let field = format!("<input type=\"hidden\" name=\"_csrf\" value=\"{}\">", token.replace('"', ""));
    let mut out = String::with_capacity(html.len() + 256);
    let mut rest = html;
    while let Some(start) = rest.find("<form") {
        let Some(tag_end) = rest[start..].find('>').map(|i| start + i) else { break };
        let posts = rest[start..tag_end].contains("method=\"post\"");
        out.push_str(&rest[..=tag_end]);
        rest = &rest[tag_end + 1..];
        if posts && let Some(close) = rest.find("</form>") {
            out.push_str(&rest[..close]);
            out.push_str(&field);
            rest = &rest[close..];
        }
    }
    out.push_str(rest);
    out
}

/// Minifies and prefixes. CSS it cannot parse is served as written rather than lost.
fn minify(css: String) -> String {
    use lightningcss::stylesheet::{MinifyOptions, ParserOptions, PrinterOptions, StyleSheet};
    let out = (|| {
        let mut sheet = StyleSheet::parse(&css, ParserOptions::default()).ok()?;
        sheet.minify(MinifyOptions::default()).ok()?;
        sheet.to_css(PrinterOptions { minify: true, ..Default::default() }).ok().map(|r| r.code)
    })();
    out.unwrap_or(css)
}

fn compile(parts: &[&Source], site: &Value) -> Compiled {
    let css = minify(parts.iter().map(|s| css_of(s)).collect::<Vec<_>>().join("\n"));
    let mut partials = HashMap::new();
    let mut schemes = Vec::new();
    for s in parts {
        for (name, src) in s.slots.as_object().into_iter().flatten() {
            if let (Some(p), Some(src)) = (name.strip_prefix("partial:"), src.as_str())
                && PARTIALS.contains(&p)
            {
                partials.insert(p.to_owned(), src.to_owned());
            }
        }
        schemes
            .extend(s.tokens.get("schemes").and_then(Value::as_object).into_iter().flatten().map(|(n, _)| n.clone()));
    }
    let mut slots = HashMap::new();
    // Later parts win: a component overrides its theme.
    for s in parts {
        for (name, src) in s.slots.as_object().into_iter().flatten() {
            if !SLOTS.contains(&name.as_str()) {
                continue;
            }
            if let Some(src) = src.as_str() {
                match render_slot(src, site) {
                    Ok(html) => {
                        slots.insert(name.clone(), html);
                    }
                    // A broken template shows nothing, and the page still works.
                    Err(e) => tracing::warn!(theme = %s.name, slot = %name, error = %e, "theme slot failed"),
                }
            }
        }
    }
    let hash: String = Sha256::digest(css.as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect();
    Compiled { url: format!("/theme/{hash}.css"), css, slots, partials, schemes }
}

/// Rebuilds the active set from what is stored.
pub fn install(sources: &[Source], default: Option<i64>, site_name: &str, builtin_off: bool) {
    let site = serde_json::json!({ "site_name": site_name });
    let components: Vec<&Source> = sources.iter().filter(|s| s.component && s.enabled).collect();
    let mut set = ThemeSet { default, builtin_off, ..Default::default() };
    if !components.is_empty() {
        set.base = Some(Arc::new(compile(&components, &site)));
    }
    for t in sources.iter().filter(|s| !s.component) {
        let mut parts = vec![t];
        parts.extend(components.iter().copied());
        set.themes.insert(t.id, Arc::new(compile(&parts, &site)));
        if !t.enabled {
            set.disabled.insert(t.id);
        } else {
            set.order.push(t.id);
        }
        if !t.groups.is_empty() {
            set.groups.insert(t.id, t.groups.clone());
        }
        if t.enabled && t.selectable {
            set.choices.push((t.id, t.name.clone()));
        }
    }
    let all = set.themes.values().cloned().chain(set.base.clone());
    set.by_hash = all.map(|c| (c.url.clone(), c)).collect::<BTreeMap<_, _>>().into_iter().collect();
    *CURRENT.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(set);
}

fn current() -> Arc<ThemeSet> {
    CURRENT.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

/// Whether a member of `groups` may use the theme: it has no limit, or they are in one of its groups.
fn allowed(set: &ThemeSet, id: i64, groups: &[i64]) -> bool {
    set.groups.get(&id).is_none_or(|limit| limit.iter().any(|g| groups.contains(g)))
}

/// What to use for a page: the first theme out of the given preference order that exists and that
/// someone in `groups` may use, then the site's default; with the built-in look off, the first
/// theme there is; otherwise the components alone (the built-in look).
pub fn resolve(preferred: &[Option<i64>], groups: &[i64]) -> Option<Arc<Compiled>> {
    let set = current();
    let usable = |id: &&i64| !set.disabled.contains(id) && allowed(&set, **id, groups);
    preferred
        .iter()
        .flatten()
        .chain(set.default.iter())
        .filter(usable)
        .find_map(|id| set.themes.get(id).cloned())
        .or_else(|| set.builtin_off.then(|| set.order.iter().filter(usable).find_map(|id| set.themes.get(id).cloned())).flatten())
        .or_else(|| set.base.clone())
}

/// A theme by id even if disabled, for an admin previewing it.
pub fn preview(id: i64) -> Option<Arc<Compiled>> {
    current().themes.get(&id).cloned()
}

pub fn stylesheet(path: &str) -> Option<Arc<Compiled>> {
    current().by_hash.get(path).cloned()
}

/// The themes a member of `groups` can pick between.
pub fn choices(groups: &[i64]) -> Vec<(i64, String)> {
    let set = current();
    set.choices.iter().filter(|(id, _)| allowed(&set, *id, groups)).cloned().collect()
}

/// Whether the built-in look is offered, and whether a member of `groups` could end up on it.
pub fn builtin_off() -> bool {
    current().builtin_off
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn plugin_pages_get_safe_forms_and_no_scripts() {
        let dirty = r#"<form method="POST" action="/app/notes/new" onsubmit="x()"><input name="title" type="text" data-app-n="1" data-island="x" id="a">
            <input type="file" name="f"><input name="_csrf" value="x"><input name="action"><textarea name="body"></textarea>
            <button type="submit" formaction="/evil">Go</button></form>
            <form action="https://evil.test/"><input type="password" name="p"></form><script>alert(1)</script><p style="color:red" onclick="x()">hi</p>"#;
        let out = clean_page_html(dirty, "notes");
        assert!(out.contains(r#"action="/app/notes/new""#) && out.contains(r#"method="post""#), "{out}");
        assert!(out.contains(r#"data-app-n="1""#) && out.contains(r#"id="pp-a""#), "{out}");
        for gone in ["script", "onsubmit", "onclick", "style=", "formaction", "evil.test", "data-island", "_csrf", "name=\"action\"", "type=\"password\"", "type=\"file\""] {
            assert!(!out.contains(gone), "{gone} survived in {out}");
        }
        let posted = add_csrf_fields(&out, "tok");
        assert_eq!(posted.matches("name=\"_csrf\" value=\"tok\"").count(), 1, "only the post form gets a token: {posted}");
    }

    fn src(id: i64, component: bool, css: &str, slots: Value) -> Source {
        Source {
            id,
            name: format!("t{id}"),
            component,
            enabled: true,
            selectable: true,
            tokens: json!({"light":{"accent":"#123456"}}),
            css: css.into(),
            slots,
            palettes: None,
            groups: Vec::new(),
        }
    }

    #[test]
    fn a_theme_limited_to_groups_is_for_those_groups_and_the_built_in_look_can_be_off() {
        let open = src(1, false, ".open{a:1}", json!({}));
        let mut closed = src(2, false, ".closed{a:1}", json!({}));
        closed.groups = vec![7];
        let mut own = src(3, false, ".own{a:1}", json!({}));
        own.palettes = Some((json!({"accent":"#111111"}), json!({"accent":"#222222"})));
        // Not through `compile` alone: `install` and `resolve` are what pages use.
        let id = |t: Option<Arc<Compiled>>| t.map(|t| t.css.clone());
        install(&[open, closed, own], Some(2), "Site", false);
        // The site's default is limited to group 7: others fall back to the built-in look (no theme here).
        assert!(id(resolve(&[], &[1])).is_none());
        assert!(id(resolve(&[], &[7])).unwrap().contains(".closed{"));
        // A member's own pick of a limited theme does not work either.
        assert!(id(resolve(&[Some(2)], &[1])).is_none() && id(resolve(&[Some(1)], &[1])).unwrap().contains(".open{"));
        assert_eq!(choices(&[1]).iter().map(|c| c.0).collect::<Vec<_>>(), vec![1, 3]);
        assert_eq!(choices(&[7]).len(), 3);
        // With the built-in look off, someone who cannot use the default gets the first theme they can.
        install(
            &[src(1, false, ".open{a:1}", json!({})), { let mut c = src(2, false, ".closed{a:1}", json!({})); c.groups = vec![7]; c }],
            Some(2),
            "Site",
            true,
        );
        assert!(builtin_off());
        assert!(id(resolve(&[], &[1])).unwrap().contains(".open{"));
        // A theme's own schemes come before its tokens.
        let mut t = src(9, false, ".t{a:1}", json!({}));
        t.palettes = Some((json!({"accent":"#111111"}), json!({"accent":"#222222"})));
        let css = css_of(&t);
        assert!(css.starts_with(":root { --bf-color-bg:light-dark("), "{css}");
        assert!(css.find("--bf-color-accent:light-dark(#808080,#808080)").is_some() || css.contains("--bf-color-accent:"));
    }

    #[test]
    fn bad_tokens_and_css_are_refused() {
        assert!(check_tokens(&json!({"light":{"accent":"red; } body { display:none"}})).is_err());
        assert!(check_tokens(&json!({"light":{"bad name":"red"}})).is_err());
        assert!(check_tokens(&json!({"dark":{"accent":"url(https://x)"}})).is_err());
        assert!(check_tokens(&json!({"light":{"accent":"#fff"}})).is_ok());
        assert!(check_css("@IMPORT url(x)").is_err());
        assert!(check_css("</STYLE><script>").is_err());
        assert!(check_css(".a { color: red }").is_ok());
    }

    #[test]
    fn slots_are_sandboxed_and_sanitized() {
        let out = render_slot(
            "<b>{{ site_name }}</b><script>alert(1)</script><img src=x onerror=alert(1)>",
            &json!({"site_name":"Forum"}),
        )
        .unwrap();
        assert!(out.contains("<b>Forum</b>"), "{out}");
        assert!(!out.contains("script") && !out.contains("onerror"), "{out}");
        assert!(
            render_slot("{% for i in range(100000000) %}x{% endfor %}", &json!({})).is_err(),
            "fuel stops endless work"
        );
        assert!(render_slot("{% include '/etc/passwd' %}", &json!({})).is_err());
    }

    #[test]
    fn compiling_combines_theme_and_components() {
        let t = src(1, false, ".theme{color:red}", json!({"footer":"<i>t</i>"}));
        let c = src(2, true, ".comp{color:blue}", json!({"footer":"<i>c</i>"}));
        let out = compile(&[&t, &c], &json!({}));
        assert!(out.css.contains(".theme{") && out.css.contains(".comp{") && out.css.contains("--bf-accent:#123456"));
        assert_eq!(out.slots["footer"], "<i>c</i>", "the later part wins");
    }

    /// What a plugin's slot, panel or widget may carry: markup and classes, not code or styles.
    #[test]
    fn plugin_html_loses_styles_and_scripts() {
        let out = clean_html(
            "<style>a{color:red}</style><script>alert(1)</script><p class=\"x\" dir=\"rtl\" style=\"color:red\" id=\"i\" data-a=\"1\" onclick=\"f()\">hi</p>",
        );
        assert_eq!(out, "<p class=\"x\" dir=\"rtl\">hi</p>");
    }
}
