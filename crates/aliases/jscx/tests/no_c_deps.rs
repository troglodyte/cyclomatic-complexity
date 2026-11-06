//! `cargo install --path crates/aliases/jscx` must compile no C and no `syn`:
//! the JS alias's normal dependency tree may not reach a parser crate.

use std::path::Path;
use std::process::Command;

const FORBIDDEN: &[&str] = &["syn", "proc-macro2", "cc", "tree-sitter"];

/// The crate name a `cargo tree --prefix none` line starts with.
fn crate_name(line: &str) -> &str {
    line.split_whitespace().next().unwrap_or("")
}

/// Whole-token match, so `cc` does not catch `ccache` and `syn` not `synstructure`;
/// `tree-sitter` also covers its grammar crates (`tree-sitter-python`, …).
fn is_forbidden(name: &str) -> bool {
    FORBIDDEN.contains(&name) || name.starts_with("tree-sitter-")
}

#[test]
fn the_js_alias_has_no_parser_or_c_dependencies() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../Cargo.toml");
    let out = Command::new(env!("CARGO"))
        .args([
            "tree",
            "-p",
            "jscx",
            "-e",
            "normal",
            "--prefix",
            "none",
            "--manifest-path",
        ])
        .arg(&workspace)
        .output()
        .expect("run cargo tree");
    assert!(
        out.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let tree = String::from_utf8(out.stdout).expect("utf-8 cargo tree output");
    assert!(
        tree.lines().any(|line| crate_name(line) == "cmplx-js"),
        "the tree should include cmplx-js:\n{tree}"
    );
    let offenders: Vec<&str> = tree
        .lines()
        .filter(|line| is_forbidden(crate_name(line)))
        .collect();
    assert!(
        offenders.is_empty(),
        "forbidden dependencies: {offenders:#?}"
    );
}

#[test]
fn the_name_match_is_by_whole_token() {
    assert!(is_forbidden(crate_name("syn v3.0.0")));
    assert!(is_forbidden(crate_name("tree-sitter-python v0.23.0")));
    assert!(!is_forbidden(crate_name("synstructure v0.13.1")));
    assert!(!is_forbidden(crate_name("ccache v1.0.0")));
}
