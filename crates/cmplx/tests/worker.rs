//! The hidden `--worker` mode of the real binaries: one JSON request per line
//! in, one JSON response per line out.

mod common;

use common::bin;
use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Output, Stdio};

/// Run `program --worker` with `input` on stdin until it exits.
fn worker(program: &str, input: &str) -> Output {
    let mut child = Command::new(bin(program))
        .arg("--worker")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the worker");
    let mut stdin = child.stdin.take().expect("piped stdin");
    stdin
        .write_all(input.as_bytes())
        .expect("write the requests");
    drop(stdin);
    child.wait_with_output().expect("wait for the worker")
}

fn responses(out: &Output) -> Vec<Value> {
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).expect("each response is JSON"))
        .collect()
}

fn request(id: u64, lang: &str, path: &str, text: &str) -> String {
    format!(
        "{}\n",
        json!({ "id": id, "lang": lang, "path": path, "text": text, "options": {} })
    )
}

#[test]
fn cmplx_worker_round_trips_a_small_file() {
    let out = worker(
        "cmplx",
        &request(1, "rust", "a.rs", "fn gate(a: bool) { if a {} }\n"),
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let responses = responses(&out);
    assert_eq!(responses.len(), 1, "{responses:?}");
    assert_eq!(responses[0]["id"], 1);
    let function = &responses[0]["functions"][0];
    assert_eq!(
        *function,
        json!({ "n": "gate", "k": "fn", "l": 1, "c": 2, "p": [{ "l": 1, "b": "if" }] }),
        "the compact worker form"
    );
}

#[test]
fn a_worker_serves_requests_in_order_until_its_input_ends() {
    let input = format!(
        "{}{}",
        request(
            1,
            "js",
            "a.ts",
            "function f(a: boolean) { return a ? 1 : 2; }\n"
        ),
        request(2, "rust", "b.rs", "fn f( {"),
    );
    let out = worker("cmplx", &input);
    assert!(out.status.success());
    let responses = responses(&out);
    assert_eq!(responses[0]["id"], 1);
    assert_eq!(responses[0]["functions"][0]["c"], 2);
    assert_eq!(responses[1]["id"], 2);
    assert_eq!(responses[1]["error"]["kind"], "syntax");
}

#[test]
fn an_alias_worker_knows_only_its_language() {
    let out = worker("jscx", &request(1, "rust", "a.rs", "fn f() {}\n"));
    assert!(out.status.success());
    assert_eq!(
        responses(&out)[0]["error"]["message"],
        "unknown language `rust`"
    );
}

#[test]
fn the_worker_flag_is_not_in_help() {
    let out = Command::new(bin("cmplx"))
        .arg("--help")
        .output()
        .expect("run cmplx --help");
    assert!(!String::from_utf8_lossy(&out.stdout).contains("--worker"));
}

/// Source bytes per worst-case sample: big enough that per-response overhead
/// is noise, small enough for a debug build.
const WORST_CASE_SAMPLE_BYTES: usize = 100_000;

/// `unit` repeated to about [`WORST_CASE_SAMPLE_BYTES`].
fn repeated(unit: &str) -> String {
    unit.repeat(WORST_CASE_SAMPLE_BYTES / unit.len())
}

/// The sources that make the most response per source byte, as measured on
/// the release binary (see `MAX_RESPONSE_RATIO`): the most functions per byte
/// and the most points per byte, in each language. Rust closures nest only
/// about 4000 deep before `syn` overflows, so they come in groups: 200 deep
/// here, since `bin` is a debug build, which overflows below 1000 (2000 deep
/// on the release binary is the measured worst, 25.2). Nested arrows take
/// quadratic time to analyze, so their sample is a fifth of the size.
fn worst_case_samples() -> Vec<(&'static str, &'static str, String, Value)> {
    let nested_closures = format!("fn f(){{{}0;}}", "||".repeat(200));
    let nested_arrows = "x=>".repeat(WORST_CASE_SAMPLE_BYTES / 5 / 3);
    vec![
        ("js", "a.js", repeated("x=>0;"), json!({})),
        ("js", "a.js", format!("[{}]", repeated("()=>0,")), json!({})),
        ("js", "a.js", format!("{nested_arrows}0"), json!({})),
        (
            "js",
            "a.js",
            format!("{}0", nested_arrows.replace("=>", "=>\n")),
            json!({}),
        ),
        ("js", "a.js", format!("x=>{}a", repeated("a||")), json!({})),
        ("rust", "a.rs", repeated("fn a(){}"), json!({})),
        (
            "rust",
            "a.rs",
            format!("fn f(){{{}}}", repeated("||0;")),
            json!({}),
        ),
        ("rust", "a.rs", repeated(&nested_closures), json!({})),
        (
            "rust",
            "a.rs",
            repeated(&format!(
                "fn f(){{match 0{{{}1=>0,_=>0}}}}",
                "1|".repeat(100)
            )),
            json!({}),
        ),
        (
            "rust",
            "a.rs",
            repeated(&format!("fn f(){{a{};}}", "?".repeat(100))),
            json!({ "question": true }),
        ),
    ]
}

#[test]
fn the_densest_responses_stay_under_half_the_response_cap() {
    let ratio_cap = cmplx_core::supervisor::MAX_RESPONSE_RATIO as f64;
    for (lang, path, text, options) in worst_case_samples() {
        let line = json!({ "id": 1, "lang": lang, "path": path, "text": text, "options": options });
        let out = worker("cmplx", &format!("{line}\n"));
        let response = responses(&out).pop().expect("one response");
        assert!(
            response["functions"].is_array(),
            "{lang} sample was not scored: {response}"
        );
        let ratio = out.stdout.len() as f64 / text.len() as f64;
        let head: String = text.chars().take(30).collect();
        assert!(
            ratio * 2.0 <= ratio_cap,
            "{head:?}…: {ratio:.2} response bytes per source byte leaves less than 2x under the cap of {ratio_cap}"
        );
    }
}
