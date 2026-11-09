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
    assert_eq!(function["name"], "gate");
    assert_eq!(function["complexity"], 2);
    assert_eq!(function["kind"], json!({ "id": "fn", "label": "fn" }));
    assert_eq!(function["points"], json!([{ "line": 1, "branch": "if" }]));
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
    assert_eq!(responses[0]["functions"][0]["complexity"], 2);
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
