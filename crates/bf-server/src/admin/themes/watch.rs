//! Theme development: sends a theme folder to a site whenever a file changes.

use serde_json::{Value, json};
use std::time::Duration;

use super::{ThemeToml, default_kind};

/// Theme development: sends a theme folder to a site whenever a file changes, using an
/// admin API key. Open the site with this theme as preview, then reload to see each save.
// The `theme watch` subcommand reports to the terminal of the person running it.
#[allow(clippy::print_stdout)]
pub async fn watch(dir: &str, url: &str, key: &str) -> Result<(), String> {
    fn stamp(root: &std::path::Path, out: &mut Vec<(std::path::PathBuf, std::time::SystemTime)>) {
        for e in std::fs::read_dir(root).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stamp(&p, out);
            } else if let Ok(t) = e.metadata().and_then(|m| m.modified()) {
                out.push((p, t));
            }
        }
    }
    fn load(root: &std::path::Path) -> Result<Value, String> {
        let text = |p: &std::path::Path| std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()));
        let m: ThemeToml = toml::from_str(&text(&root.join("theme.toml"))?).map_err(|e| e.to_string())?;
        let tokens = if root.join("tokens.json").exists() {
            serde_json::from_str(&text(&root.join("tokens.json"))?).map_err(|e| e.to_string())?
        } else {
            json!({})
        };
        let mut css = String::new();
        for e in std::fs::read_dir(root.join("styles")).into_iter().flatten().flatten() {
            css.push_str(&text(&e.path())?);
        }
        let mut slots = serde_json::Map::new();
        for (folder, prefix) in [("slots", ""), ("templates", "partial:")] {
            for e in std::fs::read_dir(root.join(folder)).into_iter().flatten().flatten() {
                let name = e.path().file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                slots.insert(format!("{prefix}{name}"), Value::String(text(&e.path())?));
            }
        }
        Ok(
            json!({ "slug": m.id, "name": m.name, "kind": m.kind.unwrap_or_else(default_kind), "tokens": tokens, "css": css, "slots": slots }),
        )
    }
    let root = std::path::Path::new(dir);
    let client = reqwest::Client::new();
    let mut last = Vec::new();
    println!("Watching {dir}; press Ctrl+C to stop.");
    loop {
        let mut now = Vec::new();
        stamp(root, &mut now);
        now.sort();
        if now != last {
            last = now;
            let sent = match load(root) {
                Ok(body) => client
                    .post(format!("{}/api/v1/admin/themes", url.trim_end_matches('/')))
                    .bearer_auth(key)
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| e.to_string()),
                Err(e) => Err(e),
            };
            match sent {
                Ok(r) if r.status().is_success() => println!("Sent."),
                Ok(r) => println!("Refused: {}", r.text().await.unwrap_or_default()),
                Err(e) => println!("{e}"),
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
