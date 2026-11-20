//! Plugin development commands (`boring-forum plugin pack | sign | watch`): build a folder into
//! the zip the forum installs, sign it, and reinstall it on a dev server every time a file changes.

use serde_json::Value;
use std::io::{Read, Write};
use std::path::Path;

use super::package::read_package;

type Files = Vec<(String, Vec<u8>)>;

/// Folders that never go into a package.
const SKIP: [&str; 5] = ["target", "dist", ".git", "node_modules", ".cargo"];

fn collect(root: &Path, dir: &Path, out: &mut Files) -> Result<(), String> {
    for e in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten() {
        let path = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if !SKIP.contains(&name.as_str()) {
                collect(root, &path, out)?;
            }
        } else {
            let rel = path.strip_prefix(root).map_err(|e| e.to_string())?.to_string_lossy().replace('\\', "/");
            out.push((rel, std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?));
        }
    }
    Ok(())
}

/// Builds a Rust plugin (a folder with a `Cargo.toml`) for WebAssembly; returns the module's bytes.
fn build_wasm(dir: &Path) -> Result<Vec<u8>, String> {
    let cargo: toml::Value =
        toml::from_str(&std::fs::read_to_string(dir.join("Cargo.toml")).map_err(|e| e.to_string())?)
            .map_err(|e| format!("Cargo.toml: {e}"))?;
    let name = cargo
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(toml::Value::as_str)
        .ok_or("Cargo.toml has no package name.")?
        .replace('-', "_");
    let status = std::process::Command::new("cargo")
        .args(["build", "--release", "--target", "wasm32-unknown-unknown", "--manifest-path"])
        .arg(dir.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(dir.join("target"))
        .status()
        .map_err(|e| format!("cargo: {e}"))?;
    if !status.success() {
        return Err("The build failed (is `rustup target add wasm32-unknown-unknown` done?).".into());
    }
    let wasm = dir.join(format!("target/wasm32-unknown-unknown/release/{name}.wasm"));
    std::fs::read(&wasm).map_err(|e| format!("{}: {e}", wasm.display()))
}

fn zip_files(files: &Files) -> Result<Vec<u8>, String> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in files {
        zip.start_file(name.as_str(), options).map_err(|e| e.to_string())?;
        zip.write_all(bytes).map_err(|e| e.to_string())?;
    }
    Ok(zip.finish().map_err(|e| e.to_string())?.into_inner())
}

/// The package for a plugin folder, checked by the same reader the forum installs with. A folder
/// with a `Cargo.toml` is built first, and the module is stored under the name `plugin.toml`'s
/// `[server] module` gives it.
pub fn pack(dir: &str) -> Result<Vec<u8>, String> {
    let root = Path::new(dir);
    let mut files = Files::new();
    collect(root, root, &mut files)?;
    files.retain(|(n, _)| !matches!(n.as_str(), "Cargo.toml" | "Cargo.lock" | "build.sh" | ".gitignore") && !n.starts_with("src/"));
    if root.join("Cargo.toml").exists() {
        let manifest = files.iter().find(|(n, _)| n == "plugin.toml").ok_or("plugin.toml is missing.")?;
        let parsed: toml::Value = toml::from_str(&String::from_utf8_lossy(&manifest.1)).map_err(|e| e.to_string())?;
        let module = parsed
            .get("server")
            .and_then(|s| s.get("module"))
            .and_then(toml::Value::as_str)
            .ok_or("plugin.toml has no [server] module.")?
            .to_owned();
        files.retain(|(n, _)| *n != module);
        files.push((module, build_wasm(root)?));
    }
    let bytes = zip_files(&files)?;
    read_package(&bytes)?; // what the forum would refuse is refused here, with its message
    Ok(bytes)
}

fn unzip(bytes: &[u8]) -> Result<Files, String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let mut out = Files::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| e.to_string())?;
        if f.is_dir() {
            continue;
        }
        let name = f.name().map(std::borrow::Cow::into_owned).map_err(|e| e.to_string())?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).map_err(|e| e.to_string())?;
        out.push((name, buf));
    }
    Ok(out)
}

/// Adds `signature.txt`: an Ed25519 signature, by the 32-byte seed in hex, over what the forum
/// verifies (see `package::read_package`). The matching public key goes in `BF_PLUGIN_PUBLIC_KEYS`.
pub fn sign(zip_bytes: &[u8], seed_hex: &str) -> Result<Vec<u8>, String> {
    use ed25519_dalek::{Signer, SigningKey};
    let seed = super::package::from_hex(seed_hex.trim())
        .and_then(|b| <[u8; 32]>::try_from(b).ok())
        .ok_or("The key must be 64 hex characters (a 32-byte seed).")?;
    let mut files = unzip(zip_bytes)?;
    files.retain(|(n, _)| !n.ends_with("signature.txt"));
    let digest = read_package(&zip_files(&files)?)?.digest;
    let key = SigningKey::from_bytes(&seed);
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    files.push(("signature.txt".into(), hex(&key.sign(&digest).to_bytes()).into_bytes()));
    println!("public key: {}", hex(key.verifying_key().as_bytes()));
    zip_files(&files)
}

fn stamp(root: &Path, out: &mut Vec<(std::path::PathBuf, std::time::SystemTime)>) {
    for e in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            if !SKIP.contains(&e.file_name().to_string_lossy().as_ref()) {
                stamp(&p, out);
            }
        } else if let Ok(t) = e.metadata().and_then(|m| m.modified()) {
            out.push((p, t));
        }
    }
}

/// Rebuilds and reinstalls on every change, with an administrator's API key (as `theme watch`).
/// The first install of a plugin leaves it disabled for approval; later ones keep what was approved.
pub async fn watch(dir: &str, url: &str, key: &str) -> Result<(), String> {
    let client = reqwest::Client::new();
    let mut last = Vec::new();
    println!("Watching {dir}; press Ctrl+C to stop.");
    loop {
        let mut now = Vec::new();
        stamp(Path::new(dir), &mut now);
        now.sort();
        if now != last {
            last = now;
            let dir = dir.to_owned();
            let built = tokio::task::spawn_blocking(move || pack(&dir)).await.map_err(|e| e.to_string())?;
            match built {
                Ok(bytes) => {
                    let part = reqwest::multipart::Part::bytes(bytes).file_name("plugin.zip");
                    let sent = client
                        .post(format!("{}/api/v1/admin/plugins/install", url.trim_end_matches('/')))
                        .bearer_auth(key)
                        .multipart(reqwest::multipart::Form::new().part("file", part))
                        .send()
                        .await;
                    match sent {
                        Ok(r) if r.status().is_success() => println!("Installed."),
                        Ok(r) => {
                            let status = r.status();
                            let body: Value = r.json().await.unwrap_or(Value::Null);
                            println!("The forum said {status}: {}", body.get("detail").unwrap_or(&body));
                        }
                        Err(e) => println!("Could not reach the forum: {e}"),
                    }
                }
                Err(e) => println!("{e}"),
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_packs_and_its_signature_verifies() {
        let dir = std::env::temp_dir().join(format!("bf-plugin-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("templates")).unwrap();
        std::fs::create_dir_all(dir.join("target")).unwrap();
        std::fs::write(dir.join("plugin.toml"), "id = \"t\"\nname = \"T\"\nversion = \"1\"\napi = \"^1.0\"\n").unwrap();
        std::fs::write(dir.join("templates/footer.html"), "<p>hi</p>").unwrap();
        std::fs::write(dir.join("target/ignored.txt"), "x").unwrap();
        let bytes = pack(dir.to_str().unwrap()).unwrap();
        let names: Vec<String> = unzip(&bytes).unwrap().into_iter().map(|(n, _)| n).collect();
        assert!(names.contains(&"plugin.toml".to_owned()) && !names.iter().any(|n| n.starts_with("target/")));

        let seed = "07".repeat(32);
        let signed = sign(&bytes, &seed).unwrap();
        let files = unzip(&signed).unwrap();
        let sig = String::from_utf8(files.iter().find(|(n, _)| n == "signature.txt").unwrap().1.clone()).unwrap();
        let key = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
        let public: String = key.verifying_key().as_bytes().iter().map(|b| format!("{b:02x}")).collect();
        let digest = read_package(&bytes).unwrap().digest;
        assert!(super::super::package::verify_signature_with(&public, &digest, &sig));
        // Signing again replaces the signature rather than stacking a second one.
        let again = unzip(&sign(&signed, &seed).unwrap()).unwrap();
        assert_eq!(again.iter().filter(|(n, _)| n == "signature.txt").count(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }
}
