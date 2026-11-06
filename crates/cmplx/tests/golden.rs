//! The new binaries against the output the pre-merge jscx and rustcx recorded
//! in `tests/golden/` (see `tools/capture-goldens.sh` for the exact commands).
//!
//! Compared by meaning, not bytes (spec, "Compatibility"): JSON must contain
//! every old field with the same value; tables are compared as a multiset of
//! rows; exit codes are exact. Accepted differences (rulings R12/R14): JSON key
//! order, the jscx banner and summary lines, and `--top`'s layout.

mod common;

use common::{bin, workspace_root};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

/// One parsed table row: cc, line, kind, name, indent (nesting columns).
type TableRow = (u32, u32, String, String, usize);

/// Why `new` is not a superset of `old`, or `Ok` when every old field is
/// present in `new` with an equal value (numbers within 1e-3, arrays
/// element by element and of equal length).
fn json_superset(old: &Value, new: &Value) -> Result<(), String> {
    fn walk(old: &Value, new: &Value, path: &str) -> Result<(), String> {
        match (old, new) {
            (Value::Object(o), Value::Object(n)) => o.iter().try_for_each(|(key, old_val)| {
                let at = format!("{path}.{key}");
                let new_val = n.get(key).ok_or_else(|| format!("{at}: missing in new"))?;
                walk(old_val, new_val, &at)
            }),
            (Value::Array(o), Value::Array(n)) => {
                if o.len() != n.len() {
                    return Err(format!("{path}: length {} != {}", o.len(), n.len()));
                }
                o.iter()
                    .zip(n)
                    .enumerate()
                    .try_for_each(|(i, (a, b))| walk(a, b, &format!("{path}[{i}]")))
            }
            (Value::Number(o), Value::Number(n)) => {
                let (o, n) = (o.as_f64().unwrap_or(f64::NAN), n.as_f64().unwrap_or(f64::NAN));
                if (o - n).abs() <= 1e-3 {
                    Ok(())
                } else {
                    Err(format!("{path}: {o} != {n}"))
                }
            }
            _ if old == new => Ok(()),
            _ => Err(format!("{path}: {old} != {new}")),
        }
    }
    walk(old, new, "$")
}

/// The function rows of a table, ignoring headers, banners, summaries and
/// `--explain` point lines. Handles both layouts: grouped
/// (`cc line  kind(13 wide)indent+name`) and flat (`cc  path:line  kind name`).
fn table_rows(stdout: &str) -> Vec<TableRow> {
    stdout.lines().filter_map(parse_row).collect()
}

fn parse_row(line: &str) -> Option<TableRow> {
    let rest = line.trim_start();
    let (cc, rest) = rest.split_once(char::is_whitespace)?;
    let cc: u32 = cc.parse().ok()?;
    let rest = rest.trim_start();
    let (second, rest) = rest.split_once(char::is_whitespace)?;
    if let Ok(line_no) = second.parse::<u32>() {
        // Grouped: two spaces (one already consumed), then the kind padded to
        // 13, then indent and name.
        let rest = rest.strip_prefix(' ')?;
        let kind_field: String = rest.chars().take(13).collect();
        let tail: String = rest.chars().skip(13).collect();
        let indent = tail.len() - tail.trim_start().len();
        return Some((cc, line_no, kind_field.trim().to_string(), tail.trim().to_string(), indent));
    }
    // Flat: the Location column is `path:line`; a flat row never shows depth.
    let (_, line_no) = second.rsplit_once(':')?;
    let line_no: u32 = line_no.parse().ok()?;
    let rest = rest.trim_start();
    let (kind, name) = match rest.strip_prefix("trait fn ") {
        Some(name) => ("trait fn", name),
        None => rest.split_once(char::is_whitespace)?,
    };
    Some((cc, line_no, kind.to_string(), name.trim().to_string(), 0))
}

fn multiset(rows: Vec<TableRow>) -> BTreeMap<TableRow, usize> {
    rows.into_iter().fold(BTreeMap::new(), |mut acc, row| {
        *acc.entry(row).or_insert(0) += 1;
        acc
    })
}

struct Run {
    stdout: String,
    stderr: String,
    code: i32,
}

/// Run `tool` from the workspace root, as the capture script did.
fn run(tool: &str, args: &[&str], stdin_file: Option<&str>) -> Run {
    let mut command = Command::new(bin(tool));
    command
        .current_dir(workspace_root())
        .env("NO_COLOR", "1")
        .env("CLICOLOR", "0")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn binary");
    let input = stdin_file.map(|p| fs::read(workspace_root().join(p)).expect("stdin file"));
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(input.as_deref().unwrap_or_default())
        .expect("write stdin");
    let out = child.wait_with_output().expect("run binary");
    Run {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        code: out.status.code().unwrap_or(-1),
    }
}

struct Golden {
    stdout: String,
    exit: i32,
}

fn golden(tool: &str, case: &str) -> Golden {
    let base = workspace_root().join("tests/golden").join(tool).join(case);
    let read = |ext: &str| {
        fs::read_to_string(format!("{}.{ext}", base.display()))
            .unwrap_or_else(|e| panic!("golden {tool}/{case}.{ext}: {e}"))
    };
    Golden {
        stdout: read("stdout"),
        exit: read("exit").trim().parse().expect("golden exit code"),
    }
}

fn assert_json_matches(tool: &str, case: &str, args: &[&str], stdin_file: Option<&str>) {
    let old = golden(tool, case);
    let new = run(tool, args, stdin_file);
    assert_eq!(new.code, old.exit, "exit code; stderr:\n{}", new.stderr);
    let old_json: Value = serde_json::from_str(&old.stdout).expect("golden is JSON");
    let new_json: Value = serde_json::from_str(&new.stdout).expect("new output is JSON");
    if let Err(why) = json_superset(&old_json, &new_json) {
        panic!("{tool}/{case}: {why}\nnew:\n{}", new.stdout);
    }
}

fn assert_rows_match(tool: &str, case: &str, args: &[&str]) {
    let old = golden(tool, case);
    let new = run(tool, args, None);
    assert_eq!(new.code, old.exit, "exit code; stderr:\n{}", new.stderr);
    let old_rows = multiset(table_rows(&old.stdout));
    assert!(!old_rows.is_empty(), "the golden has rows to compare");
    assert_eq!(
        multiset(table_rows(&new.stdout)),
        old_rows,
        "{tool}/{case} rows differ; new output:\n{}",
        new.stdout
    );
}

fn assert_missing(tool: &str, path: &str) {
    let old = golden(tool, "missing");
    let new = run(tool, &[path], None);
    assert_eq!(new.code, old.exit);
    assert!(new.stderr.contains(path), "stderr names the path:\n{}", new.stderr);
}

const RUST_FIXTURES: [&str; 4] = ["nesting", "patterns", "simple", "syntax"];

#[test]
fn rustcx_json_matches_the_goldens() {
    for name in RUST_FIXTURES {
        let path = format!("legacy/rustcx/fixtures/{name}.rs");
        assert_json_matches("rustcx", &format!("json-{name}"), &["--json", &path], None);
    }
}

#[test]
fn rustcx_explain_rows_match_the_goldens() {
    for name in RUST_FIXTURES {
        let path = format!("legacy/rustcx/fixtures/{name}.rs");
        assert_rows_match("rustcx", &format!("explain-{name}"), &["--explain", &path]);
    }
}

#[test]
fn rustcx_top_rows_match_the_golden() {
    assert_rows_match("rustcx", "top", &["--top", "5", "legacy/rustcx/fixtures"]);
}

#[test]
fn rustcx_max_rows_and_exit_match_the_golden() {
    assert_rows_match("rustcx", "max", &["--max", "3", "legacy/rustcx/fixtures"]);
}

#[test]
fn rustcx_missing_path_exits_two_naming_it() {
    assert_missing("rustcx", "does-not-exist.rs");
}

#[test]
fn jscx_json_matches_the_goldens() {
    for name in ["test.js", "test.ts"] {
        let path = format!("legacy/jscx/{name}");
        assert_json_matches("jscx", &format!("json-{name}"), &["--json", &path], None);
    }
}

#[test]
fn jscx_explain_rows_match_the_goldens() {
    for name in ["test.js", "test.ts"] {
        let path = format!("legacy/jscx/{name}");
        assert_rows_match("jscx", &format!("explain-{name}"), &["--explain", &path]);
    }
}

#[test]
fn jscx_top_rows_match_the_golden() {
    assert_rows_match("jscx", "top", &["--top", "5", "legacy/jscx"]);
}

#[test]
fn jscx_max_rows_and_exit_match_the_golden() {
    assert_rows_match("jscx", "max", &["--max", "3", "legacy/jscx"]);
}

#[test]
fn jscx_stdin_json_matches_the_golden() {
    assert_json_matches(
        "jscx",
        "stdin-ts",
        &["--json", "--ext", "ts", "-"],
        Some("legacy/jscx/test.ts"),
    );
}

#[test]
fn jscx_missing_path_exits_two_naming_it() {
    assert_missing("jscx", "does-not-exist.js");
}

#[test]
fn json_superset_accepts_added_fields_and_close_numbers_only() {
    let old: Value = serde_json::json!({"a": 1.0, "b": [{"c": "x"}]});
    let added: Value = serde_json::json!({"a": 1.0004, "b": [{"c": "x", "d": 1}], "e": 2});
    assert_eq!(json_superset(&old, &added), Ok(()));
    let drifted: Value = serde_json::json!({"a": 1.1, "b": [{"c": "x"}]});
    assert!(json_superset(&old, &drifted).is_err());
    let dropped: Value = serde_json::json!({"a": 1.0});
    assert!(json_superset(&old, &dropped).is_err());
    let longer: Value = serde_json::json!({"a": 1.0, "b": [{"c": "x"}, {"c": "y"}]});
    assert!(json_superset(&old, &longer).is_err());
}

#[test]
fn table_rows_reads_grouped_and_flat_layouts_and_skips_the_rest() {
    let grouped = "legacy/x.rs\n    CC   Line  Kind         Name\n     3      8  method       Engine::allows\n     2     21  closure        filter\n           10  if\n\n2 functions in 1 file\n";
    assert_eq!(
        table_rows(grouped),
        vec![
            (3, 8, "method".to_string(), "Engine::allows".to_string(), 0),
            (2, 21, "closure".to_string(), "filter".to_string(), 2),
        ]
    );
    let flat = "==> Running jscx <==\n   CC  Location  Kind Name\n   25  legacy/jscx/test.ts:71  function applyPricingRules\n    2  a.rs:3  trait fn      Policy::check\n";
    assert_eq!(
        table_rows(flat),
        vec![
            (25, 71, "function".to_string(), "applyPricingRules".to_string(), 0),
            (2, 3, "trait fn".to_string(), "Policy::check".to_string(), 0),
        ]
    );
}
