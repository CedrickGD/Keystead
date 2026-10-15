use std::path::{Path, PathBuf};

fn main() {
    embed_browser_extension();
    tauri_build::build();
}

/// Embeds the browser extension (`extension/chrome`) into the binary: writes
/// `$OUT_DIR/extension_files.rs`, a slice of `(relative path, bytes)` pairs
/// (`include_bytes!`, so file edits are tracked by rustc as well). The app
/// writes it to `<data_dir>/browser-extension` (see `src/extension.rs`).
fn embed_browser_extension() {
    let manifest_dir =
        PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let source = manifest_dir
        .join("..")
        .join("..")
        .join("..")
        .join("extension")
        .join("chrome");
    // A directory: cargo re-runs this script when anything below it changes
    // (also added or removed files).
    println!("cargo:rerun-if-changed={}", source.display());

    let mut files = Vec::new();
    collect(&source, Path::new(""), &mut files);
    files.sort();
    assert!(
        files.iter().any(|(rel, _)| rel == "manifest.json"),
        "extension/chrome/manifest.json missing ({})",
        source.display()
    );

    let mut code = String::from("&[\n");
    for (rel, abs) in &files {
        code.push_str(&format!(
            "    ({rel:?}, include_bytes!({:?}) as &[u8]),\n",
            abs.display().to_string()
        ));
    }
    code.push_str("]\n");
    let out =
        PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("extension_files.rs");
    std::fs::write(&out, code).expect("write extension_files.rs");
}

/// Regular files below `dir` as (`a/b.js` with forward slashes, absolute
/// path); hidden entries (`.…`) are skipped.
fn collect(root: &Path, rel: &Path, out: &mut Vec<(String, PathBuf)>) {
    let dir = root.join(rel);
    let entries = std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.expect("directory entry");
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let rel_path = rel.join(&name);
        let file_type = entry.file_type().expect("file type");
        if file_type.is_dir() {
            collect(root, &rel_path, out);
        } else if file_type.is_file() {
            let rel_str = rel_path
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            out.push((rel_str, entry.path()));
        }
    }
}
