//! The Claude Code complexity hook reads `--json` output through a jq filter
//! (`fixtures/complexity.jq`, copied from the skills repo's `complexity.sh`).
//! Feeding it the new binaries' output must give what the old output gave.

mod common;

use common::{bin, workspace_root};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

const NO_BEFORE: &str = r#"{"functions":[]}"#;

/// `jq -rs --argjson max 1 -f complexity.jq` over `[after, {"functions":[]}]`.
fn hook_findings(after_json: &str) -> String {
    let filter = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/complexity.jq");
    let mut jq = Command::new("jq")
        .args(["-rs", "--argjson", "max", "1", "-f"])
        .arg(filter)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|_| panic!("install jq to run hook_compat"));
    jq.stdin
        .take()
        .expect("piped stdin")
        .write_all(format!("{after_json}\n{NO_BEFORE}\n").as_bytes())
        .expect("write to jq");
    let out = jq.wait_with_output().expect("run jq");
    assert!(out.status.success(), "jq: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).expect("utf-8 jq output")
}

fn new_json(tool: &str, path: &str) -> String {
    let out = Command::new(bin(tool))
        .current_dir(workspace_root())
        .args(["--json", path])
        .output()
        .expect("run binary");
    String::from_utf8(out.stdout).expect("utf-8 output")
}

fn golden_json(tool: &str, case: &str) -> String {
    let path = workspace_root().join(format!("tests/golden/{tool}/{case}.stdout"));
    fs::read_to_string(path).expect("golden output")
}

fn assert_hook_sees_the_same(tool: &str, case: &str, path: &str) {
    let old = hook_findings(&golden_json(tool, case));
    let new = hook_findings(&new_json(tool, path));
    assert!(!old.is_empty(), "the filter should report something at max 1");
    assert_eq!(new, old);
}

#[test]
fn the_hook_filter_reads_jscx_json_as_before() {
    assert_hook_sees_the_same("jscx", "json-test.ts", "legacy/jscx/test.ts");
}

#[test]
fn the_hook_filter_reads_rustcx_json_as_before() {
    assert_hook_sees_the_same("rustcx", "json-patterns", "legacy/rustcx/fixtures/patterns.rs");
}
