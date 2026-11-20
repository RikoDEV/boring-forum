//! What a package adds besides code and slot templates: translations (`locales/*.json`), style
//! sheets (`[client] css`), static assets (`assets/`), declarative syntax (`[[cook_rules]]`) and
//! buttons (`[[post_actions]]`, `[[composer_buttons]]`). They are checked here when the zip is
//! read and kept in the plugin's `schema` under `x-` keys, like the other manifest extras.

use base64::Engine as _;
use bf_core::i18n::en;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// One file of `assets/`.
pub(super) const MAX_ASSET: usize = 1 << 20;
/// All of `assets/` together, before base64.
pub(super) const MAX_ASSETS_TOTAL: usize = 3 << 20;
pub(super) const MAX_ASSET_FILES: usize = 100;
pub(super) const MAX_CSS: usize = 256 * 1024;
const MAX_LOCALES: usize = 20;
const MAX_LOCALE_FILE: usize = 256 * 1024;
const MAX_ACTIONS: usize = 10;
const MAX_CUSTOM_RULES: usize = 16;

#[derive(Deserialize)]
pub(super) struct CookRuleDecl {
    pub(super) open: String,
    pub(super) close: String,
    #[serde(default)]
    pub(super) html: String,
    #[serde(default)]
    pub(super) block: bool,
    /// Instead of a template: the plugin's `cook.rule` hook decides the HTML (a name for it).
    #[serde(default)]
    pub(super) callback: Option<String>,
}

/// `[[emoji]]`: an image from `assets/` usable as `:key:` in posts and in the composer's picker.
#[derive(Deserialize)]
pub(super) struct EmojiDecl {
    pub(super) key: String,
    #[serde(default)]
    pub(super) label: String,
    /// A file of `assets/`, like `assets/party.png`.
    pub(super) file: String,
    /// A group heading in the picker (a sticker pack, a theme).
    #[serde(default)]
    pub(super) set: String,
}

/// `[[widgets]]`: a block the page builder offers, drawn by `widget.render` with the values an
/// administrator filled in from `fields`.
#[derive(Deserialize)]
pub(super) struct WidgetDecl {
    pub(super) key: String,
    #[serde(default)]
    pub(super) label: String,
    #[serde(default)]
    pub(super) label_key: Option<String>,
    #[serde(default)]
    pub(super) description: String,
    #[serde(default)]
    pub(super) description_key: Option<String>,
    #[serde(default)]
    pub(super) fields: Vec<Value>,
}

/// `[[admin_pages]]`: a form page in the admin panel. The values are saved with the plugin's
/// settings, and `admin.form.save` (if the plugin listens) may refuse them.
#[derive(Deserialize)]
pub(super) struct AdminPageDecl {
    pub(super) key: String,
    #[serde(default)]
    pub(super) label: String,
    #[serde(default)]
    pub(super) label_key: Option<String>,
    #[serde(default)]
    pub(super) intro: String,
    #[serde(default)]
    pub(super) intro_key: Option<String>,
    #[serde(default)]
    pub(super) fields: Vec<Value>,
}

/// `[[post_actions]]` or `[[composer_buttons]]`: a button core draws, and the plugin's
/// `action.run` hook answers when it is pressed.
#[derive(Deserialize)]
pub(super) struct ActionDecl {
    pub(super) key: String,
    /// English text; `label_key` names its translation in `locales/`.
    #[serde(default)]
    pub(super) label: String,
    #[serde(default)]
    pub(super) label_key: Option<String>,
    #[serde(default)]
    pub(super) icon: String,
    /// What the answer does: `toast` (a message), `insert` (text into the composer) or `panel`
    /// (sanitized HTML in a dialog).
    #[serde(default = "toast")]
    pub(super) result: String,
}

fn toast() -> String {
    "toast".into()
}

#[derive(Default)]
pub(super) struct Extras {
    pub(super) locales: Map<String, Value>,
    /// Style sheets: path in the package → text.
    pub(super) css: Map<String, Value>,
    /// Files of `assets/`: path → `{type, b64}`.
    pub(super) assets: Map<String, Value>,
    pub(super) cook: Vec<Value>,
    pub(super) actions: Vec<Value>,
    /// Slots this plugin draws per viewer with `slot.render`.
    pub(super) dynamic: Vec<String>,
    pub(super) emoji: Vec<Value>,
    pub(super) widgets: Vec<Value>,
    pub(super) pages: Vec<Value>,
}

/// A non-empty key of lowercase letters, digits, dashes and the `more` characters.
pub(super) fn plain(k: &str, max: usize, more: &[char]) -> bool {
    !k.is_empty()
        && k.len() <= max
        && k.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || more.contains(&c))
}

/// `style.css`, `assets/logo.png`: letters, digits and `._-/`, no way out of the folder.
pub(super) fn safe_path(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 120
        && !p.starts_with('/')
        && !p.contains("..")
        && !p.contains("//")
        && p.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'))
}

pub(super) fn mime_for(path: &str) -> Option<&'static str> {
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        _ => return None,
    })
}

/// A style sheet may not reach other sites: the page's policy refuses it anyway, but a package
/// that tries is not one to install.
fn check_sheet(css: &str) -> Result<(), String> {
    bf_web::theme::check_css(css)?;
    let lower = css.to_ascii_lowercase().replace([' ', '"', '\''], "");
    for bad in ["url(http", "url(//", "url(javascript"] {
        if lower.contains(bad) {
            return Err(format!("CSS may not contain {bad}"));
        }
    }
    Ok(())
}

/// The English text of a label: its own `label`, else the `en` translation of `label_key`.
pub(super) fn english(label: &str, key: Option<&str>, locales: &Map<String, Value>) -> String {
    if !label.trim().is_empty() {
        return label.trim().to_owned();
    }
    key.and_then(|k| locales.get("en")?.get(k)?.as_str()).unwrap_or_default().to_owned()
}

type Reader<'a> = dyn FnMut(&str) -> Result<Option<Vec<u8>>, String> + 'a;

/// `names` are the archive's file names, `prefix` the folder wrapping them (see `archive_prefix`).
pub(super) fn read(
    m: &super::package::Manifest,
    names: &[String],
    prefix: &str,
    read: &mut Reader<'_>,
) -> Result<Extras, String> {
    let css_files: Vec<String> = m.client.as_ref().map(|c| c.css.clone()).unwrap_or_default();
    let (cook_rules, post_actions, composer_buttons) = (&m.cook_rules, &m.post_actions, &m.composer_buttons);
    let css_files = &css_files;
    let mut x = Extras::default();
    let under = |dir: &str| -> Vec<String> {
        let start = format!("{prefix}{dir}/");
        names
            .iter()
            .filter_map(|n| n.strip_prefix(&start))
            .filter(|n| !n.is_empty() && !n.ends_with('/'))
            .map(|n| format!("{dir}/{n}"))
            .collect()
    };
    // Translations: flat objects of key → text.
    let files = under("locales");
    if files.len() > MAX_LOCALES {
        return Err(en("Too many translation files.").into());
    }
    for file in files {
        let code = file.strip_prefix("locales/").and_then(|f| f.strip_suffix(".json")).unwrap_or_default();
        let ok = (2..=8).contains(&code.len()) && code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        if !ok {
            return Err(format!("{file}: name it after a language, like locales/pl.json."));
        }
        let bytes = read(&file)?.ok_or_else(|| format!("{file} is missing."))?;
        if bytes.len() > MAX_LOCALE_FILE {
            return Err(format!("{file} is too large."));
        }
        let doc: Value = serde_json::from_slice(&bytes).map_err(|e| format!("{file}: {e}"))?;
        let map = doc.as_object().ok_or_else(|| format!("{file} must be a JSON object."))?;
        if map.len() > 1000 || map.values().any(|v| v.as_str().is_none_or(|s| s.chars().count() > 2000)) {
            return Err(format!("{file} must map keys to texts of up to 2000 characters."));
        }
        x.locales.insert(code.to_owned(), doc);
    }
    // Style sheets named in the manifest.
    if css_files.len() > 8 {
        return Err(en("Too many style sheets.").into());
    }
    for path in css_files {
        if !safe_path(path) || !path.ends_with(".css") {
            return Err(format!("Bad style sheet path {path}."));
        }
        let bytes = read(path)?.ok_or_else(|| format!("{path} is missing."))?;
        if bytes.len() > MAX_CSS {
            return Err(format!("{path} is too large."));
        }
        let text = String::from_utf8(bytes).map_err(|_| format!("{path} is not text."))?;
        check_sheet(&text).map_err(|e| format!("{path}: {e}"))?;
        x.css.insert(path.clone(), Value::String(text));
    }
    // Static files: images and fonts only, because everything served from here is public and
    // `immutable`, and nothing a browser would run belongs in it.
    let files = under("assets");
    if files.len() > MAX_ASSET_FILES {
        return Err(en("Too many files in assets/.").into());
    }
    let mut total = 0;
    for file in files {
        let (Some(mime), true) = (mime_for(&file), safe_path(&file)) else {
            return Err(format!("{file}: only images and fonts can go in assets/."));
        };
        let bytes = read(&file)?.ok_or_else(|| format!("{file} is missing."))?;
        total += bytes.len();
        if bytes.len() > MAX_ASSET || total > MAX_ASSETS_TOTAL {
            return Err(format!("{file} is too large; assets/ may hold 1 MB a file and 3 MB in all."));
        }
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        x.assets.insert(file, json!({ "type": mime, "b64": b64 }));
    }
    if cook_rules.len() > MAX_CUSTOM_RULES {
        return Err(en("Too many cook rules.").into());
    }
    for r in cook_rules {
        let rule = bf_cook::ExtensionRule {
            open: r.open.clone(),
            close: r.close.clone(),
            html: r.html.clone(),
            block: r.block,
            callback: r.callback.clone(),
        };
        bf_cook::check_extension(&rule).map_err(|e| format!("[[cook_rules]] {}: {e}", r.open))?;
        x.cook.push(
            json!({ "open": r.open, "close": r.close, "html": r.html, "block": r.block, "callback": r.callback }),
        );
    }
    for (kind, list) in [("post", post_actions), ("composer", composer_buttons)] {
        if list.len() > MAX_ACTIONS {
            return Err(en("Too many buttons.").into());
        }
        for a in list {
            let label = english(&a.label, a.label_key.as_deref(), &x.locales);
            if !plain(&a.key, 40, &[])
                || label.is_empty()
                || label.chars().count() > 40
                || !matches!(a.result.as_str(), "toast" | "insert" | "panel")
            {
                return Err(format!("Bad button {}.", a.key));
            }
            if !a.icon.is_empty() && !plain(&a.icon, 40, &[]) {
                return Err(format!("Bad icon on button {}.", a.key));
            }
            x.actions.push(json!({ "kind": kind, "key": a.key, "label": label, "label_key": a.label_key, "icon": a.icon, "result": a.result }));
        }
    }
    read_declared(m, &mut x)?;
    Ok(x)
}

/// `[[emoji]]`, `[[widgets]]` and `[[admin_pages]]`, which need the translations read above.
fn read_declared(m: &super::package::Manifest, x: &mut Extras) -> Result<(), String> {
    if m.emoji.len() > 200 {
        return Err(en("Too many emoji.").into());
    }
    for e in &m.emoji {
        let name_ok = plain(&e.key, 32, &['_']);
        let is_image = mime_for(&e.file).is_some_and(|t| t.starts_with("image/"));
        if !name_ok || !e.file.starts_with("assets/") || !is_image || !x.assets.contains_key(&e.file) {
            return Err(format!("Bad emoji {}: it needs an image file in assets/.", e.key));
        }
        if m.emoji.iter().filter(|o| o.key == e.key).count() > 1 {
            return Err(format!("Emoji {} is declared twice.", e.key));
        }
        let label: String = if e.label.is_empty() { e.key.clone() } else { e.label.chars().take(60).collect() };
        x.emoji.push(
            json!({ "key": e.key, "label": label, "file": e.file, "set": e.set.chars().take(40).collect::<String>() }),
        );
    }
    if m.widgets.len() > 20 || m.admin_pages.len() > 20 {
        return Err(en("Too many widgets or admin pages.").into());
    }
    for w in &m.widgets {
        let label = english(&w.label, w.label_key.as_deref(), &x.locales);
        if !plain(&w.key, 40, &[]) || label.is_empty() || m.widgets.iter().filter(|o| o.key == w.key).count() > 1 {
            return Err(format!("Bad widget {}.", w.key));
        }
        let description = english(&w.description, w.description_key.as_deref(), &x.locales);
        x.widgets.push(json!({
            "key": w.key, "label": label, "label_key": w.label_key, "description": description,
            "description_key": w.description_key, "fields": super::forms::declared(&w.fields)?,
        }));
    }
    for p in &m.admin_pages {
        let label = english(&p.label, p.label_key.as_deref(), &x.locales);
        if !plain(&p.key, 40, &[]) || label.is_empty() || m.admin_pages.iter().filter(|o| o.key == p.key).count() > 1 {
            return Err(format!("Bad admin page {}.", p.key));
        }
        let intro = english(&p.intro, p.intro_key.as_deref(), &x.locales);
        x.pages.push(json!({
            "key": p.key, "label": label, "label_key": p.label_key, "intro": intro,
            "intro_key": p.intro_key, "fields": super::forms::declared(&p.fields)?,
        }));
    }
    Ok(())
}

impl Extras {
    /// Each part as stored in the plugin's schema.
    fn parts(&self) -> [(&'static str, Value); 9] {
        [
            ("x-locales", Value::Object(self.locales.clone())),
            ("x-css", Value::Object(self.css.clone())),
            ("x-assets", Value::Object(self.assets.clone())),
            ("x-cook", Value::Array(self.cook.clone())),
            ("x-actions", Value::Array(self.actions.clone())),
            ("x-dynamic", json!(self.dynamic)),
            ("x-emoji", Value::Array(self.emoji.clone())),
            ("x-widgets", Value::Array(self.widgets.clone())),
            ("x-admin-pages", Value::Array(self.pages.clone())),
        ]
    }

    pub(super) fn is_empty(&self) -> bool {
        self.parts().iter().all(|(_, v)| blank(v))
    }

    /// Bytes a signature covers, so none of this can change under a signed package.
    pub(super) fn signed_bytes(&self) -> Vec<u8> {
        if self.is_empty() {
            return Vec::new();
        }
        serde_json::to_vec(&json!([
            self.locales,
            self.css,
            self.assets,
            self.cook,
            self.actions,
            self.dynamic,
            self.emoji,
            self.widgets,
            self.pages
        ]))
        .unwrap_or_default()
    }

    pub(super) fn store(&self, doc: &mut Value) {
        for (key, value) in self.parts() {
            if !blank(&value) {
                doc[key] = value;
            }
        }
    }
}

fn blank(v: &Value) -> bool {
    v.as_object().map_or_else(|| v.as_array().is_none_or(Vec::is_empty), Map::is_empty)
}

/// Asks plugins (`cook.rule`) for the HTML of the text their `callback` cook rules matched, and
/// puts it where the cooker left a placeholder. A plugin that fails or says nothing leaves the
/// text as it was written. At most `MAX_CALLBACKS` per post.
pub(crate) async fn finish_callbacks(state: &crate::state::AppState, cooked: &mut bf_cook::Cooked) {
    for (i, (rule, text)) in std::mem::take(&mut cooked.callbacks).into_iter().enumerate().take(bf_cook::MAX_CALLBACKS)
    {
        let marker = format!("{}{i}{}", bf_cook::PLACEHOLDER_OPEN, bf_cook::PLACEHOLDER_CLOSE);
        let (plugin, name) = rule.split_once(':').unwrap_or(("", &rule));
        let owner = super::listening("cook.rule").into_iter().find(|p| p.id == plugin);
        let answer = match owner {
            Some(p) => super::call(state, p, "cook.rule", &json!({ "rule": name, "text": text })).await,
            None => None,
        };
        let html = answer
            .as_ref()
            .and_then(|a| a.get("html").and_then(Value::as_str))
            .map_or_else(|| text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;"), bf_cook::sanitize);
        cooked.html = cooked.html.replacen(&marker, &html, 1);
    }
    // Placeholders beyond the cap, or left by a failure: drop them.
    cooked.html.retain(|c| c != bf_cook::PLACEHOLDER_OPEN && c != bf_cook::PLACEHOLDER_CLOSE);
}

/// What enabled plugins add, gathered while `reload` walks them and published at its end.
#[derive(Default)]
pub(super) struct Activation {
    pub(super) css: Vec<String>,
    #[expect(clippy::type_complexity, reason = "plugin -> path -> (media type, bytes), used only here")]
    pub(super) assets: HashMap<String, HashMap<String, (String, Arc<Vec<u8>>)>>,
    pub(super) translations: HashMap<String, HashMap<String, String>>,
    pub(super) dynamic: HashSet<String>,
    pub(super) nav_items: Vec<bf_core::nav::NavItem>,
    /// Links with an icon or a menu, or an order: drawn by the layout, not the plain list.
    pub(super) nav_rich: Vec<bf_web::theme::PluginNav>,
    pub(super) emoji: Vec<EmojiEntry>,
    pub(super) dirs: HashMap<String, String>,
    pub(super) widgets: Vec<(String, String, Vec<Value>)>,
    pub(super) cook: Vec<bf_cook::ExtensionRule>,
    pub(super) actions: Vec<Value>,
    pub(super) strings: HashMap<String, Map<String, Value>>,
}

/// An emoji a plugin ships.
#[derive(Clone)]
pub(super) struct EmojiEntry {
    pub(super) name: String,
    pub(super) label: String,
    pub(super) set: String,
    pub(super) url: String,
}

static EMOJI: std::sync::LazyLock<std::sync::RwLock<Vec<EmojiEntry>>> = std::sync::LazyLock::new(Default::default);

/// Checks a settings object an administrator saved against the plugin's declared fields
/// (`settings.json` and `[[admin_pages]]`), then lets the plugin refuse it (`admin.form.save`).
pub(super) async fn check_settings(
    state: &crate::state::AppState,
    id: &str,
    values: Value,
) -> Result<Value, bf_core::AppError> {
    let pool = state.db()?;
    let all = bf_db::platform::plugins::summaries(pool).await.map_err(crate::http::render::db_err)?;
    let Some(schema) = all.iter().find(|p| p.id == id).and_then(|p| p.schema.clone()) else { return Ok(values) };
    let mut fields = super::forms::settings_fields(&schema);
    for page in schema.get("x-admin-pages").and_then(Value::as_array).into_iter().flatten() {
        fields.extend(page.get("fields").and_then(Value::as_array).cloned().unwrap_or_default());
    }
    let clean = super::forms::validate(&fields, &values)
        .map_err(|(template, key)| bf_core::AppError::invalid(template, &[("key", &key)]))?;
    if let Some(p) = super::listening("admin.form.save").into_iter().find(|p| p.id == id)
        && let Some(out) = super::call(state, p, "admin.form.save", &json!({ "values": clean })).await
        && let Some(reason) = out.get("reject").and_then(Value::as_str)
    {
        return Err(bf_core::AppError::Validation(reason.chars().take(300).collect()));
    }
    Ok(clean)
}

/// The widget kinds enabled plugins declare: (plugin, key, fields).
#[expect(clippy::type_complexity, reason = "a private table of (plugin, key, fields)")]
static WIDGETS: std::sync::LazyLock<std::sync::RwLock<Vec<(String, String, Vec<Value>)>>> =
    std::sync::LazyLock::new(Default::default);

/// A plugin widget's config, with the values for its declared settings checked and coerced
/// (`config.settings`). Widgets of a plugin that declares none are left as they are.
pub(crate) fn check_widget_config(mut config: Value) -> Result<Value, bf_core::AppError> {
    let (Some(plugin), Some(key)) = (
        config.get("plugin").and_then(Value::as_str).map(str::to_owned),
        config.get("key").and_then(Value::as_str).map(str::to_owned),
    ) else {
        return Ok(config);
    };
    let fields = WIDGETS
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .find(|(p, k, _)| *p == plugin && *k == key)
        .map(|(_, _, f)| f.clone());
    let Some(fields) = fields else { return Ok(config) };
    let settings = config.get("settings").cloned().unwrap_or_else(|| json!({}));
    let clean = super::forms::validate(&fields, &settings)
        .map_err(|(template, field)| bf_core::AppError::invalid(template, &[("key", &field)]))?;
    config["settings"] = clean;
    Ok(config)
}

/// Where each plugin's files are served: its `<id>.<hash>` folder.
static DIRS: std::sync::LazyLock<std::sync::RwLock<HashMap<String, String>>> =
    std::sync::LazyLock::new(Default::default);

pub(super) fn dir_of(plugin: &str) -> Option<String> {
    DIRS.read().unwrap_or_else(std::sync::PoisonError::into_inner).get(plugin).cloned()
}

/// Plugin emoji for the composer's picker and `/api/v1/emoji`: (name, address, group, label).
pub(crate) fn emoji_entries() -> Vec<(String, String, String, String)> {
    let all = EMOJI.read().unwrap_or_else(std::sync::PoisonError::into_inner);
    all.iter().map(|e| (e.name.clone(), e.url.clone(), e.set.clone(), e.label.clone())).collect()
}

/// Each plugin's translations by language code, for the texts it draws itself.
static STRINGS: std::sync::LazyLock<std::sync::RwLock<HashMap<String, Map<String, Value>>>> =
    std::sync::LazyLock::new(Default::default);

/// A plugin's texts for one language: English, with that language's own on top.
pub(super) fn strings_for(plugin: &str, locale: &str) -> Value {
    let all = STRINGS.read().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(locales) = all.get(plugin) else { return json!({}) };
    let mut out = locales.get("en").and_then(Value::as_object).cloned().unwrap_or_default();
    if let Some(own) = locales.get(locale).and_then(Value::as_object) {
        out.extend(own.clone());
    }
    Value::Object(out)
}

impl Activation {
    /// Takes one enabled plugin's stored extras.
    pub(super) fn add(&mut self, id: &str, schema: Option<&Value>, hooks: &[String]) {
        let Some(schema) = schema else { return };
        let listens = |h: &str| hooks.iter().any(|x| x == h);
        // Labels are written in English and found again by that text, like the core's own.
        let locales = schema.get("x-locales").and_then(Value::as_object);
        if let Some(l) = locales {
            self.strings.insert(id.to_owned(), l.clone());
        }
        let mut keyed: Vec<(String, String)> = Vec::new();
        for list in ["x-panels", "x-nav", "x-actions"] {
            for item in schema.get(list).and_then(Value::as_array).into_iter().flatten() {
                let key = item.get("label_key").and_then(Value::as_str);
                let label = item.get("label").and_then(Value::as_str).unwrap_or_default();
                if let Some(key) = key.filter(|_| !label.is_empty()) {
                    keyed.push((label.to_owned(), key.to_owned()));
                }
                let menu = item.get("menu").and_then(Value::as_str).unwrap_or_default();
                if let Some(key) = item.get("menu_key").and_then(Value::as_str).filter(|_| !menu.is_empty()) {
                    keyed.push((menu.to_owned(), key.to_owned()));
                }
            }
        }
        for (code, map) in locales.into_iter().flatten().filter(|(c, _)| c.as_str() != "en") {
            for (label, key) in &keyed {
                if let Some(text) = map.get(key).and_then(Value::as_str) {
                    self.translations.entry(code.clone()).or_default().insert(label.clone(), text.to_owned());
                }
            }
        }
        // Files: served under a folder named by what is in them, so they can be cached forever.
        let mut files: Vec<(String, String, Vec<u8>)> = Vec::new();
        for (path, text) in schema.get("x-css").and_then(Value::as_object).into_iter().flatten() {
            let text = text.as_str().unwrap_or_default();
            files.push((
                path.clone(),
                "text/css; charset=utf-8".into(),
                format!(
                    "@layer plugin {{
{text}
}}"
                )
                .into_bytes(),
            ));
        }
        for (path, a) in schema.get("x-assets").and_then(Value::as_object).into_iter().flatten() {
            let bytes = a
                .get("b64")
                .and_then(Value::as_str)
                .and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok());
            if let (Some(bytes), Some(mime)) = (bytes, a.get("type").and_then(Value::as_str)) {
                files.push((path.clone(), mime.to_owned(), bytes));
            }
        }
        if !files.is_empty() {
            files.sort_by(|a, b| a.0.cmp(&b.0));
            let mut h = Sha256::new();
            for (path, _, bytes) in &files {
                h.update(path.as_bytes());
                h.update((bytes.len() as u64).to_be_bytes());
                h.update(bytes);
            }
            let hash: String = h.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect();
            let dir = format!("{id}.{hash}");
            for (path, _, _) in files.iter().filter(|f| f.0.ends_with(".css")) {
                self.css.push(format!("/plugins/{dir}/{path}"));
            }
            self.dirs.insert(id.to_owned(), dir.clone());
            self.assets.insert(dir, files.into_iter().map(|(p, m, b)| (p, (m, Arc::new(b)))).collect());
        }
        if listens("slot.render") {
            let slots =
                schema.get("x-dynamic").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str);
            self.dynamic.extend(
                slots.filter(|s| bf_web::theme::SLOTS.contains(s) || *s == bf_web::theme::CARD_SLOT).map(str::to_owned),
            );
        }
        // Links with a visibility rule; the plain ones are added by `reload` itself.
        // Links with an icon, a menu or an order are drawn by the layout (`PluginNav`); the rest of
        // those with a visibility rule go in `nav_items`.
        for raw in schema.get("x-nav").and_then(Value::as_array).into_iter().flatten() {
            let Some(item) = bf_core::nav::parse(&Value::Array(vec![raw.clone()]).to_string()).into_iter().next()
            else {
                continue;
            };
            if item.key.is_some() {
                continue;
            }
            let text = |k: &str| raw.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
            let rich = !text("icon").is_empty() || !text("menu").is_empty() || raw.get("order").is_some();
            if rich {
                self.nav_rich.push(bf_web::theme::PluginNav {
                    item,
                    icon: text("icon"),
                    menu: text("menu"),
                    order: raw.get("order").and_then(Value::as_i64).unwrap_or(100),
                });
            } else if item.group.is_some() || !item.groups.is_empty() {
                self.nav_items.push(item);
            }
        }
        for w in schema.get("x-widgets").and_then(Value::as_array).into_iter().flatten() {
            let fields = w.get("fields").and_then(Value::as_array).cloned().unwrap_or_default();
            self.widgets.push((
                id.to_owned(),
                w.get("key").and_then(Value::as_str).unwrap_or_default().to_owned(),
                fields,
            ));
        }
        for e in schema.get("x-emoji").and_then(Value::as_array).into_iter().flatten() {
            let text = |k: &str| e.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
            self.emoji.push(EmojiEntry {
                name: text("key"),
                label: text("label"),
                set: text("set"),
                url: format!("/plugin-emoji/{id}/{}", text("file")),
            });
        }
        for r in schema.get("x-cook").and_then(Value::as_array).into_iter().flatten() {
            let text = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
            self.cook.push(bf_cook::ExtensionRule {
                open: text("open"),
                close: text("close"),
                html: text("html"),
                block: r.get("block").and_then(Value::as_bool).unwrap_or(false),
                // Named per plugin, so the answer goes back to the plugin that asked.
                callback: r.get("callback").and_then(Value::as_str).map(|n| format!("{id}:{n}")),
            });
        }
        if listens("action.run") {
            for a in schema.get("x-actions").and_then(Value::as_array).into_iter().flatten() {
                let mut a = a.clone();
                a["plugin"] = json!(id);
                self.actions.push(a);
            }
        }
    }

    /// Makes everything gathered live. When the cooker's rules or emoji differ from what posts
    /// were last cooked with, every post is marked and the re-cook job rebuilds them.
    pub(super) fn publish(self, state: &crate::state::AppState) {
        let digest = if self.cook.is_empty() && self.emoji.is_empty() {
            String::new()
        } else {
            let mut h = Sha256::new();
            for r in &self.cook {
                h.update(format!("{r:?}"));
            }
            for e in &self.emoji {
                h.update(format!("{}={}", e.name, e.url));
            }
            h.finalize().iter().map(|b| format!("{b:02x}")).collect()
        };
        let emoji_map: std::collections::BTreeMap<String, String> =
            self.emoji.iter().map(|e| (e.name.clone(), e.url.clone())).collect();
        bf_cook::set_plugin_emoji(emoji_map);
        *EMOJI.write().unwrap_or_else(std::sync::PoisonError::into_inner) = self.emoji;
        *DIRS.write().unwrap_or_else(std::sync::PoisonError::into_inner) = self.dirs;
        *WIDGETS.write().unwrap_or_else(std::sync::PoisonError::into_inner) = self.widgets;
        bf_cook::set_extensions(self.cook);
        if let Some(pool) = state.pool.clone() {
            let state = state.clone();
            tokio::spawn(async move {
                match bf_db::platform::cook_state::swap_digest(&pool, &digest).await {
                    Ok(true) => {
                        tracing::info!("plugin cook rules or emoji changed: posts will be re-cooked");
                        if bf_db::admin::maintenance::mark_all_for_recook(&pool).await.is_ok() {
                            crate::state::recook(state).await;
                        }
                    }
                    Ok(false) => {}
                    Err(e) => tracing::warn!(error = %e, "plugin cook state not saved"),
                }
            });
        }
        *STRINGS.write().unwrap_or_else(std::sync::PoisonError::into_inner) = self.strings;
        *super::ASSETS.write().unwrap_or_else(std::sync::PoisonError::into_inner) = self.assets;
        bf_web::theme::set_plugin_extras(bf_web::theme::PluginExtras {
            css: self.css,
            translations: self.translations,
            dynamic_slots: self.dynamic,
            nav_items: self.nav_items,
            nav_rich: self.nav_rich,
            actions: self.actions,
        });
        super::routes::forget_slots();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_inside_the_package() {
        assert!(safe_path("assets/logo.png") && safe_path("style.css"));
        for bad in ["", "/etc/passwd", "../x.css", "a//b", "a\\b", "a b.css", "assets/..png"] {
            assert!(!safe_path(bad), "{bad}");
        }
        assert_eq!(mime_for("a/b.WOFF2"), Some("font/woff2"));
        assert_eq!(mime_for("a.html"), None);
        assert_eq!(mime_for("a.js"), None);
    }

    #[test]
    fn sheets_cannot_reach_other_sites() {
        assert!(check_sheet("a { background: url(assets/a.png) }").is_ok());
        assert!(check_sheet("a { background: url( 'https://x.test/a.png' ) }").is_err());
        assert!(check_sheet("@import 'x.css';").is_err());
        assert!(check_sheet("a { background: url(//x.test/a.png) }").is_err());
    }

    #[test]
    fn labels_come_from_the_english_file() {
        let mut locales = Map::new();
        locales.insert("en".into(), json!({ "k": "Hello" }));
        assert_eq!(english("", Some("k"), &locales), "Hello");
        assert_eq!(english("Own", Some("k"), &locales), "Own");
        assert_eq!(english("", Some("missing"), &locales), "");
    }
}
