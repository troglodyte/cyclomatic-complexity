use std::process::Command;

fn rustcx(args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_rustcx"))
        .args(args)
        .output()
        .expect("binary should run");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
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
fn stdin_is_read_with_a_dash() {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = Command::new(env!("CARGO_BIN_EXE_rustcx"))
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
