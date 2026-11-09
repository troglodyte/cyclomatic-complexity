//! End-to-end: spawn the real binary, speak real frames to it.
//!
//! Framing is where hand-rolled servers break — a length computed in chars
//! rather than bytes, a missing `\r`, a response written before the header.
//! The Rust cases run against both `cmplx --lsp` and `rustcx --lsp`, which must
//! behave identically on a `.rs` document; the JS cases against `cmplx` and
//! `jscx`.

mod common;

use common::bin;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdout, Command, Stdio};

/// The servers that score `.rs` documents.
const RUST_SERVERS: [&str; 2] = ["cmplx", "rustcx"];
/// The servers that score JavaScript and TypeScript documents.
const JS_SERVERS: [&str; 2] = ["cmplx", "jscx"];

struct Server {
    child: Child,
    out: BufReader<ChildStdout>,
}

impl Server {
    fn start(program: &str) -> Server {
        let mut child = Command::new(bin(program))
            .arg("--lsp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap_or_else(|e| panic!("spawn {program} --lsp: {e}"));
        let out = BufReader::new(child.stdout.take().unwrap());
        Server { child, out }
    }

    fn send(&mut self, msg: Value) {
        let body = msg.to_string();
        let stdin = self.child.stdin.as_mut().unwrap();
        // Content-Length is in BYTES, which is why body.len() is right here and
        // a char count would not be.
        write!(stdin, "Content-Length: {}\r\n\r\n{}", body.len(), body).unwrap();
        stdin.flush().unwrap();
    }

    fn recv(&mut self) -> Value {
        let mut len = 0usize;
        loop {
            let mut line = String::new();
            let n = self.out.read_line(&mut line).unwrap();
            assert!(n > 0, "server closed stdout before sending a message");
            if line == "\r\n" || line == "\n" {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                if name.trim().eq_ignore_ascii_case("content-length") {
                    len = value.trim().parse().unwrap();
                }
            }
        }
        assert!(len > 0, "a frame arrived with no Content-Length");
        let mut body = vec![0u8; len];
        self.out.read_exact(&mut body).unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    fn initialize(&mut self) -> Value {
        self.send(json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "rootUri": null, "capabilities": {} }
        }));
        self.recv()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn initialize_advertises_the_capabilities_we_implement() {
    for program in RUST_SERVERS {
        let mut s = Server::start(program);
        let reply = s.initialize();

        assert_eq!(reply["id"], 1);
        assert_eq!(reply["jsonrpc"], "2.0");
        assert_eq!(reply["result"]["capabilities"]["textDocumentSync"], 1);
        assert_eq!(reply["result"]["capabilities"]["hoverProvider"], true);
        assert!(
            reply.get("method").is_none(),
            "a response must not carry a method"
        );
    }
}

#[test]
fn shutdown_replies_null_and_exit_ends_the_process() {
    for program in RUST_SERVERS {
        let mut s = Server::start(program);
        s.initialize();

        s.send(json!({ "jsonrpc": "2.0", "id": 2, "method": "shutdown" }));
        let reply = s.recv();
        assert_eq!(reply["id"], 2);
        assert!(reply["result"].is_null());

        s.send(json!({ "jsonrpc": "2.0", "method": "exit" }));
        let status = s.child.wait().unwrap();
        assert!(status.success(), "exit must end the process cleanly");
    }
}

#[test]
fn an_unknown_request_gets_method_not_found() {
    for program in RUST_SERVERS {
        let mut s = Server::start(program);
        s.initialize();

        // A request carries an id and a client waits on it. Ignoring one — as jscx
        // does — can hang a client indefinitely.
        s.send(json!({ "jsonrpc": "2.0", "id": 3, "method": "textDocument/codeLens" }));
        let reply = s.recv();
        assert_eq!(reply["id"], 3);
        assert_eq!(reply["error"]["code"], -32601);
    }
}

#[test]
fn an_unknown_notification_is_ignored() {
    for program in RUST_SERVERS {
        let mut s = Server::start(program);
        s.initialize();

        // No id, so no reply is owed. The next request must still be answered,
        // which proves the loop did not wedge.
        s.send(json!({ "jsonrpc": "2.0", "method": "$/setTrace", "params": { "value": "off" } }));
        s.send(json!({ "jsonrpc": "2.0", "id": 4, "method": "shutdown" }));
        assert_eq!(s.recv()["id"], 4);
    }
}

#[test]
fn a_malformed_frame_does_not_take_the_server_down() {
    for program in RUST_SERVERS {
        let mut s = Server::start(program);
        s.initialize();

        {
            let stdin = s.child.stdin.as_mut().unwrap();
            // Junk header, then a well-formed frame.
            write!(stdin, "Content-Length: banana\r\n\r\n").unwrap();
            stdin.flush().unwrap();
        }
        s.send(json!({ "jsonrpc": "2.0", "id": 5, "method": "shutdown" }));
        assert_eq!(s.recv()["id"], 5);
    }
}

impl Server {
    fn did_open(&mut self, uri: &str, text: &str) {
        self.send(json!({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": { "textDocument": { "uri": uri, "languageId": "rust", "version": 1, "text": text } }
        }));
    }

    fn did_change(&mut self, uri: &str, text: &str) {
        self.send(json!({
            "jsonrpc": "2.0", "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": uri, "version": 2 },
                "contentChanges": [ { "text": text } ]
            }
        }));
    }

    /// The next `publishDiagnostics`, skipping any log messages.
    fn diagnostics(&mut self) -> Value {
        loop {
            let msg = self.recv();
            if msg["method"] == "textDocument/publishDiagnostics" {
                return msg["params"].clone();
            }
        }
    }
}

/// A function of complexity `1 + branches`, on its own line.
fn branchy(name: &str, branches: usize) -> String {
    format!("fn {name}(a: bool) {{ {} }}\n", "if a {} ".repeat(branches))
}

#[test]
fn opening_a_document_publishes_diagnostics() {
    for program in RUST_SERVERS {
        let mut s = Server::start(program);
        s.initialize();
        s.did_open(
            "file:///x.rs",
            &format!("{}{}", branchy("small", 1), branchy("big", 21)),
        );

        let params = s.diagnostics();
        assert_eq!(params["uri"], "file:///x.rs");
        let list = params["diagnostics"].as_array().unwrap();
        assert_eq!(
            list.len(),
            1,
            "only `big` is over the default threshold of 11"
        );
        assert_eq!(list[0]["message"], "Complexity is 22 for fn 'big'");
        assert_eq!(list[0]["severity"], 1, "22 is at least twice 11");
    }
}

#[test]
fn a_change_republishes() {
    for program in RUST_SERVERS {
        let mut s = Server::start(program);
        s.initialize();
        s.did_open("file:///x.rs", &branchy("f", 20));
        assert_eq!(s.diagnostics()["diagnostics"].as_array().unwrap().len(), 1);

        s.did_change("file:///x.rs", &branchy("f", 1));
        assert!(
            s.diagnostics()["diagnostics"]
                .as_array()
                .unwrap()
                .is_empty(),
            "an empty list is how a client is told the warnings are gone"
        );
    }
}

#[test]
fn the_threshold_comes_from_initialization_options() {
    for program in RUST_SERVERS {
        let mut s = Server::start(program);
        s.send(json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "rootUri": null, "capabilities": {}, "initializationOptions": { "threshold": 3 } }
        }));
        s.recv();

        s.did_open("file:///x.rs", &branchy("f", 3));
        let list = s.diagnostics()["diagnostics"].as_array().unwrap().clone();
        assert_eq!(list.len(), 1, "complexity 4 is over a threshold of 3");
    }
}

#[test]
fn closing_a_document_clears_its_diagnostics() {
    for program in RUST_SERVERS {
        let mut s = Server::start(program);
        s.initialize();
        s.did_open("file:///x.rs", &branchy("f", 20));
        assert_eq!(s.diagnostics()["diagnostics"].as_array().unwrap().len(), 1);

        s.send(json!({
            "jsonrpc": "2.0", "method": "textDocument/didClose",
            "params": { "textDocument": { "uri": "file:///x.rs" } }
        }));
        assert!(s.diagnostics()["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty());
    }
}

#[test]
fn hover_returns_the_explain_breakdown() {
    for program in RUST_SERVERS {
        let mut s = Server::start(program);
        s.initialize();
        s.did_open(
            "file:///x.rs",
            "fn describe(a: bool, b: bool) {\n    if a && b {}\n}\n",
        );
        s.diagnostics(); // drain the publish that follows didOpen

        s.send(json!({
            "jsonrpc": "2.0", "id": 9, "method": "textDocument/hover",
            "params": {
                "textDocument": { "uri": "file:///x.rs" },
                "position": { "line": 0, "character": 3 }
            }
        }));
        let reply = s.recv();
        assert_eq!(reply["id"], 9);
        let value = reply["result"]["contents"]["value"].as_str().unwrap();
        assert!(value.contains("describe"), "got: {value}");
        assert!(value.contains("complexity 3"), "got: {value}");
        assert_eq!(reply["result"]["contents"]["kind"], "markdown");
    }
}

#[test]
fn hover_on_an_unknown_document_is_null_not_an_error() {
    for program in RUST_SERVERS {
        let mut s = Server::start(program);
        s.initialize();

        s.send(json!({
            "jsonrpc": "2.0", "id": 10, "method": "textDocument/hover",
            "params": {
                "textDocument": { "uri": "file:///never-opened.rs" },
                "position": { "line": 0, "character": 0 }
            }
        }));
        let reply = s.recv();
        assert_eq!(reply["id"], 10);
        assert!(
            reply["result"].is_null(),
            "a request must always be answered"
        );
        assert!(reply.get("error").is_none(), "but this is not an error");
    }
}

impl Server {
    /// The next `window/logMessage`, skipping diagnostics.
    fn log_message(&mut self) -> String {
        loop {
            let msg = self.recv();
            if msg["method"] == "window/logMessage" {
                return msg["params"]["message"].as_str().unwrap().to_string();
            }
        }
    }
}

#[test]
fn a_parse_failure_keeps_the_last_good_diagnostics() {
    for program in RUST_SERVERS {
        let mut s = Server::start(program);
        s.initialize();
        s.did_open("file:///x.rs", &branchy("f", 20));
        assert_eq!(s.diagnostics()["diagnostics"].as_array().unwrap().len(), 1);

        // Mid-edit: an unclosed brace. Nothing may be published — the warnings on
        // screen must not flash — but the failure is logged.
        s.did_change("file:///x.rs", "fn f(a: bool) { if a {");
        let logged = s.log_message();
        assert!(logged.contains(program), "{program}: got: {logged}");

        // The next well-formed change still publishes, proving nothing wedged.
        s.did_change("file:///x.rs", &branchy("f", 1));
        assert!(s.diagnostics()["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty());
    }
}

#[test]
fn hover_still_answers_from_the_last_good_parse() {
    for program in RUST_SERVERS {
        let mut s = Server::start(program);
        s.initialize();
        s.did_open(
            "file:///x.rs",
            "fn describe(a: bool, b: bool) {\n    if a && b {}\n}\n",
        );
        s.diagnostics();

        s.did_change(
            "file:///x.rs",
            "fn describe(a: bool, b: bool) {\n    if a && b {",
        );
        s.log_message();

        s.send(json!({
            "jsonrpc": "2.0", "id": 11, "method": "textDocument/hover",
            "params": {
                "textDocument": { "uri": "file:///x.rs" },
                "position": { "line": 0, "character": 3 }
            }
        }));
        let value = s.recv()["result"]["contents"]["value"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            value.contains("describe"),
            "hover survives an unparseable buffer"
        );
    }
}

#[test]
fn an_identical_parse_error_is_logged_once() {
    for program in RUST_SERVERS {
        let mut s = Server::start(program);
        s.initialize();
        s.did_open("file:///x.rs", &branchy("f", 20));
        s.diagnostics();

        s.did_change("file:///x.rs", "fn f(a: bool) { if a {");
        let first = s.log_message();

        // The same broken text again: a held keystroke inside an unclosed brace
        // must not write a line per character.
        s.did_change("file:///x.rs", "fn f(a: bool) { if a {");
        // Nothing is owed for that change, so the next message must be the reply to
        // this shutdown — if a second log line were sent, this assert catches it.
        s.send(json!({ "jsonrpc": "2.0", "id": 12, "method": "shutdown" }));
        let next = s.recv();
        assert_eq!(
            next["id"], 12,
            "a repeated identical error must not log again"
        );
        assert!(first.contains(program));
    }
}

/// A one-line JavaScript function of complexity `1 + branches`.
fn js_branchy(branches: usize) -> String {
    format!("function f(a) {{ {}}}\n", "if (a) {} ".repeat(branches))
}

#[test]
fn rust_diagnostics_name_the_serving_program_as_source() {
    for program in RUST_SERVERS {
        let mut s = Server::start(program);
        s.initialize();
        s.did_open("file:///x.rs", &branchy("f", 20));
        let list = s.diagnostics()["diagnostics"].as_array().unwrap().clone();
        assert_eq!(list[0]["source"], program);
    }
}

#[test]
fn js_diagnostics_match_jscx() {
    for program in JS_SERVERS {
        let mut s = Server::start(program);
        s.initialize();
        let text = js_branchy(11);
        s.did_open("file:///x/a.ts", &text);

        let params = s.diagnostics();
        assert_eq!(params["uri"], "file:///x/a.ts");
        let list = params["diagnostics"].as_array().unwrap();
        assert_eq!(list.len(), 1, "{program}: {list:?}");
        let width = text.lines().next().unwrap().encode_utf16().count();
        assert_eq!(
            list[0],
            json!({
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 0, "character": width },
                },
                "severity": 2,
                "source": program,
                "message": "Complexity is 12 for function 'f'",
            }),
            "{program}"
        );
    }
}

#[test]
fn a_document_no_language_claims_gets_no_diagnostics() {
    for program in ["cmplx", "jscx", "rustcx"] {
        let mut s = Server::start(program);
        s.initialize();
        s.did_open(
            "file:///x/README.md",
            "# if if if if if if if if if if if if\n",
        );

        let params = s.diagnostics();
        assert_eq!(params["uri"], "file:///x/README.md");
        assert_eq!(params["diagnostics"], json!([]), "{program}");

        // The server is still serving: a following request is answered.
        s.send(json!({ "jsonrpc": "2.0", "id": 7, "method": "shutdown" }));
        assert_eq!(s.recv()["id"], 7, "{program}");
    }
}

#[test]
fn a_document_of_a_language_the_alias_lacks_gets_no_diagnostics() {
    let mut s = Server::start("rustcx");
    s.initialize();
    s.did_open("file:///x/a.ts", &js_branchy(30));
    assert_eq!(s.diagnostics()["diagnostics"], json!([]));
}

#[test]
fn dts_documents_are_skipped_on_the_server() {
    for program in JS_SERVERS {
        let mut s = Server::start(program);
        s.initialize();
        s.did_open("file:///x/a.d.ts", &js_branchy(30));
        assert_eq!(s.diagnostics()["diagnostics"], json!([]), "{program}");
    }
}

#[test]
fn include_dts_in_initialization_options_scores_declaration_files() {
    for program in JS_SERVERS {
        let mut s = Server::start(program);
        s.send(json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "rootUri": null, "capabilities": {},
                        "initializationOptions": { "js": { "include-dts": true } } }
        }));
        s.recv();
        s.did_open("file:///x/a.d.ts", &js_branchy(30));
        let list = s.diagnostics()["diagnostics"].as_array().unwrap().clone();
        assert_eq!(list.len(), 1, "{program}");
    }
}

/// `branchy("f", 5)` (complexity 6) followed by a `#[cfg(test)]` module holding
/// an equally branchy function.
fn with_tests_module() -> String {
    format!(
        "{}#[cfg(test)]\nmod tests {{\n{}}}\n",
        branchy("f", 5),
        branchy("t", 5)
    )
}

#[test]
fn flat_legacy_init_options_still_apply_to_rustcx() {
    let mut s = Server::start("rustcx");
    s.send(json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "rootUri": null, "capabilities": {},
                    "initializationOptions": { "threshold": 6, "tests": false } }
    }));
    s.recv();
    s.did_open("file:///x.rs", &with_tests_module());

    let list = s.diagnostics()["diagnostics"].as_array().unwrap().clone();
    assert_eq!(
        list.len(),
        1,
        "threshold 6 flags `f`; tests:false drops `t`: {list:?}"
    );
    assert_eq!(list[0]["message"], "Complexity is 6 for fn 'f'");
    assert_eq!(list[0]["severity"], 2);
}

#[test]
fn nested_init_options_apply_per_language() {
    let mut s = Server::start("cmplx");
    s.send(json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "rootUri": null, "capabilities": {},
                    "initializationOptions": { "threshold": 6, "rust": { "tests": false } } }
    }));
    s.recv();
    s.did_open("file:///x.rs", &with_tests_module());
    let list = s.diagnostics()["diagnostics"].as_array().unwrap().clone();
    assert_eq!(list.len(), 1, "{list:?}");
}

#[test]
fn an_unknown_init_option_is_logged_and_the_rest_still_apply() {
    for program in RUST_SERVERS {
        let mut s = Server::start(program);
        s.send(json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "rootUri": null, "capabilities": {},
                        "initializationOptions": { "treshold": 3, "threshold": 3 } }
        }));
        let mut logged = Vec::new();
        loop {
            let msg = s.recv();
            if msg["id"] == 1 {
                break;
            }
            logged.push(
                msg["params"]["message"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        // The warning may follow the reply; either way it names the key.
        s.did_open("file:///x.rs", &branchy("f", 3));
        let list = loop {
            let msg = s.recv();
            if msg["method"] == "textDocument/publishDiagnostics" {
                break msg["params"]["diagnostics"].as_array().unwrap().clone();
            }
            logged.push(
                msg["params"]["message"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
        };
        assert!(
            logged.iter().any(|m| m.contains("treshold")),
            "{program}: {logged:?}"
        );
        assert_eq!(list.len(), 1, "{program}: threshold 3 still applies");
    }
}

#[test]
fn a_cmplx_toml_under_the_root_sets_the_threshold() {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    for program in RUST_SERVERS {
        let n = N.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("cmplx-lsp-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("cmplx.toml"), "threshold = 3\n").unwrap();

        let mut s = Server::start(program);
        s.send(json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "rootUri": format!("file://{}", root.display()), "capabilities": {} }
        }));
        s.recv();
        s.did_open("file:///x.rs", &branchy("f", 3));
        let list = s.diagnostics()["diagnostics"].as_array().unwrap().clone();
        assert_eq!(
            list.len(),
            1,
            "{program}: complexity 4 is over the file's threshold of 3"
        );
    }
}

/// Move `path`'s mtime a minute on, so a rewrite within the same second is
/// still seen as a change.
fn bump_mtime(path: &std::path::Path) {
    let later = std::fs::metadata(path).unwrap().modified().unwrap()
        + std::time::Duration::from_secs(60);
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(later))
        .unwrap();
}

#[test]
fn editing_cmplx_toml_mid_session_changes_the_next_diagnostics() {
    for program in RUST_SERVERS {
        let root = std::env::temp_dir().join(format!(
            "cmplx-lsp-reload-{program}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let config = root.join("cmplx.toml");
        std::fs::write(&config, "threshold = 3\n").unwrap();

        let mut s = Server::start(program);
        s.send(json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "rootUri": format!("file://{}", root.display()), "capabilities": {} }
        }));
        s.recv();
        let text = branchy("f", 3);
        s.did_open("file:///x.rs", &text);
        assert_eq!(s.diagnostics()["diagnostics"].as_array().unwrap().len(), 1, "{program}");

        std::fs::write(&config, "threshold = 10\n").unwrap();
        bump_mtime(&config);
        s.did_change("file:///x.rs", &text);
        assert_eq!(
            s.diagnostics()["diagnostics"],
            json!([]),
            "{program}: complexity 4 is within the new threshold of 10"
        );
    }
}

#[test]
fn an_untitled_document_gets_no_diagnostics() {
    for program in ["cmplx", "jscx", "rustcx"] {
        let mut s = Server::start(program);
        s.initialize();
        s.did_open("untitled:Untitled-1", &branchy("f", 30));
        let params = s.diagnostics();
        assert_eq!(params["uri"], "untitled:Untitled-1");
        assert_eq!(params["diagnostics"], json!([]), "{program}");
    }
}
