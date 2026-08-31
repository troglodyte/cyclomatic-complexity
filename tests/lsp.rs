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
