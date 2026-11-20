//! Installing a theme from a zip or an archive URL.

use crate::admin::api::{Admin, AdminWrite};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Multipart, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use bf_core::AppError;
use bf_core::i18n::en;
use serde::Deserialize;
use serde_json::{Value, json};
use std::io::Read;

use super::{MAX_UNPACKED, MAX_ZIP, NewTheme, default_kind, save_new};

#[derive(Deserialize)]
pub(super) struct ThemeToml {
    pub(super) id: String,
    pub(super) name: String,
    #[serde(default)]
    pub(super) version: Option<String>,
    #[serde(default, rename = "type")]
    pub(super) kind: Option<String>,
}

/// What a theme package holds, read purely in memory: nothing is written to disk, so paths in
/// the archive cannot escape anywhere. Sizes are checked as they are read (zip bombs).
pub(super) fn read_package(bytes: &[u8]) -> Result<(NewTheme, String), String> {
    let mut zip =
        zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|_| en("That is not a zip file.").to_owned())?;
    if zip.len() > 200 {
        return Err(en("Too many files in the package.").into());
    }
    let (mut manifest, mut tokens, mut css, mut slots) = (None, Value::Null, String::new(), serde_json::Map::new());
    let mut names: Vec<String> = (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok().and_then(|f| f.name().ok().map(std::borrow::Cow::into_owned)))
        .collect();
    names.sort();
    // Archives from git hosts wrap everything in one top-level folder.
    let prefix = crate::platform::plugins::archive_prefix(&names, "theme.toml");
    let mut budget = MAX_UNPACKED;
    for full in names {
        let Some(name) = full.strip_prefix(prefix.as_str()).map(str::to_owned) else { continue };
        let wanted = name == "theme.toml"
            || name == "tokens.json"
            || (name.starts_with("styles/") && name.ends_with(".css"))
            || (name.starts_with("slots/") && name.ends_with(".html"))
            || (name.starts_with("templates/") && name.ends_with(".html"));
        if !wanted {
            continue;
        }
        let file = zip.by_name(&full).map_err(|e| e.to_string())?;
        let mut text = String::new();
        let read = file.take(budget + 1).read_to_string(&mut text).map_err(|_| format!("{name} is not text."))?;
        if read as u64 > budget {
            return Err(en("The package is too large when unpacked.").into());
        }
        budget -= read as u64;
        match name.as_str() {
            "theme.toml" => {
                manifest = Some(toml::from_str::<ThemeToml>(&text).map_err(|e| format!("theme.toml: {e}"))?);
            }
            "tokens.json" => tokens = serde_json::from_str(&text).map_err(|e| format!("tokens.json: {e}"))?,
            n if n.starts_with("styles/") => {
                css.push_str(&text);
                css.push('\n');
            }
            n if n.starts_with("templates/") => {
                let partial = n.trim_start_matches("templates/").trim_end_matches(".html");
                slots.insert(format!("partial:{partial}"), Value::String(text));
            }
            n => {
                let slot = n.trim_start_matches("slots/").trim_end_matches(".html").to_owned();
                slots.insert(slot, Value::String(text));
            }
        }
    }
    let m = manifest.ok_or(en("The package has no theme.toml."))?;
    let t = NewTheme {
        slug: m.id,
        name: m.name,
        kind: m.kind.unwrap_or_else(default_kind),
        tokens,
        css,
        slots: Value::Object(slots),
    };
    Ok((t, m.version.unwrap_or_else(|| "1.0.0".into())))
}

pub(super) async fn install_zip(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    mut m: Multipart,
) -> Result<Response, AppError> {
    let mut bytes = None;
    while let Some(f) = m.next_field().await.map_err(|e| AppError::BadRequest(e.to_string()))? {
        if f.name() == Some("file") {
            bytes = Some(f.bytes().await.map_err(|e| AppError::BadRequest(e.to_string()))?);
        }
    }
    let bytes = bytes.ok_or_else(|| AppError::Validation("No file was sent.".into()))?;
    let (t, version) = read_package(&bytes).map_err(AppError::Validation)?;
    let id = save_new(&state, t, &version, who).await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))).into_response())
}

#[derive(Deserialize)]
pub(super) struct InstallUrl {
    pub(super) url: String,
}

/// From an archive URL, which is what git hosts offer for any repository and tag.
pub(super) async fn install_url(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Json(b): Json<InstallUrl>,
) -> Result<Response, AppError> {
    let bytes = crate::platform::outbound::fetch_public(&b.url, MAX_ZIP).await.map_err(AppError::Validation)?;
    let (t, version) = read_package(&bytes).map_err(AppError::Validation)?;
    let id = save_new(&state, t, &version, who).await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn package(files: &[(&str, &str)]) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        let mut z = zip::ZipWriter::new(&mut out);
        for (name, body) in files {
            z.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(body.as_bytes()).unwrap();
        }
        z.finish().unwrap();
        out.into_inner()
    }

    #[test]
    fn packages_are_read_in_memory() {
        let bytes = package(&[
            ("theme.toml", "id = \"sunny\"\nname = \"Sunny\"\nversion = \"2.0.0\"\ntype = \"theme\""),
            ("tokens.json", "{\"light\":{\"accent\":\"#f80\"}}"),
            ("styles/a.css", ".a{}"),
            ("slots/footer.html", "<b>hi</b>"),
            ("../../evil.css", ".evil{}"),
        ]);
        let (t, version) = read_package(&bytes).unwrap();
        assert_eq!((t.slug.as_str(), version.as_str()), ("sunny", "2.0.0"));
        assert!(t.css.contains(".a{}") && !t.css.contains("evil"));
        assert_eq!(t.slots["footer"], "<b>hi</b>");
    }

    #[test]
    fn bad_packages_are_refused() {
        assert!(read_package(b"not a zip").is_err());
        assert!(read_package(&package(&[("styles/a.css", ".a{}")])).is_err());
    }
}
