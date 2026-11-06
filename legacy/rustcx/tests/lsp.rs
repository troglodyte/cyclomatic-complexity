//! End-to-end: spawn the real binary, speak real frames to it.
//!
//! jscx has no test at this layer, and framing is where hand-rolled servers
//! break — a length computed in chars rather than bytes, a missing `\r`, a
//! response written before the header.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdout, Command, Stdio};

struct Server {
    child: Child,
    out: BufReader<ChildStdout>,
}

impl Server {
    fn start() -> Server {
        let mut child = Command::new(env!("CARGO_BIN_EXE_rustcx"))
            .arg("--lsp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn rustcx --lsp");
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
    let mut s = Server::start();
    let reply = s.initialize();

    assert_eq!(reply["id"], 1);
    assert_eq!(reply["jsonrpc"], "2.0");
    assert_eq!(reply["result"]["capabilities"]["textDocumentSync"], 1);
    assert_eq!(reply["result"]["capabilities"]["hoverProvider"], true);
    assert!(reply.get("method").is_none(), "a response must not carry a method");
}

#[test]
fn shutdown_replies_null_and_exit_ends_the_process() {
    let mut s = Server::start();
    s.initialize();

    s.send(json!({ "jsonrpc": "2.0", "id": 2, "method": "shutdown" }));
    let reply = s.recv();
    assert_eq!(reply["id"], 2);
    assert!(reply["result"].is_null());

    s.send(json!({ "jsonrpc": "2.0", "method": "exit" }));
    let status = s.child.wait().unwrap();
    assert!(status.success(), "exit must end the process cleanly");
}

#[test]
fn an_unknown_request_gets_method_not_found() {
    let mut s = Server::start();
    s.initialize();

    // A request carries an id and a client waits on it. Ignoring one — as jscx
    // does — can hang a client indefinitely.
    s.send(json!({ "jsonrpc": "2.0", "id": 3, "method": "textDocument/codeLens" }));
    let reply = s.recv();
    assert_eq!(reply["id"], 3);
    assert_eq!(reply["error"]["code"], -32601);
}

#[test]
fn an_unknown_notification_is_ignored() {
    let mut s = Server::start();
    s.initialize();

    // No id, so no reply is owed. The next request must still be answered,
    // which proves the loop did not wedge.
    s.send(json!({ "jsonrpc": "2.0", "method": "$/setTrace", "params": { "value": "off" } }));
    s.send(json!({ "jsonrpc": "2.0", "id": 4, "method": "shutdown" }));
    assert_eq!(s.recv()["id"], 4);
}

#[test]
fn a_malformed_frame_does_not_take_the_server_down() {
    let mut s = Server::start();
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
    let mut s = Server::start();
    s.initialize();
    s.did_open("file:///x.rs", &format!("{}{}", branchy("small", 1), branchy("big", 21)));

    let params = s.diagnostics();
    assert_eq!(params["uri"], "file:///x.rs");
    let list = params["diagnostics"].as_array().unwrap();
    assert_eq!(list.len(), 1, "only `big` is over the default threshold of 11");
    assert_eq!(list[0]["message"], "Complexity is 22 for fn 'big'");
    assert_eq!(list[0]["severity"], 1, "22 is at least twice 11");
}

#[test]
fn a_change_republishes() {
    let mut s = Server::start();
    s.initialize();
    s.did_open("file:///x.rs", &branchy("f", 20));
    assert_eq!(s.diagnostics()["diagnostics"].as_array().unwrap().len(), 1);

    s.did_change("file:///x.rs", &branchy("f", 1));
    assert!(
        s.diagnostics()["diagnostics"].as_array().unwrap().is_empty(),
        "an empty list is how a client is told the warnings are gone"
    );
}

#[test]
fn the_threshold_comes_from_initialization_options() {
    let mut s = Server::start();
    s.send(json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "rootUri": null, "capabilities": {}, "initializationOptions": { "threshold": 3 } }
    }));
    s.recv();

    s.did_open("file:///x.rs", &branchy("f", 3));
    let list = s.diagnostics()["diagnostics"].as_array().unwrap().clone();
    assert_eq!(list.len(), 1, "complexity 4 is over a threshold of 3");
}

#[test]
fn closing_a_document_clears_its_diagnostics() {
    let mut s = Server::start();
    s.initialize();
    s.did_open("file:///x.rs", &branchy("f", 20));
    assert_eq!(s.diagnostics()["diagnostics"].as_array().unwrap().len(), 1);

    s.send(json!({
        "jsonrpc": "2.0", "method": "textDocument/didClose",
        "params": { "textDocument": { "uri": "file:///x.rs" } }
    }));
    assert!(s.diagnostics()["diagnostics"].as_array().unwrap().is_empty());
}

#[test]
fn hover_returns_the_explain_breakdown() {
    let mut s = Server::start();
    s.initialize();
    s.did_open("file:///x.rs", "fn describe(a: bool, b: bool) {\n    if a && b {}\n}\n");
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

#[test]
fn hover_on_an_unknown_document_is_null_not_an_error() {
    let mut s = Server::start();
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
    assert!(reply["result"].is_null(), "a request must always be answered");
    assert!(reply.get("error").is_none(), "but this is not an error");
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
    let mut s = Server::start();
    s.initialize();
    s.did_open("file:///x.rs", &branchy("f", 20));
    assert_eq!(s.diagnostics()["diagnostics"].as_array().unwrap().len(), 1);

    // Mid-edit: an unclosed brace. Nothing may be published — the warnings on
    // screen must not flash — but the failure is logged.
    s.did_change("file:///x.rs", "fn f(a: bool) { if a {");
    let logged = s.log_message();
    assert!(logged.contains("rustcx"), "got: {logged}");

    // The next well-formed change still publishes, proving nothing wedged.
    s.did_change("file:///x.rs", &branchy("f", 1));
    assert!(s.diagnostics()["diagnostics"].as_array().unwrap().is_empty());
}

#[test]
fn hover_still_answers_from_the_last_good_parse() {
    let mut s = Server::start();
    s.initialize();
    s.did_open("file:///x.rs", "fn describe(a: bool, b: bool) {\n    if a && b {}\n}\n");
    s.diagnostics();

    s.did_change("file:///x.rs", "fn describe(a: bool, b: bool) {\n    if a && b {");
    s.log_message();

    s.send(json!({
        "jsonrpc": "2.0", "id": 11, "method": "textDocument/hover",
        "params": {
            "textDocument": { "uri": "file:///x.rs" },
            "position": { "line": 0, "character": 3 }
        }
    }));
    let value = s.recv()["result"]["contents"]["value"].as_str().unwrap().to_string();
    assert!(value.contains("describe"), "hover survives an unparseable buffer");
}

#[test]
fn an_identical_parse_error_is_logged_once() {
    let mut s = Server::start();
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
    assert_eq!(next["id"], 12, "a repeated identical error must not log again");
    assert!(first.contains("rustcx"));
}
