//! The core must not know about any concrete language crate or parser.

use std::fs;
use std::path::{Path, PathBuf};

const FORBIDDEN: &[&str] = &[
    "syn::",
    "syn =",
    "cmplx_js",
    "cmplx-js",
    "cmplx_rust",
    "cmplx-rust",
    "\"jscx\"",
    "\"rustcx\"",
];

/// Strings allowed only in a specific file (the `rustcx.toml` fallback).
const WHITELIST: &[(&str, &str)] = &[("config.rs", "\"rustcx\"")];

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read_dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

fn is_whitelisted(path: &Path, needle: &str) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    WHITELIST
        .iter()
        .any(|(file, allowed)| *file == name && *allowed == needle)
}

#[test]
fn core_sources_and_manifest_do_not_mention_language_crates() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = vec![root.join("Cargo.toml")];
    rust_sources(&root.join("src"), &mut files);

    let violations: Vec<String> = files
        .iter()
        .flat_map(|path| {
            let text = fs::read_to_string(path).expect("read file");
            FORBIDDEN
                .iter()
                .filter(|needle| text.contains(**needle) && !is_whitelisted(path, needle))
                .map(|needle| format!("{} contains {needle}", path.display()))
                .collect::<Vec<_>>()
        })
        .collect();

    assert!(violations.is_empty(), "{violations:#?}");
}
