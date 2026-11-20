//! Message catalogs: Fluent (`.ftl`) files embedded in the binary and formatted at runtime
//! by `fluent-bundle`. A message id is its catalog key with the dots written as hyphens
//! (`nav.forums` is `nav-forums`); a plural is one message with a `$n` select. Missing keys fall back
//! to English, then to the key itself, so a gap is visible but never breaks a page.
//!
//! `compile` turns the same messages into a small JSON tree for the browser (`T::messages`), so the
//! script formats with the same plural rules, selects and references as Rust.

pub mod badges;
mod compile;

use compile::{Ast, Chain, Part};
use fluent_bundle::concurrent::FluentBundle;
use fluent_bundle::{FluentArgs, FluentResource, FluentValue};
use fluent_syntax::ast::{Entry, Message};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, LazyLock, RwLock};

/// Supported locales: (code, native name, catalog source).
const LOCALES: [(&str, &str, &str); 3] = [
    ("en", "English", include_str!("../../locales/en.ftl")),
    ("pl", "Polski", include_str!("../../locales/pl.ftl")),
    ("ar", "العربية", include_str!("../../locales/ar.ftl")),
];

/// One message after loading: its compiled tree (or why it could not be) and the variables it uses.
struct Compiled {
    parts: Result<Vec<Part>, String>,
    vars: Vec<(String, bool)>,
}

/// One locale: the Fluent bundle that formats Rust output, and the parsed messages the browser
/// compiler and the `t()` placeholder logic read.
struct Loc {
    bundle: FluentBundle<FluentResource>,
    ast: Ast,
    compiled: HashMap<String, Compiled>,
}

struct Bundles {
    locales: HashMap<&'static str, Loc>,
}

/// An admin override row: (locale, key, Fluent pattern source).
type Row = (String, String, String);

// The catalogs are compiled into the binary: a broken one is a build mistake, found by the
// `cargo test -p bf-core i18n` run in CI and by the first start, never by a request.
#[expect(clippy::panic, reason = "startup: the embedded catalogs are static and checked by tests")]
static BASE: LazyLock<Arc<Bundles>> = LazyLock::new(|| Arc::new(build(&[]).unwrap_or_else(|e| panic!("locales: {e}"))));

/// The catalogs in use: the base ones until the admin's overrides are loaded.
static STATE: LazyLock<RwLock<Arc<Bundles>>> = LazyLock::new(|| RwLock::new(BASE.clone()));

/// English texts that live in the database, not in the catalogs (a custom badge's name): they are
/// registered here so `/admin/text` can list them and an override can translate them. See
/// `set_site_texts`; `build` reads them on every rebuild.
static SITE_TEXTS: LazyLock<RwLock<Vec<String>>> = LazyLock::new(Default::default);

/// The longest database text offered for translation.
const SITE_TEXT_MAX: usize = 4000;

/// Replaces the registered database texts; true when they changed. Call `set_overrides` after it
/// to apply them.
pub fn set_site_texts(texts: impl IntoIterator<Item = String>) -> bool {
    let mut all: Vec<String> = texts
        .into_iter()
        // Not trimmed: a text is looked up by its exact stored form.
        .filter(|t| !t.trim().is_empty() && t.chars().count() <= SITE_TEXT_MAX)
        .collect();
    all.sort_unstable();
    all.dedup();
    let mut current = SITE_TEXTS.write().unwrap_or_else(std::sync::PoisonError::into_inner);
    let changed = *current != all;
    *current = all;
    changed
}

/// Registered database texts the catalogs do not already have, as `(key, English)`.
pub fn site_catalog() -> Vec<(String, String)> {
    let base = &BASE.locales["en"].ast.messages;
    let texts = SITE_TEXTS.read().unwrap_or_else(std::sync::PoisonError::into_inner);
    texts.iter().map(|t| (msg_key(t), t.clone())).filter(|(k, _)| !base.contains_key(&id_of(k))).collect()
}

/// `site_catalog` against the catalog being built.
fn site_catalog_in(en: &Ast, site: &[String]) -> Vec<(String, String)> {
    site.iter().map(|t| (msg_key(t), t.clone())).filter(|(k, _)| !en.messages.contains_key(&id_of(k))).collect()
}

/// A stored text as an override value: itself when Fluent reads it as plain text, else as a
/// string literal, so braces, brackets and the like in it stay text.
pub fn plain_pattern(text: &str) -> String {
    let special = text.contains(['{', '}'])
        || text.lines().any(|l| l.trim_start().starts_with(['[', '*', '.']))
        || text.trim() != text;
    if special { literal(text) } else { text.to_owned() }
}

/// A text as a Fluent string literal, so braces and brackets in it stay text.
fn literal(text: &str) -> String {
    // A string literal cannot hold a raw line break; `\u000A` is one.
    let body = text
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "\\u000A");
    format!("{{ \"{body}\" }}")
}

fn state() -> Arc<Bundles> {
    STATE.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

fn id_of(key: &str) -> String {
    key.replace('.', "-")
}

fn key_of(id: &str) -> String {
    id.replace('-', ".")
}

/// `id = <source>` with continuation lines indented, the form an override is parsed in.
fn wrap(id: &str, source: &str) -> String {
    if !source.contains('\n') {
        return format!("{id} = {source}\n");
    }
    let mut out = format!("{id} =\n");
    for line in source.lines() {
        if !line.trim().is_empty() {
            out.push_str("    ");
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

/// A parse error in words an admin can act on.
fn describe(kind: &fluent_syntax::parser::ErrorKind) -> String {
    use fluent_syntax::parser::ErrorKind as K;
    match kind {
        K::ExpectedCharRange { .. } => "something is missing here, such as a closing } or a line break".into(),
        other => other.to_string(),
    }
}

/// Parses a pattern written as an override into its AST.
fn parse_override(source: &str) -> Result<compile::Pat, String> {
    let wrapped = wrap("x", source);
    let resource = fluent_syntax::parser::parse_runtime(wrapped.clone()).map_err(|(_, errors)| {
        // The line in what the admin typed: `wrap` adds a first line for a multi-line text.
        let last = source.lines().count().max(1);
        let header = usize::from(source.contains('\n'));
        errors
            .iter()
            .map(|e| {
                let line = wrapped
                    .get(..e.pos.start)
                    .map_or(1, |s| s.matches('\n').count() + 1)
                    .saturating_sub(header)
                    .clamp(1, last);
                format!("line {line}: {}", describe(&e.kind))
            })
            .collect::<Vec<_>>()
            .join("; ")
    })?;
    let mut messages = resource.body.into_iter().filter_map(|e| match e {
        Entry::Message(m) => Some(m),
        _ => None,
    });
    match (messages.next(), messages.next()) {
        (Some(Message { value: Some(pattern), attributes, .. }), None) if attributes.is_empty() => Ok(pattern),
        _ => Err("expected one text, without attributes".into()),
    }
}

fn build(overrides: &[Row]) -> Result<Bundles, String> {
    build_with(overrides, &[])
}

/// `build` with database texts registered as English messages of their own.
fn build_with(overrides: &[Row], site: &[String]) -> Result<Bundles, String> {
    let mut locales: HashMap<&'static str, Loc> = HashMap::new();
    for (code, _, src) in LOCALES {
        let resource = fluent_syntax::parser::parse_runtime(src.to_owned())
            .map_err(|(_, errors)| format!("locales/{code}.ftl: {errors:?}"))?;
        let mut ast = Ast::default();
        for entry in resource.body {
            match entry {
                Entry::Message(Message { id, value: Some(pattern), .. }) => {
                    ast.messages.insert(id.name, pattern);
                }
                Entry::Term(term) => {
                    ast.terms.insert(term.id.name, term.value);
                }
                _ => {}
            }
        }
        let langid = code.parse().map_err(|e| format!("locale {code}: {e}"))?;
        let mut bundle = FluentBundle::new_concurrent(vec![langid]);
        bundle.set_use_isolating(false);
        let fluent = FluentResource::try_new(src.to_owned()).map_err(|(_, e)| format!("locales/{code}.ftl: {e:?}"))?;
        bundle.add_resource(fluent).map_err(|e| format!("locales/{code}.ftl: {e:?}"))?;
        locales.insert(code, Loc { bundle, ast, compiled: HashMap::new() });
    }

    // Database texts: English messages of their own, so overrides can translate them.
    let mut site_doc = String::new();
    for (key, text) in site_catalog_in(&locales["en"].ast, site) {
        let id = id_of(&key);
        let Ok(pattern) = parse_override(&literal(&text)) else { continue };
        site_doc.push_str(&wrap(&id, &literal(&text)));
        if let Some(loc) = locales.get_mut("en") {
            loc.ast.messages.insert(id, pattern);
        }
    }
    if !site_doc.is_empty() {
        match FluentResource::try_new(site_doc) {
            Ok(resource) => {
                if let Some(loc) = locales.get_mut("en") {
                    loc.bundle.add_resource_overriding(resource);
                }
            }
            Err((_, e)) => tracing::warn!("database texts skipped: {e:?}"),
        }
    }

    // Overrides: each replaces one whole message of one locale. One that does not parse and compile
    // is skipped, as is a stale row whose key is no longer in the English catalog.
    let mut docs: HashMap<&'static str, String> = HashMap::new();
    for (locale, key, source) in overrides {
        let id = id_of(key);
        let Some((&code, _)) = locales.get_key_value(locale.as_str()) else { continue };
        if !locales["en"].ast.messages.contains_key(&id) {
            continue;
        }
        let chain = Chain { first: &locales[code].ast, second: Some(&locales["en"].ast) };
        let checked = parse_override(source).and_then(|p| compile::compile(&p, chain).map(|_| p));
        match checked {
            Ok(pattern) => {
                docs.entry(code).or_default().push_str(&wrap(&id, source));
                if let Some(loc) = locales.get_mut(code) {
                    loc.ast.messages.insert(id, pattern);
                }
            }
            Err(e) => tracing::warn!(locale, key, "text override skipped: {e}"),
        }
    }
    for (code, doc) in docs {
        match FluentResource::try_new(doc) {
            Ok(resource) => {
                if let Some(loc) = locales.get_mut(code) {
                    loc.bundle.add_resource_overriding(resource);
                }
            }
            Err((_, e)) => tracing::warn!(code, "text overrides skipped: {e:?}"),
        }
    }

    // Compile every message, with English behind each translation.
    let mut compiled: Vec<(&'static str, HashMap<String, Compiled>)> = Vec::new();
    for (code, loc) in &locales {
        let fallback = if *code == "en" { None } else { Some(&locales["en"].ast) };
        compiled.push((code, compile_all(&loc.ast, fallback)));
    }
    for (code, map) in compiled {
        if let Some(loc) = locales.get_mut(code) {
            loc.compiled = map;
        }
    }
    Ok(Bundles { locales })
}

fn compile_all(ast: &Ast, fallback: Option<&Ast>) -> HashMap<String, Compiled> {
    let chain = Chain { first: ast, second: fallback };
    ast.messages
        .iter()
        .map(|(id, pattern)| {
            let parts = compile::compile(pattern, chain);
            let vars = parts.as_ref().map(|p| compile::variables(p)).unwrap_or_default();
            (id.clone(), Compiled { parts, vars })
        })
        .collect()
}

impl Bundles {
    /// Formats `key` in `locale` (English behind it). Variables the message uses that `given` does
    /// not supply are rendered as the literal `{name}`, for a caller that fills them afterwards.
    fn render(&self, locale: &str, key: &str, given: &[(&str, &str)], count: Option<i64>) -> Option<String> {
        let id = id_of(key);
        for code in [locale, "en"] {
            let Some(loc) = self.locales.get(code) else { continue };
            let Some(compiled) = loc.compiled.get(&id) else { continue };
            let Some(pattern) = loc.bundle.get_message(&id).and_then(|m| m.value()) else { continue };
            let mut args = FluentArgs::new();
            for (name, numeric) in &compiled.vars {
                let value = match (count, given.iter().find(|(k, _)| k == name)) {
                    (Some(n), _) if name == "n" => FluentValue::from(n),
                    (_, Some((_, v))) => match v.parse::<f64>() {
                        Ok(x) if *numeric && x.is_finite() => match v.parse::<i64>() {
                            Ok(i) => FluentValue::from(i),
                            Err(_) => FluentValue::from(x),
                        },
                        _ => FluentValue::from((*v).to_owned()),
                    },
                    _ => FluentValue::from(format!("{{{name}}}")),
                };
                args.set(name.clone(), value);
            }
            let mut errors = Vec::new();
            return Some(loc.bundle.format_pattern(pattern, Some(&args), &mut errors).into_owned());
        }
        None
    }

    fn compiled(&self, locale: &str, id: &str) -> Option<&Compiled> {
        self.locales.get(locale).and_then(|l| l.compiled.get(id)).or_else(|| self.locales["en"].compiled.get(id))
    }

    fn messages(&self, locale: &str, prefix: &str) -> BTreeMap<String, Value> {
        let mut out = BTreeMap::new();
        for id in self.locales["en"].compiled.keys() {
            let key = key_of(id);
            if !key.starts_with(prefix) {
                continue;
            }
            match self.compiled(locale, id).map(|c| &c.parts) {
                Some(Ok(parts)) => {
                    out.insert(key, compile::to_json(parts));
                }
                Some(Err(e)) => tracing::warn!(key, "message cannot be sent to the browser: {e}"),
                None => {}
            }
        }
        out
    }
}

/// Replaces the admin's text overrides: (locale, key, Fluent pattern source). See `build`.
pub fn set_overrides(rows: &[(String, String, String)]) {
    let site = SITE_TEXTS.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    match build_with(rows, &site) {
        Ok(bundles) => *STATE.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(bundles),
        Err(e) => tracing::error!("text overrides not loaded: {e}"),
    }
}

/// Every message key with its English pattern as written after `id =`, dedented, for the admin
/// editor. Sorted by key.
pub fn catalog() -> Vec<(String, String)> {
    let mut all: Vec<(String, String)> =
        BASE.locales["en"].ast.messages.iter().map(|(id, pattern)| (key_of(id), source_of(id, pattern))).collect();
    all.sort_unstable();
    all
}

/// The built-in text of every message in `locale` (English where the language has none), as
/// `catalog` writes it, ignoring any admin override. Keyed like `catalog`, same order.
pub fn catalog_for(locale: &str) -> Vec<(String, String)> {
    let loc = BASE.locales.get(locale).unwrap_or(&BASE.locales["en"]);
    let mut all: Vec<(String, String)> = BASE.locales["en"]
        .ast
        .messages
        .iter()
        .map(|(id, english)| (key_of(id), source_of(id, loc.ast.messages.get(id).unwrap_or(english))))
        .collect();
    all.sort_unstable();
    all
}

fn source_of(id: &str, pattern: &compile::Pat) -> String {
    let message = Message {
        id: fluent_syntax::ast::Identifier { name: id.to_owned() },
        value: Some(pattern.clone()),
        attributes: vec![],
        comment: None,
    };
    let resource = fluent_syntax::ast::Resource { body: vec![Entry::Message(message)] };
    let out = fluent_syntax::serializer::serialize(&resource);
    let rest = out.strip_prefix(&format!("{id} =")).unwrap_or(&out).trim_end_matches('\n');
    if let Some(single) = rest.strip_prefix(' ').filter(|r| !r.contains('\n')) {
        return single.to_owned();
    }
    let body = rest.trim_start_matches('\n');
    let indent =
        body.lines().filter(|l| !l.trim().is_empty()).map(|l| l.len() - l.trim_start().len()).min().unwrap_or(0);
    body.lines().map(|l| l.get(indent..).unwrap_or("")).collect::<Vec<_>>().join("\n")
}

/// Whether `source` is acceptable as an override: valid Fluent, one text, and only features the
/// browser can render. The error is shown to the admin.
pub fn validate_override(source: &str) -> Result<(), String> {
    if source.trim().is_empty() {
        return Err("The text is empty.".into());
    }
    let pattern = parse_override(source)?;
    let en = &BASE.locales["en"].ast;
    compile::compile(&pattern, Chain { first: en, second: None })?;
    Ok(())
}

/// The text of a Fluent string literal: `\\`, `\"` and `\uXXXX` escapes resolved.
fn string_literal(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                out.extend(u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32));
            }
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// (code, native name) of every supported locale, for the language picker.
pub fn locales() -> Vec<(&'static str, &'static str)> {
    LOCALES.iter().map(|(c, n, _)| (*c, *n)).collect()
}

pub fn supported(code: &str) -> bool {
    LOCALES.iter().any(|(c, _, _)| *c == code)
}

/// The site's own language from the `default_locale` setting: that language, or English when the
/// setting is `browser`. Visitors' languages then follow their browsers, but mail to someone with
/// no language of their own, AI text and the like still need one.
pub fn site_language(setting: &str) -> &str {
    if supported(setting) { setting } else { "en" }
}

/// Picks the first supported language from an `Accept-Language` header, by q value.
pub fn from_accept_language(header: &str) -> Option<&'static str> {
    let mut langs: Vec<(f32, &str)> = header
        .split(',')
        .filter_map(|part| {
            let mut it = part.trim().split(';');
            let tag = it.next()?.trim();
            let q = it.find_map(|p| p.trim().strip_prefix("q=")).and_then(|q| q.parse().ok()).unwrap_or(1.0);
            Some((q, tag))
        })
        .collect();
    langs.sort_by(|a, b| b.0.total_cmp(&a.0));
    langs.into_iter().find_map(|(_, tag)| {
        let primary = tag.split('-').next()?.to_ascii_lowercase();
        LOCALES.iter().find(|(c, _, _)| *c == primary).map(|(c, _, _)| *c)
    })
}

/// Marks English text for translation where it is written, for text that is translated later,
/// where it is shown (`T::msg` on a page's message). Returns the text unchanged; `scripts/i18n.py`
/// finds it.
pub const fn en(text: &str) -> &str {
    text
}

/// How texts get their catalog keys (`scripts/i18n.py` writes the file, the browser bundles read a
/// copy): the Latin letters that are written plain, and the explicit names of texts that read the
/// same once case and punctuation are gone (`Closed` and `closed`).
struct KeyTable {
    translit: HashMap<char, String>,
    texts: HashMap<String, HashMap<String, String>>,
}

#[expect(clippy::panic, reason = "the file is embedded and checked by tests")]
static KEY_TABLE: LazyLock<KeyTable> = LazyLock::new(|| {
    let v: Value = serde_json::from_str(include_str!("../../locales/keys.json")).unwrap_or_else(|e| panic!("keys.json: {e}"));
    let strings = |v: &Value| -> HashMap<String, String> {
        v.as_object()
            .map(|o| o.iter().filter_map(|(k, x)| x.as_str().map(|x| (k.clone(), x.to_owned()))).collect())
            .unwrap_or_default()
    };
    KeyTable {
        translit: strings(&v["translit"]).into_iter().filter_map(|(k, x)| k.chars().next().map(|c| (c, x))).collect(),
        texts: v["texts"]
            .as_object()
            .map(|o| o.iter().map(|(p, t)| (p.clone(), strings(t))).collect())
            .unwrap_or_default(),
    }
});

/// The longest readable name; a longer text is cut after a whole word.
const KEY_MAX: usize = 56;

/// A text's readable name: its words in lower case joined by underscores (`First like` is
/// `first_like`). A text with no Latin words at all is `text_` and its hash.
fn slug(text: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();
    for ch in text.chars() {
        let plain = KEY_TABLE.translit.get(&ch).cloned().unwrap_or_else(|| ch.to_string());
        for c in plain.chars() {
            if c.is_ascii_alphanumeric() {
                cur.push(c.to_ascii_lowercase());
            } else if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    let name = words.join("_");
    if name.is_empty() {
        return format!("text_{}", fnv8(text));
    }
    if name.len() > KEY_MAX {
        let cut = &name[..KEY_MAX];
        return cut.rfind('_').map_or(cut, |i| &cut[..i]).to_owned();
    }
    name
}

fn fnv8(text: &str) -> String {
    let mut h: u32 = 0x811c_9dc5;
    for b in text.as_bytes() {
        h ^= u32::from(*b);
        h = h.wrapping_mul(0x0100_0193);
    }
    format!("{h:08x}")
}

/// The catalog key of a text in `prefix` (`msg` for the server, `js`, `admin`).
pub fn text_key(prefix: &str, text: &str) -> String {
    let name = KEY_TABLE.texts.get(prefix).and_then(|t| t.get(text)).cloned().unwrap_or_else(|| slug(text));
    format!("{prefix}.{name}")
}

/// The catalog key of a message written in English in the code (`T::msg`): `msg.` and the words
/// of the text, so a key reads as what it says. `scripts/i18n.py` computes the same keys to keep
/// the catalogs in step with the code.
pub fn msg_key(english: &str) -> String {
    text_key("msg", english)
}

/// A number a counted message (`T::n`) can take: any integer, or a reference to one.
pub trait Count {
    fn count(&self) -> i64;
}

macro_rules! count_int {
    ($($t:ty),*) => {$(impl Count for $t {
        fn count(&self) -> i64 {
            i64::try_from(*self).unwrap_or(i64::MAX)
        }
    })*};
}
count_int!(i8, i16, i32, i64, u8, u16, u32, u64, usize, isize);

impl<C: Count + ?Sized> Count for &C {
    fn count(&self) -> i64 {
        (**self).count()
    }
}

/// A translator bound to one locale.
#[derive(Debug, Clone, Copy)]
pub struct T {
    pub locale: &'static str,
}

impl T {
    /// Text direction for the `dir` attribute. CSS uses logical properties where it can.
    pub fn dir(&self) -> &'static str {
        if matches!(self.locale.split('-').next(), Some("ar" | "he" | "fa" | "ur")) { "rtl" } else { "ltr" }
    }
}

impl Default for T {
    fn default() -> Self {
        T { locale: "en" }
    }
}

impl T {
    pub fn new(code: &str) -> T {
        T { locale: LOCALES.iter().find(|(c, _, _)| *c == code).map_or("en", |(c, _, _)| *c) }
    }

    /// Every message whose key starts with `prefix` (`js.`, `admin.`), in this locale, with
    /// English where it has none and the admin's overrides applied, compiled for the browser
    /// (see `compile`): the catalog a script needs.
    pub fn messages(&self, prefix: &str) -> BTreeMap<String, Value> {
        state().messages(self.locale, prefix)
    }

    fn has(&self, key: &str) -> bool {
        state().compiled(self.locale, &id_of(key)).is_some()
    }

    /// A message written in English in the code: its translation, or the English itself. Errors
    /// (`AppError` details) go through this on their way out, so they need nothing at the call site.
    pub fn msg(&self, english: &str) -> String {
        let key = msg_key(english);
        if self.has(&key) { self.t(&key) } else { english.to_owned() }
    }

    /// `msg` with `{name}` placeholders filled from `vars`.
    pub fn msgf(&self, english: &str, vars: &[(&str, &str)]) -> String {
        let key = msg_key(english);
        if self.has(&key) { self.f(&key, vars) } else { crate::mail::fill(english, vars) }
    }

    /// A message whose key ends with a value: `tj("follow.", "watch")` is `t("follow.watch")`.
    /// For templates, which cannot build a key with `format!`.
    pub fn tj(&self, prefix: &str, key: &str) -> String {
        self.t(&format!("{prefix}{key}"))
    }

    /// A message; each variable it uses stays as the literal `{name}`, for `mail::fill` later.
    pub fn t(&self, key: &str) -> String {
        self.f(key, &[])
    }

    /// A message with variables.
    pub fn f(&self, key: &str, vars: &[(&str, &str)]) -> String {
        state().render(self.locale, key, vars, None).unwrap_or_else(|| key.to_owned())
    }

    /// A counted message: one message under `key` that selects on `$n` by the locale's CLDR plural
    /// rules. Takes any integer or a reference to one, so templates can pass bound values directly.
    pub fn n(&self, key: &str, n: impl Count) -> String {
        state().render(self.locale, key, &[], Some(n.count())).unwrap_or_else(|| key.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(r: &[(&str, &str, &str)]) -> Vec<Row> {
        r.iter().map(|(a, b, c)| (a.to_string(), b.to_string(), c.to_string())).collect()
    }

    #[test]
    fn a_bad_override_is_explained_with_its_line() {
        let err = validate_override("{ $n ->\n    [one] a\n   *[other] { $n").unwrap_err();
        assert!(err.starts_with("line "), "{err}");
        assert!(err.starts_with("line 3:"), "{err}");
        assert!(validate_override("{ $n -> bad").is_err());
        assert!(validate_override("").is_err());
        assert!(validate_override("{ $n ->\n    [one] { $n } x\n   *[other] { $n } xs\n}").is_ok());
    }

    #[test]
    fn catalog_for_gives_the_locales_own_text() {
        let en = catalog();
        assert_eq!(catalog_for("en"), en);
        let pl = catalog_for("pl");
        assert_eq!(pl.len(), en.len());
        assert!(pl.iter().zip(&en).all(|(a, b)| a.0 == b.0));
        assert!(pl != en, "Polish differs from English");
        assert_eq!(catalog_for("xx"), en);
    }

    #[test]
    fn catalogs_load_without_junk() {
        for (code, _, src) in LOCALES {
            if let Err((_, errors)) = fluent_syntax::parser::parse(src) {
                panic!("locales/{code}.ftl has junk: {errors:?}");
            }
        }
        assert!(BASE.locales["en"].ast.messages.len() > 100);
    }

    #[test]
    fn every_message_compiles_for_the_browser() {
        for (code, loc) in &BASE.locales {
            for (id, c) in &loc.compiled {
                if let Err(e) = &c.parts {
                    panic!("{code}: {id}: {e}");
                }
            }
        }
    }

    #[test]
    fn translations_use_the_same_variables_as_english() {
        let names = |c: &Compiled| {
            let mut v: Vec<_> = c.vars.iter().map(|(n, _)| n.clone()).collect();
            v.sort();
            v
        };
        let mut bad = Vec::new();
        for (id, en) in &BASE.locales["en"].compiled {
            for code in ["pl", "ar"] {
                if let Some(other) = BASE.locales[code].compiled.get(id)
                    && names(en) != names(other)
                {
                    bad.push(format!("{code}: {} has {:?}, English has {:?}", key_of(id), names(other), names(en)));
                }
            }
        }
        bad.sort();
        assert!(bad.is_empty(), "variables differ from English:\n{}", bad.join("\n"));
    }

    fn selects<'a>(parts: &'a [Part], found: &mut Vec<&'a Vec<(String, Vec<Part>)>>) {
        for part in parts {
            if let Part::Sel { cases, .. } = part {
                found.push(cases);
                for (_, body) in cases {
                    selects(body, found);
                }
            }
        }
    }

    #[test]
    fn plural_variants_are_categories_of_the_locale() {
        let allowed: [(&str, &[&str]); 3] = [
            ("en", &["one", "other"]),
            ("pl", &["one", "few", "many", "other"]),
            ("ar", &["zero", "one", "two", "few", "many", "other"]),
        ];
        let all = ["zero", "one", "two", "few", "many", "other"];
        for (code, ok) in allowed {
            for (id, c) in &BASE.locales[code].compiled {
                let mut found = Vec::new();
                selects(c.parts.as_ref().unwrap(), &mut found);
                for cases in found {
                    if !cases.iter().any(|(k, _)| all.contains(&k.as_str())) {
                        continue;
                    }
                    for (k, _) in cases {
                        assert!(
                            ok.contains(&k.as_str()) || k.parse::<f64>().is_ok(),
                            "{code}: {} has variant [{k}]",
                            key_of(id)
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn arabic_is_right_to_left() {
        assert_eq!(T::new("ar").dir(), "rtl");
        assert_eq!(T::new("pl").dir(), "ltr");
    }

    #[test]
    fn message_keys_match_the_catalog_script() {
        // scripts/i18n.py computes the same FNV-1a keys; this one is in the catalogs.
        assert_eq!(msg_key("Not found"), "msg.not_found");
        // A key reads as what it says.
        assert_eq!(msg_key("First Like"), "msg.first_like");
        assert_eq!(msg_key("Hi {name},"), "msg.hi_name");
        assert_eq!(text_key("admin", "Save text"), "admin.save_text");
        // Accented Latin letters are written plain; a text with no Latin words keeps a hash.
        assert_eq!(msg_key("Członek-założyciel"), "msg.czlonek_zalozyciel");
        assert!(msg_key("مرحبا").starts_with("msg.text_"));
        // Two texts that differ only in case or punctuation have explicit, stable names.
        assert_eq!(msg_key("Closed"), "msg.closed");
        assert_eq!(msg_key("closed"), "msg.closed_2");
        // A long text is cut after a whole word.
        let long = msg_key("This is a very long sentence that goes on and on and on and on and on forever");
        assert!(long.len() <= "msg.".len() + KEY_MAX && !long.ends_with('_'), "{long}");
        let pl = T::new("pl");
        assert_eq!(T::new("en").msg("No such text in any catalog"), "No such text in any catalog");
        assert_ne!(pl.msg("Not found"), "Not found");
    }

    #[test]
    fn every_english_key_is_translated() {
        let en = &BASE.locales["en"].ast.messages;
        for code in ["pl", "ar"] {
            let other = &BASE.locales[code].ast.messages;
            let mut missing: Vec<_> = en.keys().filter(|k| !other.contains_key(*k)).map(|k| key_of(k)).collect();
            missing.sort_unstable();
            assert!(missing.is_empty(), "missing in {code}: {missing:?}");
        }
    }

    #[test]
    fn lookup_fallback_and_plurals() {
        let en = T::new("en");
        let pl = T::new("pl");
        let ar = T::new("ar");
        assert_eq!(en.t("nav.forums"), "Forums");
        assert_eq!(pl.t("nav.forums"), "Fora");
        assert_eq!(T::new("xx").t("nav.forums"), "Forums");
        assert_eq!(en.t("no.such.key"), "no.such.key");
        assert_eq!(en.n("count.topics", 1), "1 topic");
        assert_eq!(en.n("count.topics", 5), "5 topics");
        assert_eq!(en.n("count.topics", 1234), "1234 topics");
        assert_eq!(pl.n("count.topics", 1), "1 temat");
        assert_eq!(pl.n("count.topics", 3), "3 tematy");
        assert_eq!(pl.n("count.topics", 5), "5 tematów");
        assert_eq!(pl.n("count.topics", 12), "12 tematów");
        assert_eq!(pl.n("count.topics", 22), "22 tematy");
        assert_eq!(ar.n("count.topics", 2), "2 موضوعان");
    }

    #[test]
    fn t_leaves_placeholders_for_a_later_fill() {
        let en = T::new("en");
        let text = en.t("msg.hi_name_confirm_email_as_the_new_address_for_your");
        assert!(text.contains("Hi {name},") && text.contains("{email}") && text.contains("{link}"), "{text}");
        let filled = en.f("msg.hi_name_confirm_email_as_the_new_address_for_your", &[("name", "Ann"), ("email", "a@b.c")]);
        assert!(filled.contains("Hi Ann,") && filled.contains("a@b.c") && filled.contains("{link}"), "{filled}");
        assert_eq!(en.msgf("Not found {x}", &[("x", "1")]), "Not found 1");
    }

    #[test]
    fn compiled_json_for_plain_variable_and_plural_messages() {
        let m = T::new("en").messages("nav.");
        assert_eq!(m["nav.forums"], serde_json::json!("Forums"));
        let m = T::new("en").messages("msg.hi_name_confirm_email_as_the_new_address_for_your");
        assert_eq!(m["msg.hi_name_confirm_email_as_the_new_address_for_your"][0], serde_json::json!("Hi "));
        assert_eq!(m["msg.hi_name_confirm_email_as_the_new_address_for_your"][1], serde_json::json!({ "v": "name" }));
        let m = T::new("pl").messages("count.topics");
        let v = &m["count.topics"][0];
        assert_eq!(v["sel"], "n");
        assert_eq!(v["def"], "other");
        assert_eq!(v["cases"]["few"], serde_json::json!([{ "v": "n" }, " tematy"]));
    }

    #[test]
    fn an_override_replaces_a_whole_plural_message() {
        let source = "{ $n ->\n    [one] only { $n } thread\n   *[other] { $n } threads\n}";
        assert!(validate_override(source).is_ok());
        let b = build(&rows(&[
            ("en", "count.topics", source),
            ("pl", "count.topics", "{ $n } tem."),
            ("pl", "gone.key", "x"),
        ]))
        .unwrap();
        assert_eq!(b.render("en", "count.topics", &[], Some(1)).unwrap(), "only 1 thread");
        assert_eq!(b.render("en", "count.topics", &[], Some(3)).unwrap(), "3 threads");
        assert_eq!(b.render("pl", "count.topics", &[], Some(3)).unwrap(), "3 tem.");
        assert_eq!(b.render("ar", "count.topics", &[], Some(2)).unwrap(), "2 موضوعان");
        assert_eq!(b.messages("en", "count.topics")["count.topics"][0]["cases"]["one"][0], "only ");
        assert!(b.render("pl", "gone.key", &[], None).is_none());
        // An invalid override is skipped, not applied.
        let b = build(&rows(&[("en", "nav.forums", "{ BAD($x) }")])).unwrap();
        assert_eq!(b.render("en", "nav.forums", &[], None).unwrap(), "Forums");
        assert!(validate_override("{ BAD($x) }").is_err());
        assert!(validate_override("{ $n ->").is_err());
        assert!(validate_override("").is_err());
    }

    #[test]
    fn every_catalog_key_is_what_the_code_computes() {
        // The Polish catalog writes each message's English above it; the key under it must be the
        // one `text_key` gives that English (the script that wrote it and this code agree).
        let (_, _, pl) = LOCALES.iter().find(|(c, _, _)| *c == "pl").unwrap();
        let generated = pl.split_once("## Messages from the code").map_or("", |(_, rest)| rest);
        let mut english: Option<String> = None;
        let mut checked = 0;
        for line in generated.lines() {
            if let Some(text) = line.strip_prefix("# ") {
                english = Some(text.replace("\\n", "\n"));
                continue;
            }
            let id = line.split(" =").next().unwrap_or_default();
            let prefix = ["msg-", "js-", "admin-"].into_iter().find(|p| id.starts_with(p));
            if let (Some(text), Some(prefix)) = (english.take(), prefix) {
                let want = text_key(&prefix[..prefix.len() - 1], &text);
                assert_eq!(id.replacen('-', ".", 1), want, "for {text:?}");
                checked += 1;
            }
        }
        assert!(checked > 3000, "only {checked} checked");
    }

    #[test]
    fn a_database_text_can_be_translated_by_an_override() {
        // Braces, a quote and a backslash must stay text; a built-in message is not registered twice.
        let text = "Founding {Member} \"one\" \\ #1\nSecond line";
        set_site_texts([text.to_owned(), "First Like".to_owned(), "  ".to_owned()]);
        let key = msg_key(text);
        assert!(site_catalog().iter().any(|(k, t)| *k == key && t == text));
        assert!(!site_catalog().iter().any(|(_, t)| t == "First Like"), "built-in text stays in the catalog");
        let b = build_with(&rows(&[("pl", &key, "Członek-założyciel")]), &[text.to_owned()]).unwrap();
        assert_eq!(b.render("en", &key, &[], None).unwrap(), text);
        assert_eq!(b.render("pl", &key, &[], None).unwrap(), "Członek-założyciel");
        assert_eq!(b.render("ar", &key, &[], None).unwrap(), text, "no override: the English text");
    }

    #[test]
    fn references_are_inlined_and_recursion_is_an_error() {
        let r = fluent_syntax::parser::parse_runtime(
            "-brand = Forum\nhello = Hi { $who }, welcome to { -brand }. { other }\nother = Bye { NUMBER($n) }\na = { b }\nb = { a }\n".to_owned(),
        )
        .unwrap();
        let mut a = Ast::default();
        for e in r.body {
            match e {
                Entry::Message(m) => {
                    a.messages.insert(m.id.name, m.value.unwrap());
                }
                Entry::Term(t) => {
                    a.terms.insert(t.id.name, t.value);
                }
                _ => {}
            }
        }
        let chain = Chain { first: &a, second: None };
        let parts = compile::compile(&a.messages["hello"], chain).unwrap();
        assert_eq!(
            compile::to_json(&parts),
            serde_json::json!(["Hi ", { "v": "who" }, ", welcome to Forum. Bye ", { "v": "n", "num": true }])
        );
        assert!(compile::compile(&a.messages["a"], chain).unwrap_err().contains("refers to itself"));
    }

    #[test]
    fn catalog_sources_round_trip() {
        let en = &BASE.locales["en"].ast.messages;
        let all = catalog();
        assert_eq!(all.len(), en.len());
        for (key, source) in all {
            let parsed = parse_override(&source).unwrap_or_else(|e| panic!("{key}: {e}\n{source}"));
            assert_eq!(parsed, en[&id_of(&key)], "{key}\n{source}");
        }
    }

    #[test]
    fn accept_language() {
        assert_eq!(from_accept_language("pl-PL,pl;q=0.9,en;q=0.8"), Some("pl"));
        assert_eq!(from_accept_language("de-DE, en;q=0.5"), Some("en"));
        assert_eq!(from_accept_language("de, fr;q=0.8"), None);
        assert_eq!(from_accept_language("en;q=0.2, pl;q=0.9"), Some("pl"));
        // The site's own language: a language we have, else English (the setting says "browser").
        assert_eq!((site_language("pl"), site_language("browser"), site_language("")), ("pl", "en", "en"));
    }
}
