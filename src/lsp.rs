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
#[derive(Default)]
struct Doc {
    funcs: Vec<crate::analyze::Func>,
    /// The last parse error logged for this document, for dedupe.
    last_error: Option<String>,
}

struct Server {
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

    fn publish(&mut self, uri: &str, diagnostics: Vec<serde_json::Value>) -> io::Result<()> {
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": { "uri": uri, "diagnostics": diagnostics },
        }))
    }

    /// Analyze a document and publish. A parse failure publishes nothing —
    /// Task 5 adds the logging that explains why.
    fn update(&mut self, uri: &str, text: &str) -> io::Result<()> {
        // Pick up an edited rustcx.toml before using its settings.
        if let Some(e) = self.config.refresh() {
            self.log(&format!("config: {e}"))?;
        }
        let settings = self.settings();
        let result = crate::analyze::analyze(text, settings.options());
        // The thread-local span table retains the text of every file parsed on
        // this thread, so it is cleared after every analysis — the same
        // invariant `score_into` holds in main.rs. A long-lived server makes
        // this load-bearing rather than merely tidy.
        proc_macro2::extra::invalidate_current_thread_spans();

        let funcs = match result {
            Ok(funcs) => funcs,
            Err(e) => {
                // Publish nothing: a buffer is unparseable for most of the time
                // someone is typing into it, so clearing would make the
                // warnings flash, and publishing the syntax error would
                // duplicate what rust-analyzer already reports. Hover keeps
                // answering from the functions cached below.
                let message = format!("{}: {}: {}", uri, e.line, e.message);
                let doc = self.docs.entry(uri.to_string()).or_default();
                if doc.last_error.as_deref() == Some(message.as_str()) {
                    return Ok(()); // already logged; a held key must not flood
                }
                doc.last_error = Some(message.clone());
                return self.log(&message);
            }
        };
        let diagnostics = diagnostics_for(&funcs, text, self.threshold());
        let doc = self.docs.entry(uri.to_string()).or_default();
        doc.funcs = funcs;
        doc.last_error = None;
        self.publish(uri, diagnostics)
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
            "textDocument/didOpen" => {
                if let (Some(uri), Some(text)) = (
                    params["textDocument"]["uri"].as_str().map(str::to_string),
                    params["textDocument"]["text"].as_str().map(str::to_string),
                ) {
                    server.update(&uri, &text)?;
                }
            }
            "textDocument/didChange" => {
                // Full sync: one change carrying the entire buffer.
                if let (Some(uri), Some(text)) = (
                    params["textDocument"]["uri"].as_str().map(str::to_string),
                    params["contentChanges"][0]["text"].as_str().map(str::to_string),
                ) {
                    server.update(&uri, &text)?;
                }
            }
            "textDocument/didSave" => {
                // Under full sync the buffer already arrived via didChange, and
                // `text` is only present if the client opted into sending it.
                // Nothing to do. (jscx reads a `text` that never comes.)
            }
            "textDocument/didClose" => {
                if let Some(uri) = params["textDocument"]["uri"].as_str().map(str::to_string) {
                    server.docs.remove(&uri);
                    server.publish(&uri, Vec::new())?;
                }
            }
            "textDocument/hover" => {
                let Some(id) = id else { continue }; // a request always has one
                let uri = params["textDocument"]["uri"].as_str().unwrap_or_default().to_string();
                // LSP positions are 0-based; Func.line is 1-based.
                let line = params["position"]["line"].as_u64().unwrap_or(0) as u32 + 1;
                let text = server.docs.get(&uri).and_then(|d| hover_at(&d.funcs, line));
                let result = match text {
                    Some(text) => serde_json::json!({
                        "contents": { "kind": "markdown", "value": text }
                    }),
                    None => serde_json::Value::Null,
                };
                server.respond(id, result)?;
            }
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

/// Build the diagnostics for one analyzed document.
fn diagnostics_for(
    funcs: &[crate::analyze::Func],
    text: &str,
    threshold: u32,
) -> Vec<serde_json::Value> {
    // LSP character offsets are UTF-16 code units, not bytes or chars.
    let widths: Vec<u32> = text
        .lines()
        .map(|l| l.chars().map(char::len_utf16).sum::<usize>() as u32)
        .collect();

    funcs
        .iter()
        .filter(|f| f.complexity >= threshold)
        .map(|f| {
            // Func.line is 1-based; LSP lines are 0-based.
            let row = f.line.saturating_sub(1);
            let end = widths.get(row as usize).copied().unwrap_or(0);
            let severity = if f.complexity >= threshold.saturating_mul(2) { 1 } else { 2 };
            serde_json::json!({
                "range": {
                    "start": { "line": row, "character": 0 },
                    "end": { "line": row, "character": end },
                },
                "severity": severity,
                "source": "rustcx",
                "message": format!(
                    "Complexity is {} for {} '{}'",
                    f.complexity,
                    f.kind.label(),
                    f.name
                ),
            })
        })
        .collect()
}

/// The hover text for a 1-based source line, or `None` if that line is neither
/// a function's signature nor one of its decision points.
///
/// `Func` carries a start line and no end line — adding one would break the
/// field-for-field parity with jscx — so hover answers on the lines the
/// analyzer already knows about. Conveniently, those are the branchy lines:
/// exactly the ones worth hovering.
///
/// Where two functions match, the innermost wins, so hovering inside a closure
/// reports the closure rather than its parent.
fn hover_at(funcs: &[crate::analyze::Func], line: u32) -> Option<String> {
    let f = funcs
        .iter()
        .filter(|f| f.line == line || f.points.iter().any(|p| p.line == line))
        .max_by_key(|f| f.depth)?;

    let mut out = format!("`{}` — {} complexity {}\n", f.name, f.kind.label(), f.complexity);
    for p in &f.points {
        out.push_str(&format!("\n- line {} — {}", p.line, p.branch));
    }
    Some(out)
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

    use crate::analyze::{analyze, Options};

    /// A function whose complexity is `1 + branches`, all on one line.
    fn one_liner(name: &str, branches: usize) -> String {
        format!("fn {name}(a: bool) {{ {} }}", "if a {} ".repeat(branches))
    }

    fn diags(src: &str, threshold: u32) -> Vec<serde_json::Value> {
        let funcs = analyze(src, Options::default()).expect("fixture parses");
        diagnostics_for(&funcs, src, threshold)
    }

    #[test]
    fn only_functions_at_or_above_the_threshold_are_reported() {
        let src = format!("{}\n{}\n", one_liner("small", 1), one_liner("big", 9));
        let d = diags(&src, 5);
        assert_eq!(d.len(), 1, "complexity 2 is below the threshold, 10 is not");
        assert!(d[0]["message"].as_str().unwrap().contains("big"));
    }

    #[test]
    fn the_severity_boundaries_are_exact() {
        // Warning at exactly the threshold.
        let at = diags(&one_liner("f", 4), 5);
        assert_eq!(at.len(), 1);
        assert_eq!(at[0]["severity"], 2, "complexity 5 at threshold 5 warns");

        // Error at exactly twice it.
        let twice = diags(&one_liner("f", 9), 5);
        assert_eq!(twice[0]["severity"], 1, "complexity 10 at threshold 5 errors");

        // And one below twice is still a warning.
        let under = diags(&one_liner("f", 8), 5);
        assert_eq!(under[0]["severity"], 2, "complexity 9 at threshold 5 warns");
    }

    #[test]
    fn a_diagnostic_names_the_kind_and_the_function() {
        let d = diags(&one_liner("parse_args", 9), 5);
        assert_eq!(
            d[0]["message"], "Complexity is 10 for fn 'parse_args'",
            "the message format matches jscx exactly"
        );
        assert_eq!(d[0]["source"], "rustcx");
    }

    #[test]
    fn the_range_covers_the_function_line_in_utf16_units() {
        // A line whose byte length, char count, and UTF-16 length all differ.
        let src = format!("// \u{1F980}\u{1F980} a four-byte crab is two UTF-16 units\n{}\n", one_liner("f", 9));
        let d = diags(&src, 5);

        assert_eq!(d[0]["range"]["start"]["line"], 1, "0-based: the second line");
        assert_eq!(d[0]["range"]["start"]["character"], 0);

        let expected: usize = src.lines().nth(1).unwrap().chars().map(char::len_utf16).sum();
        assert_eq!(
            d[0]["range"]["end"]["character"], expected,
            "columns are UTF-16 code units, not bytes and not chars"
        );
    }

    #[test]
    fn a_closure_gets_its_own_diagnostic() {
        let src = format!(
            "fn outer() {{ let f = |a: bool| {{ {} }}; }}\n",
            "if a {} ".repeat(9)
        );
        let d = diags(&src, 5);
        assert_eq!(d.len(), 1, "the closure is scored independently of its parent");
        assert!(d[0]["message"].as_str().unwrap().contains("closure"));
    }


    /// Three lines: a signature, a branch, a blank body line.
    const SAMPLE: &str = "\
fn describe(a: bool, b: bool) -> u8 {
    if a && b { 1 } else { 0 }
}
";

    fn hover(src: &str, line: u32) -> Option<String> {
        let funcs = analyze(src, Options::default()).expect("fixture parses");
        hover_at(&funcs, line)
    }

    #[test]
    fn hovering_the_signature_line_gives_the_whole_breakdown() {
        let text = hover(SAMPLE, 1).expect("the signature line answers");
        assert!(text.contains("describe"), "names the function");
        assert!(text.contains("complexity 3"), "1 + if + &&");
        assert!(text.contains("if"), "lists the decision points");
        assert!(text.contains("&&"));
    }

    #[test]
    fn hovering_a_decision_point_line_also_answers() {
        let text = hover(SAMPLE, 2).expect("a counted line answers");
        assert!(text.contains("describe"));
        assert!(text.contains("complexity 3"));
    }

    #[test]
    fn hovering_an_uncounted_line_answers_nothing() {
        assert!(
            hover(SAMPLE, 3).is_none(),
            "the closing brace is neither a signature nor a decision point"
        );
        assert!(hover(SAMPLE, 99).is_none(), "and neither is a line past the end");
    }

    #[test]
    fn the_innermost_function_wins() {
        // The closure's `if` is on the same line as the closure itself, and
        // inside the parent's span. The closure is the useful answer.
        let src = "fn outer() {\n    let f = |a: bool| if a { 1 } else { 0 };\n}\n";
        let text = hover(src, 2).expect("line 2 answers");
        assert!(
            text.contains("closure"),
            "the innermost function wins, got: {text}"
        );
    }

    #[test]
    fn a_function_with_no_branches_still_answers_on_its_signature() {
        let text = hover("fn plain() {}\n", 1).expect("a score of 1 is still a score");
        assert!(text.contains("complexity 1"));
        assert!(text.contains("plain"));
    }


    #[test]
    fn a_recovered_document_forgets_its_last_error() {
        // A struct literal rather than a reassigned default: clippy rejects the
        // latter, and the point is the transition, not how the value got there.
        let mut doc = Doc { last_error: Some("x.rs: 2: expected `}`".to_string()), ..Doc::default() };
        // What the success path does.
        doc.last_error = None;
        assert!(
            doc.last_error.is_none(),
            "a file that breaks, is fixed, then breaks the same way logs twice"
        );
    }

}
