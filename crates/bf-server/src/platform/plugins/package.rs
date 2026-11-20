//! The plugin zip: manifest, signature check and validation.

use bf_core::i18n::en;
use bf_db::platform::plugins;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::Read;

use super::extras::{self, ActionDecl, AdminPageDecl, CookRuleDecl, EmojiDecl, WidgetDecl};
use super::version::{satisfies, split_requirement};
use super::{HOOKS, HOST_API, MAX_PACKAGE, PANEL_AREAS, Panel, SCHEDULED, known_capability};

/// The folder wrapping an archive's files, if the marker file is not at the top: "" or "name/".
pub(crate) fn archive_prefix(names: &[String], marker: &str) -> String {
    if names.iter().any(|n| n == marker) {
        return String::new();
    }
    let nested = names.iter().find(|n| n.ends_with(&format!("/{marker}")) && n.matches('/').count() == 1);
    nested.map_or_else(String::new, |n| n[..n.len() - marker.len()].to_owned())
}

#[derive(Deserialize)]
pub(super) struct Manifest {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) version: String,
    pub(super) api: String,
    #[serde(default = "wasm_kind")]
    pub(super) kind: String,
    #[serde(default)]
    pub(super) capabilities: Vec<String>,
    #[serde(default)]
    pub(super) hooks: Vec<String>,
    pub(super) server: Option<ServerPart>,
    pub(super) client: Option<ClientPart>,
    /// Islands the client code serves: it loads only on pages that have one. Empty: every page.
    #[serde(default)]
    pub(super) islands: Vec<String>,
    #[serde(default)]
    pub(super) nav: Vec<Value>,
    #[serde(default)]
    pub(super) tables: Vec<plugins::TableSpec>,
    #[serde(default)]
    pub(super) panels: Vec<Panel>,
    /// Declarative registries (`[[profile_fields]]`, ...): rows seeded into the core's lists.
    #[serde(flatten)]
    pub(super) registries: super::registry::Sections,
    /// Other plugins this one needs, as `"id"` or `"id ^1.0"`; checked when it is enabled.
    #[serde(default)]
    pub(super) requires: Vec<String>,
    /// Declarative cooker syntax: a marker pair around text becomes an HTML template.
    #[serde(default)]
    pub(super) cook_rules: Vec<CookRuleDecl>,
    #[serde(default)]
    pub(super) post_actions: Vec<ActionDecl>,
    #[serde(default)]
    pub(super) composer_buttons: Vec<ActionDecl>,
    /// Slots the plugin fills per viewer with `slot.render` (names from `theme::SLOTS`).
    #[serde(default)]
    pub(super) dynamic_slots: Vec<String>,
    #[serde(default)]
    pub(super) emoji: Vec<EmojiDecl>,
    /// Blocks the page builder offers, with a form for their settings.
    #[serde(default)]
    pub(super) widgets: Vec<WidgetDecl>,
    /// Form pages in the admin panel.
    #[serde(default)]
    pub(super) admin_pages: Vec<AdminPageDecl>,
    /// Pages under `/app/<id>/`: paths with parameters, GET and POST, feeds (needs `pages`).
    #[serde(default)]
    pub(super) routes: Vec<super::apps::RouteDecl>,
    /// Kinds of entry that take part in search and the review queue (needs `content_types`).
    #[serde(default)]
    pub(super) content_types: Vec<super::content_types::ContentTypeDecl>,
}

pub(super) fn wasm_kind() -> String {
    "wasm".into()
}

#[derive(Deserialize)]
pub(super) struct ServerPart {
    pub(super) module: String,
}

#[derive(Deserialize)]
pub(super) struct ClientPart {
    /// The module; empty for a plugin that ships only style sheets.
    #[serde(default)]
    pub(super) entry: String,
    /// Style sheets, served from the plugin's own folder under /plugins/.
    #[serde(default)]
    pub(super) css: Vec<String>,
}

pub(super) struct Package {
    pub(super) manifest: Manifest,
    pub(super) module: Option<Vec<u8>>,
    pub(super) slots: Value,
    pub(super) client: Option<String>,
    pub(super) schema: Option<Value>,
    pub(super) checksum: String,
    pub(super) signed: bool,
    /// What a signature covers, for `plugin sign`.
    pub(super) digest: [u8; 32],
}

/// What a signature covers: the manifest and the code, not the zip around them.
pub(super) fn content_digest(manifest: &[u8], module: Option<&[u8]>, client: Option<&str>) -> [u8; 32] {
    let mut h = Sha256::new();
    for part in [manifest, module.unwrap_or_default(), client.unwrap_or_default().as_bytes()] {
        h.update((part.len() as u64).to_be_bytes()); // lengths, so bytes cannot move between parts
        h.update(part);
    }
    h.finalize().into()
}

pub(super) fn from_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// Ed25519 against the keys in `BF_PLUGIN_PUBLIC_KEYS` (hex, separated by commas).
pub(super) fn verify_signature(digest: &[u8; 32], signature_hex: &str) -> bool {
    let trusted_keys = std::env::var("BF_PLUGIN_PUBLIC_KEYS").unwrap_or_default();
    verify_signature_with(&trusted_keys, digest, signature_hex)
}

/// `verify_signature` against an explicit list of hex public keys separated by commas.
pub(super) fn verify_signature_with(trusted_keys: &str, digest: &[u8; 32], signature_hex: &str) -> bool {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let Some(sig) = from_hex(signature_hex).and_then(|b| Signature::from_slice(&b).ok()) else { return false };
    trusted_keys.split(',').any(|k| {
        let key = from_hex(k.trim())
            .and_then(|b| <[u8; 32]>::try_from(b).ok())
            .and_then(|b| VerifyingKey::from_bytes(&b).ok());
        key.is_some_and(|key| key.verify(digest, &sig).is_ok())
    })
}

/// Reads a plugin zip in memory: nothing is unpacked to disk, and sizes are capped as they are read.
pub(super) fn read_package(bytes: &[u8]) -> Result<Package, String> {
    let mut zip =
        zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|_| en("That is not a zip file.").to_owned())?;
    if zip.len() > 500 {
        return Err(en("Too many files in the package.").into());
    }
    let names: Vec<String> = (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok().and_then(|f| f.name().ok().map(std::borrow::Cow::into_owned)))
        .collect();
    let prefix = archive_prefix(&names, "plugin.toml");
    let mut read = |name: &str| -> Result<Option<Vec<u8>>, String> {
        let Ok(file) = zip.by_name(&format!("{prefix}{name}")) else { return Ok(None) };
        let mut out = Vec::new();
        file.take(MAX_PACKAGE as u64 + 1).read_to_end(&mut out).map_err(|e| format!("{name}: {e}"))?;
        if out.len() > MAX_PACKAGE {
            return Err(format!("{name} is too large."));
        }
        Ok(Some(out))
    };
    let text = read("plugin.toml")?.ok_or(en("The package has no plugin.toml."))?;
    let mut manifest: Manifest =
        toml::from_str(&String::from_utf8_lossy(&text)).map_err(|e| format!("plugin.toml: {e}"))?;
    validate_manifest(&manifest)?;
    let module = match &manifest.server {
        Some(s) => Some(read(&s.module)?.ok_or_else(|| format!("{} is missing.", s.module))?),
        None => None,
    };
    let client = match &manifest.client {
        Some(c) if !c.entry.is_empty() => {
            let js = read(&c.entry)?.ok_or_else(|| format!("{} is missing.", c.entry))?;
            Some(String::from_utf8(js).map_err(|_| en("The client code is not text.").to_owned())?)
        }
        _ => None,
    };
    let mut extras = extras::read(&manifest, &names, &prefix, &mut read)?;
    extras.dynamic.clone_from(&manifest.dynamic_slots);
    // A panel or link may carry its text as `label_key`, translated from `locales/`.
    for panel in &mut manifest.panels {
        panel.label = extras::english(&panel.label, panel.label_key.as_deref(), &extras.locales);
        if panel.label.is_empty() {
            return Err(format!("Panel {} has no text: add it to locales/en.json.", panel.key));
        }
    }
    for t in &mut manifest.content_types {
        t.label = extras::english(&t.label, t.label_key.as_deref(), &extras.locales);
    }
    for link in &mut manifest.nav {
        let label = link.get("label").and_then(Value::as_str).unwrap_or_default().to_owned();
        let key = link.get("label_key").and_then(Value::as_str).map(str::to_owned);
        link["label"] = Value::String(extras::english(&label, key.as_deref(), &extras.locales));
        // A menu the link is grouped under, and the icon beside it (see `Activation::add`).
        let menu = link.get("menu").and_then(Value::as_str).unwrap_or_default().to_owned();
        let menu_key = link.get("menu_key").and_then(Value::as_str).map(str::to_owned);
        if !menu.is_empty() || menu_key.is_some() {
            link["menu"] = Value::String(extras::english(&menu, menu_key.as_deref(), &extras.locales));
        }
        if !link.get("icon").and_then(Value::as_str).is_none_or(|i| i.is_empty() || extras::plain(i, 40, &[])) {
            return Err(en("A link's icon is a name like \"star\".").into());
        }
    }
    let mut schema: Option<Value> = read("settings.json")?
        .map(|b| serde_json::from_slice(&b).map_err(|e| format!("settings.json: {e}")))
        .transpose()?;
    let schema_file = schema.clone();
    if let Some(doc) = &mut schema {
        let map = doc.as_object_mut().ok_or("settings.json must be a JSON object.")?;
        // These extras are only ever taken from the validated manifest.
        map.retain(|k, _| !k.starts_with("x-"));
    }
    // Extras from the manifest travel with the schema, which is stored as JSON anyway.
    let plain_extras = !manifest.islands.is_empty()
        || !manifest.nav.is_empty()
        || !manifest.tables.is_empty()
        || !manifest.panels.is_empty()
        || !manifest.requires.is_empty();
    let routed = !manifest.routes.is_empty() || !manifest.content_types.is_empty();
    if plain_extras || routed || !manifest.registries.is_empty() || !extras.is_empty() {
        let doc = schema.get_or_insert_with(|| json!({}));
        if plain_extras {
            doc["x-islands"] = json!(manifest.islands);
            doc["x-nav"] = json!(manifest.nav);
            doc["x-tables"] = json!(manifest.tables);
            doc["x-panels"] = json!(manifest.panels);
            doc["x-requires"] = json!(manifest.requires);
        }
        if routed {
            doc["x-routes"] = json!(manifest.routes);
            doc["x-content-types"] = json!(manifest.content_types);
        }
        if !manifest.registries.is_empty() {
            manifest.registries.store(doc);
        }
        extras.store(doc);
    }
    let signature = read("signature.txt")?.map(|b| String::from_utf8_lossy(&b).trim().to_owned());
    let mut slots = serde_json::Map::new();
    for slot in bf_web::theme::SLOTS {
        if let Some(b) = read(&format!("templates/{slot}.html"))? {
            let src = String::from_utf8(b).map_err(|_| format!("Slot {slot} is not text."))?;
            bf_web::theme::render_slot(&src, &json!({ "site_name": "x" })).map_err(|e| format!("Slot {slot}: {e}"))?;
            slots.insert(slot.to_owned(), Value::String(src));
        }
    }
    let checksum = Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
    // The settings file and slot templates are installed too, so they are signed as well.
    let mut covered = content_digest(&text, module.as_deref(), client.as_deref()).to_vec();
    covered.extend_from_slice(&serde_json::to_vec(&schema_file).map_err(|e| e.to_string())?);
    covered.extend_from_slice(&serde_json::to_vec(&slots).map_err(|e| e.to_string())?);
    covered.extend_from_slice(&extras.signed_bytes());
    let digest: [u8; 32] = Sha256::digest(&covered).into();
    let signed = signature.as_deref().is_some_and(|sig| verify_signature(&digest, sig));
    if signature.is_some() && !signed {
        return Err(en("The package's signature does not verify against any trusted key.").into());
    }
    Ok(Package { manifest, module, slots: Value::Object(slots), client, schema, checksum, signed, digest })
}

pub(super) fn validate_manifest(m: &Manifest) -> Result<(), String> {
    let valid_id = !m.id.is_empty()
        && m.id.len() <= 60
        && !m.id.contains("--") // keeps table prefixes of two plugins from overlapping
        && !m.id.starts_with('-')
        && !m.id.ends_with('-')
        && m.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid_id || m.name.trim().is_empty() {
        return Err(en("Use a lowercase id with dashes, and a name.").into());
    }
    if m.kind != "wasm" {
        return Err(en("Only wasm plugins can be installed; native ones are compiled in.").into());
    }
    if !satisfies(HOST_API, &m.api) {
        return Err(format!("This plugin needs host API {}; this forum has {HOST_API}.", m.api));
    }
    for r in &m.requires {
        let (id, req) = split_requirement(r);
        let bad_req = req != "*" && super::version::parse(req.trim_start_matches(['^', '~', '=', '>'])).is_none();
        if id.is_empty() || id == m.id || bad_req {
            return Err(format!("Bad requirement {r}."));
        }
    }
    if m.client.as_ref().is_some_and(|c| !c.entry.is_empty()) && !m.capabilities.iter().any(|c| c == "client") {
        return Err(en("A plugin that ships browser code must ask for the client capability.").into());
    }
    if let Some(c) = m.capabilities.iter().find(|c| !known_capability(c)) {
        return Err(format!("Unknown capability {c}."));
    }
    m.registries.validate(&m.capabilities)?;
    let own_permissions: Vec<String> = m.registries.permissions.iter().map(|p| p.key.clone()).collect();
    super::apps::validate_routes(&m.routes, &m.capabilities, &own_permissions)?;
    super::content_types::validate(&m.content_types, &m.capabilities, &own_permissions)?;
    if !m.routes.is_empty() && !m.hooks.iter().any(|h| h == "page.request") {
        return Err(en("Routes need the page.request hook.").into());
    }
    if !m.tables.is_empty() && !m.capabilities.iter().any(|c| c == "tables") {
        return Err(en("A plugin that declares tables must ask for the tables capability.").into());
    }
    plugins::validate_tables(&m.id, &m.tables)?;
    if let Some(x) = m
        .panels
        .iter()
        .find(|x| !PANEL_AREAS.contains(&x.area.as_str()) || !extras::plain(&x.key, 40, &[]) || (x.label.trim().is_empty() && x.label_key.is_none()))
    {
        return Err(format!("Bad panel {}.", x.key));
    }
    if let Some(s) = m.dynamic_slots.iter().find(|s| !bf_web::theme::SLOTS.contains(&s.as_str()) && s.as_str() != bf_web::theme::CARD_SLOT) {
        return Err(format!("Unknown slot {s}."));
    }
    if !m.dynamic_slots.is_empty() && !m.hooks.iter().any(|h| h == "slot.render") {
        return Err(en("dynamic_slots needs the slot.render hook.").into());
    }
    if let Some(h) = m.hooks.iter().find(|h| !HOOKS.contains(&h.as_str()) && h.as_str() != SCHEDULED) {
        return Err(format!("Unknown hook {h}."));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::plugins::http_allowed;

    fn manifest(extra: &str) -> Result<(), String> {
        let m: Manifest = toml::from_str(&format!("id = \"a\"\nname = \"A\"\nversion = \"1\"\n{extra}")).unwrap();
        validate_manifest(&m)
    }

    #[test]
    fn signatures_cover_the_code() {
        use ed25519_dalek::{Signer, SigningKey};
        let key = SigningKey::from_bytes(&[7; 32]);
        let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        let trusted = hex(key.verifying_key().as_bytes());
        let verify = |digest: &[u8; 32], sig: &str| verify_signature_with(&trusted, digest, sig);
        let digest = content_digest(b"m", Some(b"code"), None);
        let sig = hex(&key.sign(&digest).to_bytes());
        assert!(verify(&digest, &sig));
        assert!(!verify(&content_digest(b"m", Some(b"evil"), None), &sig), "changed code");
        assert!(!verify(&digest, "zz"));
    }

    #[test]
    fn archives_from_git_hosts_are_unwrapped() {
        let n = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(archive_prefix(&n(&["plugin.toml"]), "plugin.toml"), "");
        assert_eq!(archive_prefix(&n(&["repo-main/plugin.toml", "repo-main/x"]), "plugin.toml"), "repo-main/");
        assert_eq!(archive_prefix(&n(&["a/b/plugin.toml"]), "plugin.toml"), "");
    }

    #[test]
    fn http_is_limited_to_approved_hosts() {
        let hosts = vec!["api.example.com".to_owned()];
        assert!(http_allowed(&hosts, "https://api.example.com/x"));
        assert!(!http_allowed(&hosts, "https://api.example.com.evil.test/"));
        assert!(!http_allowed(&hosts, "https://api.example.com@evil.test/"));
        assert!(!http_allowed(&hosts, "https://user@api.example.com/"));
        assert!(!http_allowed(&hosts, "https://other.example.com/"));
        assert!(!http_allowed(&[], "https://api.example.com/"));
        assert!(
            manifest(
                "api = \"^1.0\"
capabilities = [\"http:api.example.com\"]"
            )
            .is_ok()
        );
        assert!(
            manifest(
                "api = \"^1.0\"
capabilities = [\"http:*\"]"
            )
            .is_err()
        );
        assert!(
            manifest(
                "api = \"^1.0\"
capabilities = [\"http:\"]"
            )
            .is_err()
        );
    }

    #[test]
    fn manifests_are_checked() {
        assert!(
            manifest("api = \"^1.0\"\n[[tables]]\nname = \"t\"\ncolumns = { a = \"text\" }").is_err(),
            "tables need approval"
        );
        assert!(manifest("api = \"^1.0\"\n[[panels]]\narea = \"root\"\nkey = \"k\"\nlabel = \"L\"").is_err());
        assert!(manifest("api = \"^1.0\"").is_ok());
        assert!(manifest("api = \"^2.0\"").is_err());
        assert!(manifest("api = \"^1.0\"\nkind = \"native\"").is_err());
        assert!(manifest("api = \"^1.0\"\ncapabilities = [\"root\"]").is_err());
        assert!(manifest("api = \"^1.0\"\nhooks = [\"nope\"]").is_err());
        let caps = |c: &str| manifest(&format!("api = \"^1.0\"\ncapabilities = [\"{c}\"]"));
        assert!(caps("connect:https://api.example.com").is_ok() && caps("connect:wss://live.example.com:8443").is_ok());
        for bad in ["connect:http://api.example.com", "connect:https://api.example.com/x", "connect:*", "connect:https://*.example.com"]
        {
            assert!(caps(bad).is_err(), "{bad}");
        }
        assert!(
            manifest("api = \"^1.0\"\n[client]\nentry = \"c.js\"").is_err(),
            "browser code needs the client capability"
        );
        assert!(manifest("api = \"^1.0\"\ncapabilities = [\"client\"]\n[client]\nentry = \"c.js\"").is_ok());
        assert!(manifest("api = \"^1.1\"").is_ok() && manifest("api = \"^1.9\"").is_err(), "newer than the host");
        assert!(manifest("api = \"^1.0\"\nrequires = [\"other ^1.0\"]").is_ok());
        assert!(manifest("api = \"^1.0\"\nrequires = [\"a\"]").is_err(), "needs itself");
    }
}
