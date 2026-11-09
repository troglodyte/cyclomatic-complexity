mod common;

use common::{bin, workspace_root};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Where the rustcx fixtures live; the relative `fixtures/...` paths below
/// resolve from here, as they did when this suite sat beside the tool.
fn rustcx_home() -> PathBuf {
    workspace_root().join("crates/cmplx-rust")
}

fn rustcx(args: &[&str]) -> (String, String, i32) {
    let out = Command::new(bin("rustcx"))
        .current_dir(rustcx_home())
        .args(args)
        .output()
        .expect("binary should run");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// A unique empty directory under the system temp dir, so tests that write a
/// `rustcx.toml` cannot see each other's.
fn temp_dir(tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("rustcx-{}-{}-{}", tag, std::process::id(), n));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Run the binary with `dir` as the working directory, which is where config
/// discovery starts.
fn run_in(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(bin("rustcx"))
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run rustcx")
}

/// The row for `name`, or panic. Used to assert on the score column rather
/// than searching the whole output for a number that could match elsewhere.
fn row_for<'a>(stdout: &'a str, name: &str) -> &'a str {
    stdout
        .lines()
        .find(|l| l.trim_end().ends_with(name))
        .unwrap_or_else(|| panic!("no row for {name} in:\n{stdout}"))
}

#[test]
fn scores_a_file_and_exits_zero() {
    let (stdout, _, code) = rustcx(&["fixtures/simple.rs"]);
    assert_eq!(code, 0);
    assert!(row_for(&stdout, "branchy").starts_with("     5"),
            "branchy should score 5, got:\n{stdout}");
    assert!(row_for(&stdout, "plain").starts_with("     1"));
    assert!(row_for(&stdout, "map").starts_with("     1"),
            "the closure should get its own row");
}

#[test]
fn walks_a_directory() {
    let (stdout, _, code) = rustcx(&["fixtures"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("fixtures/simple.rs"));
}

#[test]
fn max_breach_exits_one() {
    let (_, _, code) = rustcx(&["--max", "4", "fixtures/simple.rs"]);
    assert_eq!(code, 1, "branchy scores 5, which is above 4");
}

#[test]
fn max_not_breached_exits_zero() {
    let (_, _, code) = rustcx(&["--max", "5", "fixtures/simple.rs"]);
    assert_eq!(code, 0, "--max is exclusive: 5 is not above 5");
}

#[test]
fn a_missing_path_exits_two() {
    let (_, stderr, code) = rustcx(&["does/not/exist.rs"]);
    assert_eq!(code, 2);
    assert!(!stderr.is_empty());
}

#[test]
fn an_unparseable_file_warns_and_exits_two_without_stopping() {
    let dir = std::env::temp_dir().join("rustcx-parse-test");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("broken.rs"), "fn f( {").unwrap();
    std::fs::write(dir.join("ok.rs"), "fn g(a: bool) { if a { } }").unwrap();

    let (stdout, stderr, code) = rustcx(&[dir.to_str().unwrap()]);
    assert_eq!(code, 2);
    assert!(stderr.contains("parse error"), "stderr:\n{stderr}");
    assert!(stdout.contains("g"), "the good file must still be scored:\n{stdout}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_too_deeply_nested_file_is_reported_as_not_scored() {
    let dir = temp_dir("too-deep");
    let deep = dir.join("deep.rs");
    let depth = cmplx_rust::MAX_NESTING + 1;
    std::fs::write(&deep, format!("fn f() {{\n{}{};\n}}\n", "(".repeat(depth), ")".repeat(depth))).unwrap();
    std::fs::write(dir.join("ok.rs"), "fn g(a: bool) { if a { } }").unwrap();

    let (stdout, stderr, code) = rustcx(&[dir.to_str().unwrap()]);
    assert_eq!(code, 2);
    assert_eq!(
        stderr,
        format!(
            "rustcx: {}:2: not scored: too deeply nested to score (> {} levels)\n",
            deep.display(),
            cmplx_rust::MAX_NESTING
        )
    );
    assert!(stdout.contains("g"), "the good file must still be scored:\n{stdout}");
}

#[test]
fn stdin_is_read_with_a_dash() {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = Command::new(bin("rustcx"))
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.as_mut().unwrap()
        .write_all(b"fn f(a: bool) { if a { } }").unwrap();
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0));
    assert!(stdout.contains("f"), "got:\n{stdout}");
    assert!(stdout.contains("<stdin>"), "got:\n{stdout}");
}

#[test]
fn line_numbers_stay_correct_across_many_files() {
    // Guards the span-invalidation invariant: proc-macro2's span table is
    // cleared between files, and a mistake there shows up as wrong lines in
    // every file after the first.
    let dir = std::env::temp_dir().join("rustcx-span-test");
    std::fs::create_dir_all(&dir).unwrap();
    for i in 0..5 {
        std::fs::write(
            dir.join(format!("f{i}.rs")),
            "\n\n\nfn f(a: bool) { if a { } }\n",
        )
        .unwrap();
    }
    let (stdout, _, code) = rustcx(&[dir.to_str().unwrap()]);
    assert_eq!(code, 0);
    let on_line_four = stdout.matches("      4  fn").count();
    assert_eq!(on_line_four, 5, "every file's fn is on line 4:\n{stdout}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn min_hides_trivial_functions() {
    let (stdout, _, code) = rustcx(&["--min", "2", "fixtures/simple.rs"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("branchy"));
    assert!(!stdout.contains("plain"), "plain scores 1 and should be hidden:\n{stdout}");
}

#[test]
fn json_output_parses() {
    let (stdout, _, code) = rustcx(&["--json", "fixtures/simple.rs"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert!(v["functions"].as_array().unwrap().len() >= 3);
}

#[test]
fn explain_lists_branches_that_sum_to_the_score() {
    let (stdout, _, code) = rustcx(&["--explain", "--min", "2", "fixtures/patterns.rs"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("match arm"), "got:\n{stdout}");
    assert!(stdout.contains("guard"), "got:\n{stdout}");
    assert!(stdout.contains("pattern alternative"), "got:\n{stdout}");

    // describe scores 8, so it lists 7 decision points — the score is always
    // 1 + the number of explain lines.
    let after = stdout.split("describe").nth(1).unwrap();
    let listed = after
        .lines()
        .skip(1)
        .take_while(|l| l.starts_with("      "))
        .count();
    assert_eq!(listed, 7, "explain lines under describe:\n{after}");
}

#[test]
fn question_flag_reaches_the_cli() {
    let (plain, _, _) = rustcx(&["--json", "fixtures/patterns.rs"]);
    let (with, _, _) = rustcx(&["--json", "--question", "fixtures/patterns.rs"]);
    let p: serde_json::Value = serde_json::from_str(&plain).unwrap();
    let q: serde_json::Value = serde_json::from_str(&with).unwrap();
    let find = |v: &serde_json::Value| -> u64 {
        v["functions"].as_array().unwrap().iter()
            .find(|f| f["name"] == "parse").unwrap()["complexity"].as_u64().unwrap()
    };
    assert_eq!(find(&p), 1);
    assert_eq!(find(&q), 3);
}

#[test]
fn no_tests_flag_reaches_the_cli() {
    let dir = std::env::temp_dir().join("rustcx-notests");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("m.rs"),
        "pub fn real() {}\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {}\n}\n",
    )
    .unwrap();

    let (with, _, _) = rustcx(&[dir.to_str().unwrap()]);
    assert!(with.contains("2 functions"), "got:\n{with}");
    let (without, _, _) = rustcx(&["--no-tests", dir.to_str().unwrap()]);
    assert!(without.contains("1 function in"), "got:\n{without}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_ci_gate_reads_naturally() {
    // The documented CI invocation: clean under a generous threshold, failing
    // under a strict one, with the same output either way.
    let (_, _, clean) = rustcx(&["--max", "10", "fixtures"]);
    assert_eq!(clean, 0);
    let (_, _, breach) = rustcx(&["--max", "2", "fixtures"]);
    assert_eq!(breach, 1);
}

#[test]
fn sort_prints_one_flat_table_across_files() {
    // Scores interleave the two fixture files, which used to reprint a file
    // header every time that file's rows resurfaced.
    let (stdout, _, code) = rustcx(&["--sort", "--min", "4", "fixtures/"]);
    assert_eq!(code, 0);
    assert_eq!(stdout.matches("Location").count(), 1, "got:\n{stdout}");
    // A grouped header is a line that is nothing but the filename. The summary
    // names the file too, so count bare header lines rather than mentions.
    assert!(
        !stdout.lines().any(|l| l == "fixtures/patterns.rs"),
        "no file should get a section header, got:\n{stdout}"
    );
    assert!(row_for(&stdout, "describe").starts_with("     8  fixtures/patterns.rs:16"),
            "got:\n{stdout}");
}

#[test]
fn without_sort_the_table_is_still_grouped_by_file() {
    let (stdout, _, code) = rustcx(&["--min", "4", "fixtures/"]);
    assert_eq!(code, 0);
    assert!(!stdout.contains("Location"), "got:\n{stdout}");
    assert!(
        stdout.lines().any(|l| l == "fixtures/patterns.rs"),
        "got:\n{stdout}"
    );
}

#[test]
fn a_config_file_supplies_the_max_gate() {
    let dir = temp_dir("cli-config");
    std::fs::write(dir.join("rustcx.toml"), "max = 2\n").unwrap();
    std::fs::write(dir.join("busy.rs"), "fn f(a: bool) { if a {} if a {} if a {} }\n").unwrap();

    // Complexity 4 is over `max = 2`, so the gate fails with no --max flag.
    let out = run_in(&dir, &["busy.rs"]);
    assert_eq!(out.status.code(), Some(1), "rustcx.toml gates the run");
}

#[test]
fn a_flag_overrides_the_config_file() {
    let dir = temp_dir("cli-override");
    std::fs::write(dir.join("rustcx.toml"), "max = 2\n").unwrap();
    std::fs::write(dir.join("busy.rs"), "fn f(a: bool) { if a {} if a {} if a {} }\n").unwrap();

    let out = run_in(&dir, &["--max", "10", "busy.rs"]);
    assert_eq!(out.status.code(), Some(0), "the flag wins over the file");
}

#[test]
fn a_broken_config_file_is_reported() {
    let dir = temp_dir("cli-broken");
    std::fs::write(dir.join("rustcx.toml"), "treshold = 8\n").unwrap();
    std::fs::write(dir.join("x.rs"), "fn f() {}\n").unwrap();

    let out = run_in(&dir, &["x.rs"]);
    assert_eq!(out.status.code(), Some(2), "a typo'd key is loud, not silent");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("rustcx.toml"), "got: {err}");
}

/// Run `cmplx` (both languages) with `dir` as the working directory.
fn cmplx_in(dir: &Path, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(bin("cmplx"))
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .args(args)
        .output()
        .expect("run cmplx");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

const TS_SOURCE: &str = "export function pick(a: boolean) { return a ? 1 : 2; }\n";
const RUST_SOURCE: &str = "fn gate(a: bool) { if a {} }\n";

#[test]
fn cmplx_scores_js_and_rust_in_one_walk() {
    let dir = temp_dir("mixed");
    std::fs::write(dir.join("a.ts"), TS_SOURCE).unwrap();
    std::fs::write(dir.join("b.rs"), RUST_SOURCE).unwrap();

    let (stdout, stderr, code) = cmplx_in(&dir, &["."]);
    assert_eq!(code, 0, "stderr:\n{stderr}");
    assert!(stdout.contains("pick"), "the TypeScript row:\n{stdout}");
    assert!(stdout.contains("gate"), "the Rust row:\n{stdout}");
    assert!(stdout.contains("2 files"), "the summary:\n{stdout}");
}

#[test]
fn a_parse_failure_in_one_language_does_not_stop_the_others() {
    let dir = temp_dir("mixed-bad");
    std::fs::write(dir.join("a.ts"), TS_SOURCE).unwrap();
    std::fs::write(dir.join("bad.rs"), "fn f( {").unwrap();

    let (stdout, stderr, code) = cmplx_in(&dir, &["."]);
    assert_eq!(code, 2, "a parse failure is exit 2");
    assert!(stdout.contains("pick"), "a.ts is still scored:\n{stdout}");
    assert!(
        stderr.contains("bad.rs") && stderr.contains("parse error"),
        "the warning names the file:\n{stderr}"
    );
}

#[test]
fn an_unclaimed_explicit_file_exits_two() {
    let dir = temp_dir("unclaimed");
    std::fs::write(dir.join("README.md"), "# hi\n").unwrap();

    let (_, stderr, code) = cmplx_in(&dir, &["README.md"]);
    assert_eq!(code, 2);
    assert!(stderr.contains("no language for README.md"), "got: {stderr}");
}

#[test]
fn rustcx_scores_an_explicit_file_regardless_of_extension() {
    let dir = temp_dir("snippet");
    std::fs::write(dir.join("snippet.txt"), RUST_SOURCE).unwrap();

    let out = run_in(&dir, &["snippet.txt"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("gate"), "got:\n{stdout}");
}

#[test]
fn an_alias_warns_once_per_config_key_it_ignores_and_still_runs() {
    let dir = temp_dir("cli-alias-ignored");
    std::fs::write(
        dir.join("cmplx.toml"),
        "question = true\nlogical = true\n[rust]\ntests = false\n",
    )
    .unwrap();
    std::fs::write(dir.join("x.js"), "function f() {}\n").unwrap();

    let out = Command::new(bin("jscx"))
        .current_dir(&dir)
        .env("NO_COLOR", "1")
        .arg("x.js")
        .output()
        .expect("run jscx");
    assert_eq!(out.status.code(), Some(0));
    let file = dir.join("cmplx.toml").canonicalize().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    let file = file.display();
    assert_eq!(
        err,
        format!(
            "jscx: {file}: ignoring unknown key `question`\n\
             jscx: {file}: ignoring unknown table `[rust]`\n"
        )
    );
}
