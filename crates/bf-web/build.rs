//! Copies the built island bundles into OUT_DIR so they are embedded in the binary.
//! A missing bundle (no `pnpm build` yet) becomes a stub, so `cargo build`
//! never depends on Node; the release pipeline builds the front end first.

use std::path::Path;

type BuildResult = Result<(), Box<dyn std::error::Error>>;

fn main() -> BuildResult {
    let out_dir = std::env::var("OUT_DIR")?;
    // Two stylesheets, each an entry file that imports modules into cascade layers: the first is
    // inlined (and allowed by hash in the CSP), the second is linked. The imports are resolved
    // here, so the browser gets one file each and no extra requests.
    let css_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../themes/default/css");
    for name in ["critical.css", "app.css"] {
        let css = bundle_css(&css_dir, name);
        // The sources are formatted for reading; what is served is minified. The result has no
        // line breaks, so the hash in the CSP does not depend on how the files were checked out.
        std::fs::write(Path::new(&out_dir).join(name), minify_css(&css))?;
    }
    // Country flags for the session list: looked up by code, embedded in the binary.
    let flags = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../themes/default/flags");
    println!("cargo:rerun-if-changed={}", flags.display());
    let mut arms = String::from("pub fn flag(code: &str) -> Option<&'static str> {\n    match code {\n");
    let mut names: Vec<_> = std::fs::read_dir(&flags)?.map(|e| e.map(|e| e.path())).collect::<Result<_, _>>()?;
    names.sort();
    for path in names.iter().filter(|p| p.extension().is_some_and(|e| e == "svg")) {
        let code = path.file_stem().and_then(|s| s.to_str()).ok_or("flag file names must be UTF-8")?;
        let file = path.to_str().ok_or("flag paths must be UTF-8")?;
        arms.push_str(&format!("        {code:?} => Some(include_str!({file:?})),\n"));
    }
    arms.push_str("        _ => None,\n    }\n}\n");
    std::fs::write(Path::new(&out_dir).join("flags.rs"), arms)?;
    // Chunks the entries load on demand (`g-*`, `m-*`). Their names carry a content hash, so they
    // are served under their own names, which is how the entries import them.
    let dist = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/islands/dist");
    println!("cargo:rerun-if-changed={}", dist.display());
    let mut chunks = String::from("pub static CHUNKS: &[(&str, &[u8])] = &[\n");
    let mut files: Vec<_> = std::fs::read_dir(&dist)
        .map(|d| d.filter_map(std::result::Result::ok).map(|e| e.path()).collect())
        .unwrap_or_default();
    files.sort();
    for path in files {
        let name = path.file_name().and_then(|s| s.to_str()).ok_or("island chunk names must be UTF-8")?.to_owned();
        let chunk = (name.starts_with("g-") || name.starts_with("m-")) && name.ends_with(".js");
        if chunk && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')) {
            println!("cargo:rerun-if-changed={}", path.display());
            let file = path.to_str().ok_or("island chunk paths must be UTF-8")?;
            chunks.push_str(&format!("    ({name:?}, include_bytes!({file:?})),\n"));
        }
    }
    chunks.push_str("];\n");
    std::fs::write(Path::new(&out_dir).join("chunks.rs"), chunks)?;
    for (name, stub) in [
        ("islands.js", "export function mount(){}\n"),
        ("editor.js", "export function mount(t){return{focus(){t.focus()}}}\n"),
        ("members.js", "export function mount(){return false}\n"),
    ] {
        let out = Path::new(&out_dir).join(name);
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/islands/dist").join(name);
        println!("cargo:rerun-if-changed={}", src.display());
        if let Ok(bytes) = std::fs::read(&src) {
            std::fs::write(&out, bytes)?;
        } else {
            println!("cargo:warning=web/islands/dist/{name} not found; embedding a stub (run `pnpm build` in web/)");
            std::fs::write(&out, stub)?;
        }
    }
    Ok(())
}

/// Conservative CSS minifier: drops comments, collapses runs of whitespace to one space, and
/// removes the space around `{`, `}` and `;`, where it never matters. It does not touch spaces
/// elsewhere, because a space can be a selector combinator (`.a :hover`) or separate values.
/// The theme sources have no strings or `url()`s that contain comment markers or runs of spaces.
fn minify_css(src: &str) -> String {
    let mut no_comments = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(start) = rest.find("/*") {
        no_comments.push_str(&rest[..start]);
        rest = rest[start + 2..].find("*/").map_or("", |end| &rest[start + 2 + end + 2..]);
        no_comments.push(' ');
    }
    no_comments.push_str(rest);
    let collapsed = no_comments.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = String::with_capacity(collapsed.len());
    for c in collapsed.chars() {
        if matches!(c, '{' | '}' | ';') && out.ends_with(' ') {
            out.pop();
        }
        if c == ' ' && out.ends_with(['{', '}', ';']) {
            continue;
        }
        out.push(c);
    }
    out.replace(";}", "}")
}

/// Inlines an entry stylesheet's `@import url("path") layer(name);` lines, each module wrapped in
/// `@layer name { … }`, which is what the browser would do with the import. Only that form is
/// accepted, so a typo fails the build instead of shipping an import the server cannot answer.
#[expect(clippy::panic, reason = "build script: a bad stylesheet must fail the build")]
fn bundle_css(dir: &Path, entry: &str) -> String {
    let path = dir.join(entry);
    println!("cargo:rerun-if-changed={}", path.display());
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut out = String::new();
    for line in src.lines() {
        let Some(rest) = line.trim().strip_prefix("@import ") else {
            out.push_str(line);
            out.push('\n');
            continue;
        };
        let parsed = rest
            .strip_prefix("url(\"")
            .and_then(|r| r.split_once("\")"))
            .and_then(|(file, tail)| Some((file, tail.trim().strip_prefix("layer(")?.strip_suffix(");")?)));
        let Some((file, layer)) = parsed else {
            panic!("{entry}: write imports as @import url(\"path\") layer(name);  got: {line}");
        };
        let module = dir.join(file);
        println!("cargo:rerun-if-changed={}", module.display());
        let body = std::fs::read_to_string(&module).unwrap_or_else(|e| panic!("{}: {e}", module.display()));
        assert!(!body.contains("@import"), "{file}: modules may not import further files");
        out.push_str(&format!("@layer {layer} {{\n{body}\n}}\n"));
    }
    out
}
