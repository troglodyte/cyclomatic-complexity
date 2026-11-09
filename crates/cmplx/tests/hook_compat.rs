//! The Claude Code complexity hook reads `--json` output through a jq filter
//! (`fixtures/complexity.jq`, copied from the skills repo's `complexity.sh`).
//! Feeding it the new binaries' output must give what the old output gave.

mod common;

use common::{bin, remap_golden_paths, workspace_root};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

const NO_BEFORE: &str = r#"{"functions":[]}"#;

/// `jq -rs --argjson max 1 -f complexity.jq` over `[after, {"functions":[]}]`.
fn hook_findings(after_json: &str) -> String {
    hook_findings_against(after_json, NO_BEFORE, 1)
}

/// `jq -rs --argjson max <max> -f complexity.jq` over `[after, before]`.
fn hook_findings_against(after_json: &str, before_json: &str, max: u32) -> String {
    let filter = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/complexity.jq");
    let mut jq = Command::new("jq")
        .args(["-rs", "--argjson", "max", &max.to_string(), "-f"])
        .arg(filter)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|_| panic!("install jq to run hook_compat"));
    jq.stdin
        .take()
        .expect("piped stdin")
        .write_all(format!("{after_json}\n{before_json}\n").as_bytes())
        .expect("write to jq");
    let out = jq.wait_with_output().expect("run jq");
    assert!(
        out.status.success(),
        "jq: {}",
        String::from_utf8_lossy(&out.stderr)
    );
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
    remap_golden_paths(&fs::read_to_string(path).expect("golden output"))
}

fn assert_hook_sees_the_same(tool: &str, case: &str, path: &str) {
    let old = hook_findings(&golden_json(tool, case));
    let new = hook_findings(&new_json(tool, path));
    assert!(
        !old.is_empty(),
        "the filter should report something at max 1"
    );
    assert_eq!(new, old);
}

#[test]
fn the_hook_filter_reads_jscx_json_as_before() {
    assert_hook_sees_the_same("jscx", "json-test.ts", "crates/cmplx-js/fixtures/test.ts");
}

#[test]
fn the_hook_filter_reads_rustcx_json_as_before() {
    assert_hook_sees_the_same(
        "rustcx",
        "json-patterns",
        "crates/cmplx-rust/fixtures/patterns.rs",
    );
}

/// `jscx --json` for a file holding one function `f` scoring `1 + ifs`.
fn jscx_json_for_f(ifs: usize) -> String {
    let dir = std::env::temp_dir().join(format!("cmplx-hook-{}-{ifs}", std::process::id()));
    fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("a.js");
    let body = "if (a) {}\n".repeat(ifs);
    fs::write(&path, format!("function f(a) {{\n{body}}}\n")).expect("write source");
    let out = Command::new(bin("jscx"))
        .args(["--json"])
        .arg(&path)
        .output()
        .expect("run jscx");
    String::from_utf8(out.stdout).expect("utf-8 output")
}

#[test]
fn the_hook_filter_reports_an_existing_function_that_got_worse() {
    let before = jscx_json_for_f(4);
    let after = jscx_json_for_f(11);
    assert_eq!(hook_findings_against(&after, &before, 10), "1\tf\t12\t5\n");
    assert_eq!(
        hook_findings_against(&after, &after, 10),
        "",
        "a function no worse than before is not reported"
    );
}
