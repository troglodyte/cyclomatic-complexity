//! Shared helpers for the integration tests.
//!
//! `env!("CARGO_BIN_EXE_…")` only covers binaries in the test's own package,
//! and the aliases live in separate packages, so `bin` builds them itself.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// The workspace root, where the goldens' relative paths resolve.
pub fn workspace_root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root exists")
    })
}

/// Where cargo puts `debug/<name>`: `CARGO_TARGET_DIR` if set, else `target/`.
fn target_dir() -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => workspace_root().join("target"),
    }
}

/// The debug binary `name` (`cmplx`, `jscx` or `rustcx`), building all three
/// on the first call.
pub fn bin(name: &str) -> PathBuf {
    static BUILT: OnceLock<()> = OnceLock::new();
    BUILT.get_or_init(|| {
        let status = Command::new(env!("CARGO"))
            .args(["build", "-p", "cmplx", "-p", "jscx", "-p", "rustcx"])
            .arg("--manifest-path")
            .arg(workspace_root().join("Cargo.toml"))
            .status()
            .expect("run cargo build");
        assert!(status.success(), "cargo build of the binaries failed");
    });
    target_dir().join("debug").join(name)
}

/// Rewrite the paths inside a golden's text to where the fixtures live now.
///
/// The goldens were captured with the fixtures under `legacy/`; the files are
/// byte-identical copies today under `crates/cmplx-{rust,js}/fixtures/`. The
/// golden files themselves stay untouched. Works on JSON, table rows and
/// headers alike, since it only swaps path prefixes.
pub fn remap_golden_paths(text: &str) -> String {
    text.replace("legacy/rustcx/fixtures/", "crates/cmplx-rust/fixtures/")
        .replace("legacy/jscx/", "crates/cmplx-js/fixtures/")
}

#[cfg(test)]
mod tests {
    use super::remap_golden_paths;

    #[test]
    fn remap_golden_paths_rewrites_json_and_table_prefixes() {
        let text = "{\"file\":\"legacy/rustcx/fixtures/a.rs\"}\nlegacy/jscx/test.ts:71  fn\nlegacy/jscx/test.js\n";
        assert_eq!(
            remap_golden_paths(text),
            "{\"file\":\"crates/cmplx-rust/fixtures/a.rs\"}\ncrates/cmplx-js/fixtures/test.ts:71  fn\ncrates/cmplx-js/fixtures/test.js\n"
        );
    }

    #[test]
    fn remap_golden_paths_leaves_other_paths_alone() {
        assert_eq!(
            remap_golden_paths("src/legacy.rs x.rs"),
            "src/legacy.rs x.rs"
        );
    }
}
