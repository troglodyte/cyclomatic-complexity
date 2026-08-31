//! A Language Server for rustcx: complexity diagnostics and hover.
//!
//! Hand-rolled JSON-RPC, as in jscx: `serde_json::json!` literals rather than
//! `lsp-types`, a blocking `Content-Length` loop rather than `tower-lsp`, and no
//! async runtime. One message is handled at a time; an analysis is
//! milliseconds, so there is nothing to overlap and no state to share.
//!
//! **stdout is the protocol.** Nothing in this module may print to it.

use crate::config::{FileConfig, Watched};
use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

/// Score at which a function starts warning. Twice this value is an error.
pub const DEFAULT_THRESHOLD: u32 = 11;

/// Upper bound on a JSON-RPC body, so a bogus `Content-Length` cannot ask us to
/// allocate an arbitrary amount of memory.
const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;

/// JSON-RPC error code for a method the server does not implement.
const METHOD_NOT_FOUND: i32 = -32601;

/// One open document: the last **successful** analysis of it.
///
/// The text is deliberately not kept. Full sync means every change carries the
/// whole buffer, so there is nothing to reconstruct, and the UTF-16 line widths
/// diagnostics need are computed at publish time and discarded.
// Task 3 is the first consumer; the allow goes away there.
#[allow(dead_code)]
#[derive(Default)]
struct Doc {
    funcs: Vec<crate::analyze::Func>,
    /// The last parse error logged for this document, for dedupe. Task 5.
    last_error: Option<String>,
}

struct Server {
    /// Task 3 is the first consumer; the allow goes away there.
    #[allow(dead_code)]
    docs: HashMap<String, Doc>,
    /// `initializationOptions`, which win over the file.
    init: FileConfig,
    /// The watched `rustcx.toml`, re-read on mtime change.
    config: Watched,
    stdout: io::Stdout,
}

impl Server {
    fn new() -> Server {
        Server {
            docs: HashMap::new(),
            init: FileConfig::default(),
            config: Watched::default(),
            stdout: io::stdout(),
        }
    }

    /// Settings in force right now: `initializationOptions` over the file over
    /// the built-in defaults.
    fn settings(&self) -> FileConfig {
        self.init.or(self.config.config())
    }

    /// The score at which a function warns. Errors at twice this.
    /// Task 3 is the first non-test consumer; the allow goes away there.
    #[allow(dead_code)]
    fn threshold(&self) -> u32 {
        self.settings().threshold.unwrap_or(DEFAULT_THRESHOLD)
    }

    fn send(&mut self, msg: serde_json::Value) -> io::Result<()> {
        let body = msg.to_string();
        // Content-Length is in bytes.
        write!(self.stdout, "Content-Length: {}\r\n\r\n{}", body.len(), body)?;
        self.stdout.flush()
    }

    fn respond(&mut self, id: serde_json::Value, result: serde_json::Value) -> io::Result<()> {
        // A JSON-RPC response must not carry a `method` member at all, so it is
        // omitted rather than serialized as null.
        self.send(serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }))
    }

    fn respond_error(&mut self, id: serde_json::Value, code: i32, message: &str) -> io::Result<()> {
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message },
        }))
    }

    /// Handle `initialize`: read our settings, discover a `rustcx.toml` under
    /// the workspace root, and advertise what we implement.
    fn initialize(&mut self, params: &serde_json::Value) -> serde_json::Value {
        self.init = init_options(params);
        let (watched, err) = Watched::discover(&root_dir(params));
        self.config = watched;
        if let Some(e) = err {
            // Logged, not fatal: a bad config file must not prevent a session.
            let _ = self.log(&format!("config: {e}"));
        }
        serde_json::json!({
            "capabilities": {
                "textDocumentSync": 1, // Full sync
                "hoverProvider": true,
            }
        })
    }

    /// `window/logMessage` at type 4 (`Log`) — the client's language-server
    /// log. Never `showMessage`: none of this warrants interrupting anyone.
    fn log(&mut self, message: &str) -> io::Result<()> {
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "method": "window/logMessage",
            "params": { "type": 4, "message": format!("rustcx: {message}") },
        }))
    }
}

/// Read `initializationOptions` as a `FileConfig`.
///
/// The same shape as the file, so one merge rule covers both. Unknown keys are
/// rejected there too, but here that only means falling back to defaults —
/// refusing to initialize over a stray setting would be worse.
fn init_options(params: &serde_json::Value) -> FileConfig {
    let Some(opts) = params.get("initializationOptions") else {
        return FileConfig::default();
    };
    serde_json::from_value(opts.clone()).unwrap_or_default()
}

/// The directory config discovery starts from: `rootUri`, else the first
/// workspace folder, else the current directory.
fn root_dir(params: &serde_json::Value) -> PathBuf {
    let uri = params
        .get("rootUri")
        .and_then(serde_json::Value::as_str)
        .or_else(|| params["workspaceFolders"][0]["uri"].as_str());
    uri.and_then(uri_to_path).unwrap_or_else(|| PathBuf::from("."))
}

/// `file:///a/b.rs` → `/a/b.rs`. Percent-escapes are decoded; a non-`file:`
/// URI has no path and yields `None`.
pub(crate) fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // Strip an authority component ("localhost" or empty) up to the next `/`.
    let path = match rest.find('/') {
        Some(i) => &rest[i..],
        None => return None,
    };
    Some(PathBuf::from(percent_decode(path)))
}

/// Decode `%XX` escapes. Anything malformed is left as written.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (i + 2 < bytes.len()).then(|| std::str::from_utf8(&bytes[i + 1..i + 3]).ok()).flatten();
        match (bytes[i], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// Parse a `Content-Length` header value, rejecting anything unusable.
fn parse_content_length(line: &str) -> Option<usize> {
    let (name, value) = line.split_once(':')?;
    if !name.trim().eq_ignore_ascii_case("content-length") {
        return None;
    }
    let len: usize = value.trim().parse().ok()?;
    if len == 0 || len > MAX_BODY_BYTES {
        return None;
    }
    Some(len)
}

/// Read one framed message. `Ok(None)` at end of input.
fn read_message(stdin: &mut impl BufRead) -> io::Result<Option<serde_json::Value>> {
    let mut len = None;
    loop {
        let mut line = String::new();
        if stdin.read_line(&mut line)? == 0 {
            return Ok(None); // EOF
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        // Anything that is not a usable Content-Length is skipped rather than
        // fatal: a malformed frame must not take the server down.
        if let Some(n) = parse_content_length(&line) {
            len = Some(n);
        }
    }
    let Some(len) = len else {
        return Ok(Some(serde_json::Value::Null)); // header block with no length
    };
    let mut body = vec![0u8; len];
    stdin.read_exact(&mut body)?;
    Ok(Some(serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null)))
}

/// Run the server until `exit` or end of input.
pub fn run() -> io::Result<()> {
    let stdin = io::stdin();
    let mut stdin = stdin.lock();
    let mut server = Server::new();

    while let Some(msg) = read_message(&mut stdin)? {
        let Some(method) = msg["method"].as_str().map(str::to_string) else {
            continue; // not a request or notification we can act on
        };
        let id = msg.get("id").cloned();
        let params = msg.get("params").cloned().unwrap_or(serde_json::Value::Null);

        match method.as_str() {
            "initialize" => {
                let result = server.initialize(&params);
                if let Some(id) = id {
                    server.respond(id, result)?;
                }
            }
            "shutdown" => {
                if let Some(id) = id {
                    server.respond(id, serde_json::Value::Null)?;
                }
            }
            "exit" => return Ok(()),
            _ => {
                // A request carries an id and a client waits on it; ignoring one
                // can hang the client. A notification is owed no reply.
                if let Some(id) = id {
                    server.respond_error(id, METHOD_NOT_FOUND, &format!("unknown method: {method}"))?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_length_is_read_case_insensitively() {
        assert_eq!(parse_content_length("Content-Length: 42\r\n"), Some(42));
        assert_eq!(parse_content_length("content-length:42\r\n"), Some(42));
        assert_eq!(parse_content_length("CONTENT-LENGTH:   42  \r\n"), Some(42));
    }

    #[test]
    fn an_unusable_content_length_is_rejected() {
        assert_eq!(parse_content_length("Content-Type: application/json\r\n"), None);
        assert_eq!(parse_content_length("Content-Length: banana\r\n"), None);
        assert_eq!(parse_content_length("Content-Length: 0\r\n"), None);
        assert_eq!(parse_content_length("nonsense\r\n"), None);
        assert_eq!(
            parse_content_length(&format!("Content-Length: {}\r\n", MAX_BODY_BYTES + 1)),
            None,
            "an oversized body must not be allocated"
        );
    }

    #[test]
    fn a_file_uri_becomes_a_path() {
        assert_eq!(uri_to_path("file:///a/b.rs"), Some(PathBuf::from("/a/b.rs")));
        assert_eq!(
            uri_to_path("file:///a/my%20crate/b.rs"),
            Some(PathBuf::from("/a/my crate/b.rs")),
            "percent escapes are decoded"
        );
        assert_eq!(uri_to_path("untitled:Untitled-1"), None);
    }

    #[test]
    fn initialization_options_are_read_as_a_config() {
        let params = serde_json::json!({
            "initializationOptions": { "threshold": 6, "tests": false }
        });
        let cfg = init_options(&params);
        assert_eq!(cfg.threshold, Some(6));
        assert_eq!(cfg.tests, Some(false));
        assert_eq!(cfg.logical, None, "an absent key stays unset");
    }

    #[test]
    fn absent_or_broken_initialization_options_fall_back() {
        assert_eq!(init_options(&serde_json::json!({})), FileConfig::default());
        let junk = serde_json::json!({ "initializationOptions": { "treshold": 6 } });
        assert_eq!(
            init_options(&junk),
            FileConfig::default(),
            "a stray setting must not prevent initialization"
        );
    }

    #[test]
    fn initialization_options_win_over_the_file() {
        let mut server = Server::new();
        server.init = FileConfig { threshold: Some(3), ..FileConfig::default() };
        assert_eq!(server.threshold(), 3);
        server.init = FileConfig::default();
        assert_eq!(server.threshold(), DEFAULT_THRESHOLD);
    }
}
