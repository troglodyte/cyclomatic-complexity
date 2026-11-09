//! One Language Server for every registered language: complexity diagnostics
//! and hover.
//!
//! Hand-rolled JSON-RPC: `serde_json::json!` literals rather than `lsp-types`
//! (and no serde derive, which would pull `syn` into every alias), a blocking
//! `Content-Length` loop rather than `tower-lsp`, and no async runtime. One
//! message is handled at a time; an analysis is milliseconds, so there is
//! nothing to overlap and no state to share.
//!
//! Each document is scored by the registered language that claims its path,
//! with that language's options. A document no language claims, or one the
//! language skips (a `.d.ts`, say), is published with no diagnostics.
//!
//! **stdout is the protocol.** Nothing in this module may print to it except
//! through `Server::send`.

use crate::config::{FileConfig, Watched};
use crate::language::{Lang, LangOptions, Registry};
use crate::model::Func;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

/// Score at which a function starts warning. Twice this value is an error.
pub const DEFAULT_THRESHOLD: u32 = 11;

/// Upper bound on a JSON-RPC body, so a bogus `Content-Length` cannot ask us to
/// allocate an arbitrary amount of memory.
const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;

/// JSON-RPC error code for a method the server does not implement.
const METHOD_NOT_FOUND: i32 = -32601;

/// LSP `DiagnosticSeverity`.
const SEVERITY_ERROR: u32 = 1;
const SEVERITY_WARNING: u32 = 2;

/// `window/logMessage` type 4 (`Log`): the client's language-server log.
const MESSAGE_TYPE_LOG: u32 = 4;

/// One open document: the last **successful** analysis of it.
///
/// The text is deliberately not kept. Full sync means every change carries the
/// whole buffer, so there is nothing to reconstruct, and the UTF-16 line widths
/// diagnostics need are computed at publish time and discarded.
#[derive(Default)]
struct Doc {
    funcs: Vec<Func>,
    /// The last parse error logged for this document, for dedupe.
    last_error: Option<String>,
}

struct Server<W: Write> {
    /// `cmplx`, `jscx` or `rustcx`: the diagnostic `source` and log prefix.
    program: &'static str,
    reg: Registry,
    docs: HashMap<String, Doc>,
    /// `initializationOptions`, which win over the file.
    init: FileConfig,
    /// The watched `cmplx.toml` (or `rustcx.toml`), re-read on mtime change.
    /// `None` until `initialize` says where to look.
    config: Option<Watched>,
    out: W,
}

impl<W: Write> Server<W> {
    fn new(program: &'static str, reg: &Registry, out: W) -> Server<W> {
        Server {
            program,
            reg: reg.clone(),
            docs: HashMap::new(),
            init: FileConfig::default(),
            config: None,
            out,
        }
    }

    /// Settings in force right now: `initializationOptions` over the file over
    /// the built-in defaults.
    fn settings(&self) -> FileConfig {
        let file = self
            .config
            .as_ref()
            .map(Watched::config)
            .unwrap_or_default();
        self.init.clone().or(file)
    }

    fn send(&mut self, msg: Value) -> io::Result<()> {
        let body = msg.to_string();
        // Content-Length is in bytes.
        write!(self.out, "Content-Length: {}\r\n\r\n{}", body.len(), body)?;
        self.out.flush()
    }

    fn respond(&mut self, id: Value, result: Value) -> io::Result<()> {
        // A JSON-RPC response must not carry a `method` member at all, so it is
        // omitted rather than serialized as null.
        self.send(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
    }

    fn respond_error(&mut self, id: Value, code: i32, message: &str) -> io::Result<()> {
        self.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message },
        }))
    }

    /// `window/logMessage` at type `Log`. Never `showMessage`: none of this
    /// warrants interrupting anyone.
    fn log(&mut self, message: &str) -> io::Result<()> {
        let message = format!("{}: {message}", self.program);
        self.send(json!({
            "jsonrpc": "2.0",
            "method": "window/logMessage",
            "params": { "type": MESSAGE_TYPE_LOG, "message": message },
        }))
    }

    /// Handle `initialize`: read our settings, discover a config file under the
    /// workspace root, and advertise what we implement. Problems with either
    /// are logged, not fatal: a bad setting must not prevent a session.
    fn initialize(&mut self, params: &Value) -> io::Result<Value> {
        let opts = params.get("initializationOptions").unwrap_or(&Value::Null);
        let (init, warnings) = FileConfig::from_init_options(opts, &self.reg, self.reg.langs());
        self.init = init;
        for warning in warnings {
            self.log(&warning)?;
        }
        let (watched, err) = Watched::discover(&root_dir(params), &self.reg);
        self.config = Some(watched);
        if let Some(e) = err {
            self.log(&format!("config: {e}"))?;
        }
        self.log_config_warnings()?;
        Ok(json!({
            "capabilities": {
                "textDocumentSync": 1, // Full sync
                "hoverProvider": true,
            }
        }))
    }

    fn publish(&mut self, uri: &str, diagnostics: Vec<Value>) -> io::Result<()> {
        self.send(json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": { "uri": uri, "diagnostics": diagnostics },
        }))
    }

    /// Pick up an edited config file before using its settings.
    fn refresh_config(&mut self) -> io::Result<()> {
        if let Some(e) = self.config.as_mut().and_then(Watched::refresh) {
            self.log(&format!("config: {e}"))?;
        }
        self.log_config_warnings()
    }

    /// Log, once per load, each config key an alias ignored. The CLI prints
    /// these on stderr; here stderr is unseen, so they go to the client's log.
    fn log_config_warnings(&mut self) -> io::Result<()> {
        let warnings = self
            .config
            .as_mut()
            .map(Watched::take_warnings)
            .unwrap_or_default();
        for warning in warnings {
            self.log(&format!("config: {warning}"))?;
        }
        Ok(())
    }

    /// The language that scores `path`, with its options, unless no language
    /// claims the path or the claiming language skips it.
    fn scorer_for(&self, path: &Path) -> Option<(Lang, LangOptions)> {
        let lang = self.reg.for_path(path, self.reg.langs())?;
        let opts = self.settings().options_for(lang);
        (!lang.skip(path, &opts)).then_some((lang, opts))
    }

    /// Analyze a document and publish its diagnostics.
    fn update(&mut self, uri: &str, text: &str) -> io::Result<()> {
        self.refresh_config()?;
        let path = document_path(uri);
        let Some((lang, opts)) = self.scorer_for(&path) else {
            // Nothing to score, but the client is still told so explicitly.
            self.docs.remove(uri);
            return self.publish(uri, Vec::new());
        };
        let funcs = match lang.analyze(text, &path, &opts) {
            Ok(funcs) => funcs,
            Err(e) => return self.parse_failed(uri, &format!("{uri}: {}: {}", e.line, e.message)),
        };
        let threshold = self.settings().threshold.unwrap_or(DEFAULT_THRESHOLD);
        let diagnostics = diagnostics_for(&funcs, text, threshold, self.program);
        let doc = self.docs.entry(uri.to_string()).or_default();
        doc.funcs = funcs;
        doc.last_error = None;
        self.publish(uri, diagnostics)
    }

    /// Publish nothing on a parse failure: a buffer is unparseable for most of
    /// the time someone is typing into it, so clearing would make the warnings
    /// flash, and publishing the syntax error would duplicate what the
    /// language's own server reports. Hover keeps answering from the functions
    /// cached by the last good parse. The failure is logged once per distinct
    /// message, so a held key does not flood the log.
    fn parse_failed(&mut self, uri: &str, message: &str) -> io::Result<()> {
        let doc = self.docs.entry(uri.to_string()).or_default();
        if doc.last_error.as_deref() == Some(message) {
            return Ok(());
        }
        doc.last_error = Some(message.to_string());
        self.log(message)
    }

    /// Handle a request — a message with an id, which a client is waiting on.
    /// Every path must reply: an unanswered request can hang a client, which
    /// is why the unknown-method arm sends an error rather than dropping it.
    fn request(&mut self, method: &str, id: Value, params: &Value) -> io::Result<()> {
        match method {
            "initialize" => {
                let result = self.initialize(params)?;
                self.respond(id, result)
            }
            "shutdown" => self.respond(id, Value::Null),
            "textDocument/hover" => {
                let result = self.hover(params);
                self.respond(id, result)
            }
            _ => self.respond_error(id, METHOD_NOT_FOUND, &format!("unknown method: {method}")),
        }
    }

    /// The hover result for a `textDocument/hover` request, `null` when there
    /// is nothing to say.
    fn hover(&self, params: &Value) -> Value {
        let uri = params["textDocument"]["uri"].as_str().unwrap_or_default();
        // LSP positions are 0-based; Func.line is 1-based. A line no u32 can
        // number has no function on it.
        let Some(line) = u32::try_from(params["position"]["line"].as_u64().unwrap_or(0))
            .ok()
            .and_then(|l| l.checked_add(1))
        else {
            return Value::Null;
        };
        match self.docs.get(uri).and_then(|d| hover_at(&d.funcs, line)) {
            Some(text) => json!({ "contents": { "kind": "markdown", "value": text } }),
            None => Value::Null,
        }
    }

    /// Handle a notification — no id, so no reply is owed. An unknown one is
    /// ignored, as the protocol requires. `exit` is handled by the loop.
    fn notification(&mut self, method: &str, params: &Value) -> io::Result<()> {
        match method {
            "textDocument/didOpen" => {
                let text = params["textDocument"]["text"].as_str();
                self.update_from(uri_of(params), text)
            }
            // Full sync: one change carrying the entire buffer.
            "textDocument/didChange" => {
                let text = params["contentChanges"][0]["text"].as_str();
                self.update_from(uri_of(params), text)
            }
            // Under full sync the buffer already arrived via didChange, and
            // `text` is only present if the client opted into sending it.
            "textDocument/didSave" => Ok(()),
            "textDocument/didClose" => {
                let Some(uri) = uri_of(params) else {
                    return Ok(());
                };
                self.docs.remove(uri);
                self.publish(uri, Vec::new())
            }
            _ => Ok(()),
        }
    }

    /// Analyze, if the notification carried both the fields we need. A missing
    /// field skips the frame rather than panicking on it.
    fn update_from(&mut self, uri: Option<&str>, text: Option<&str>) -> io::Result<()> {
        match (uri, text) {
            (Some(uri), Some(text)) => self.update(uri, text),
            _ => Ok(()),
        }
    }

    /// Dispatch one decoded message. `Ok(false)` means `exit` was received.
    fn handle(&mut self, msg: &Value) -> io::Result<bool> {
        let Some(method) = msg["method"].as_str() else {
            return Ok(true); // not a request or notification we can act on
        };
        if method == "exit" {
            return Ok(false);
        }
        let params = msg.get("params").unwrap_or(&Value::Null);
        // An id is what separates the two halves of JSON-RPC: a request is
        // owed a reply, a notification is not.
        match msg.get("id") {
            Some(id) => self.request(method, id.clone(), params)?,
            None => self.notification(method, params)?,
        }
        Ok(true)
    }
}

/// Serve the Language Server Protocol on stdio for the languages in `reg`
/// until `exit` or end of input. `program` names the server in diagnostics
/// and log lines.
pub fn serve(program: &'static str, reg: &Registry) -> io::Result<()> {
    let stdin = io::stdin();
    let mut stdin = stdin.lock();
    let mut server = Server::new(program, reg, io::stdout());
    while let Some(msg) = read_message(&mut stdin)? {
        if !server.handle(&msg)? {
            break;
        }
    }
    Ok(())
}

/// The document URI a text-document notification is about.
fn uri_of(params: &Value) -> Option<&str> {
    params["textDocument"]["uri"].as_str()
}

/// The directory config discovery starts from: `rootUri`, else the first
/// workspace folder, else the current directory.
fn root_dir(params: &Value) -> PathBuf {
    let uri = params
        .get("rootUri")
        .and_then(Value::as_str)
        .or_else(|| params["workspaceFolders"][0]["uri"].as_str());
    uri.and_then(uri_to_path)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The path a document is scored as: its file path, or for a non-`file:` URI
/// (`untitled:Untitled-1.rs`) the URI itself, so its extension still counts.
fn document_path(uri: &str) -> PathBuf {
    uri_to_path(uri).unwrap_or_else(|| PathBuf::from(uri))
}

/// `file:///a/b.rs` → `/a/b.rs`. Percent-escapes are decoded; a non-`file:`
/// URI has no path and yields `None`.
fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // Strip an authority component ("localhost" or empty) up to the next `/`.
    let path = &rest[rest.find('/')?..];
    Some(PathBuf::from(percent_decode(path)))
}

/// Decode `%XX` escapes. Anything malformed is left as written.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
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
    (len > 0 && len <= MAX_BODY_BYTES).then_some(len)
}

/// Read one framed message. `Ok(None)` at end of input; `Value::Null` for a
/// frame that cannot be used, which the caller ignores.
fn read_message(input: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut len = None;
    loop {
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
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
        return Ok(Some(Value::Null)); // header block with no length
    };
    let mut body = vec![0u8; len];
    input.read_exact(&mut body)?;
    Ok(Some(serde_json::from_slice(&body).unwrap_or(Value::Null)))
}

/// Build the diagnostics for one analyzed document: a warning at or above
/// `threshold`, an error at or above twice it, each spanning the function's
/// whole first line.
fn diagnostics_for(funcs: &[Func], text: &str, threshold: u32, source: &str) -> Vec<Value> {
    // LSP character offsets are UTF-16 code units, not bytes or chars.
    let widths: Vec<usize> = text.lines().map(|l| l.encode_utf16().count()).collect();
    funcs
        .iter()
        .filter(|f| f.complexity >= threshold)
        .map(|f| {
            // Func.line is 1-based; LSP lines are 0-based.
            let row = f.line.saturating_sub(1);
            let end = widths.get(row as usize).copied().unwrap_or(0);
            let severity = if f.complexity >= threshold.saturating_mul(2) {
                SEVERITY_ERROR
            } else {
                SEVERITY_WARNING
            };
            json!({
                "range": {
                    "start": { "line": row, "character": 0 },
                    "end": { "line": row, "character": end },
                },
                "severity": severity,
                "source": source,
                "message": format!(
                    "Complexity is {} for {} '{}'",
                    f.complexity, f.kind.label, f.name
                ),
            })
        })
        .collect()
}

/// The hover text for a 1-based source line, or `None` if that line is neither
/// a function's first line nor one of its decision points.
///
/// `Func` carries a start line and no end line, so hover answers on the lines
/// the analyzer already knows about. Conveniently, those are the branchy
/// lines: exactly the ones worth hovering.
///
/// Where two functions match, the innermost wins, so hovering inside a closure
/// reports the closure rather than its parent.
fn hover_at(funcs: &[Func], line: u32) -> Option<String> {
    let f = funcs
        .iter()
        .filter(|f| f.line == line || f.points.iter().any(|p| p.line == line))
        .max_by_key(|f| f.depth)?;

    let mut out = format!(
        "`{}` — {} complexity {}\n",
        f.name, f.kind.label, f.complexity
    );
    for p in &f.points {
        out.push_str(&format!("\n- line {} — {}", p.line, p.branch));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language::{LangOptions, Language, OptionSpec};
    use crate::model::{Kind, ParseError, Point};

    const SKIP_ALL: OptionSpec = OptionSpec {
        key: "skip-all",
        on_flag: None,
        off_flag: None,
        default: Some(false),
        help: "",
    };
    static OPTIONS: [OptionSpec; 1] = [SKIP_ALL];

    /// Each source line `name score` is one function on that line; a line `!`
    /// is a parse error. `skip-all` makes the language skip every path.
    struct Fake;
    impl Language for Fake {
        fn id(&self) -> &'static str {
            "fake"
        }
        fn display_name(&self) -> &'static str {
            "Fake"
        }
        fn alias(&self) -> Option<&'static str> {
            Some("fakecx")
        }
        fn extensions(&self) -> &'static [&'static str] {
            &["fk"]
        }
        fn stdin_ext(&self) -> &'static str {
            "fk"
        }
        fn options(&self) -> &'static [OptionSpec] {
            &OPTIONS
        }
        fn skip(&self, _: &Path, opts: &LangOptions) -> bool {
            opts.flag(&SKIP_ALL).unwrap_or(false)
        }
        fn analyze(&self, src: &str, _: &Path, _: &LangOptions) -> Result<Vec<Func>, ParseError> {
            src.lines()
                .enumerate()
                .filter(|(_, line)| !line.trim().is_empty())
                .map(|(i, line)| {
                    let line_no = i as u32 + 1;
                    let (name, cc) = line.split_once(' ').ok_or(ParseError {
                        line: line_no,
                        message: "expected `name score`".into(),
                    })?;
                    Ok(func(name, line_no, cc.trim().parse().unwrap_or(1)))
                })
                .collect()
        }
    }
    static FAKE: Fake = Fake;

    const FN: Kind = Kind {
        id: "trait_fn",
        label: "trait fn",
    };

    fn func(name: &str, line: u32, complexity: u32) -> Func {
        Func {
            name: name.to_string(),
            kind: FN,
            line,
            complexity,
            depth: 0,
            points: Vec::new(),
        }
    }

    fn server() -> Server<Vec<u8>> {
        Server::new("cmplx", &Registry::all(&[&FAKE]), Vec::new())
    }

    /// Decode every frame the server wrote, then forget them.
    fn sent(server: &mut Server<Vec<u8>>) -> Vec<Value> {
        let bytes = std::mem::take(&mut server.out);
        let mut input = io::Cursor::new(bytes);
        std::iter::from_fn(|| read_message(&mut input).unwrap()).collect()
    }

    fn published(server: &mut Server<Vec<u8>>) -> Vec<Value> {
        sent(server)
            .into_iter()
            .filter(|m| m["method"] == "textDocument/publishDiagnostics")
            .map(|m| m["params"]["diagnostics"].clone())
            .collect()
    }

    #[test]
    fn content_length_is_read_case_insensitively() {
        assert_eq!(parse_content_length("Content-Length: 42\r\n"), Some(42));
        assert_eq!(parse_content_length("content-length:42\r\n"), Some(42));
        assert_eq!(parse_content_length("CONTENT-LENGTH:   42  \r\n"), Some(42));
    }

    #[test]
    fn an_unusable_content_length_is_rejected() {
        assert_eq!(
            parse_content_length("Content-Type: application/json\r\n"),
            None
        );
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
        assert_eq!(
            uri_to_path("file:///a/b.rs"),
            Some(PathBuf::from("/a/b.rs"))
        );
        assert_eq!(
            uri_to_path("file:///a/my%20crate/b.rs"),
            Some(PathBuf::from("/a/my crate/b.rs")),
            "percent escapes are decoded"
        );
        assert_eq!(
            uri_to_path("file:///a/100%"),
            Some(PathBuf::from("/a/100%"))
        );
        assert_eq!(uri_to_path("untitled:Untitled-1"), None);
    }

    #[test]
    fn a_non_file_uri_is_scored_by_its_extension() {
        assert_eq!(
            document_path("untitled:Untitled-1.fk"),
            PathBuf::from("untitled:Untitled-1.fk")
        );
    }

    fn diags(src: &str, threshold: u32) -> Vec<Value> {
        let funcs = FAKE
            .analyze(src, Path::new("a.fk"), &LangOptions::default())
            .unwrap();
        diagnostics_for(&funcs, src, threshold, "cmplx")
    }

    #[test]
    fn only_functions_at_or_above_the_threshold_are_reported() {
        let d = diags("small 2\nbig 10\n", 5);
        assert_eq!(d.len(), 1, "complexity 2 is below the threshold, 10 is not");
        assert!(d[0]["message"].as_str().unwrap().contains("big"));
    }

    #[test]
    fn the_severity_boundaries_are_exact() {
        assert_eq!(diags("f 5", 5)[0]["severity"], 2, "5 at threshold 5 warns");
        assert_eq!(
            diags("f 10", 5)[0]["severity"],
            1,
            "10 at threshold 5 errors"
        );
        assert_eq!(diags("f 9", 5)[0]["severity"], 2, "9 at threshold 5 warns");
        assert!(diags("f 4", 5).is_empty(), "4 at threshold 5 is quiet");
    }

    #[test]
    fn a_diagnostic_names_the_kind_label_the_function_and_the_source() {
        let d = diags("parse_args 10", 5);
        assert_eq!(
            d[0]["message"],
            "Complexity is 10 for trait fn 'parse_args'"
        );
        assert_eq!(d[0]["source"], "cmplx");
    }

    #[test]
    fn the_range_covers_the_function_line_in_utf16_units() {
        // A name whose byte length, char count, and UTF-16 length all differ.
        let src = "x 1\n\u{1F980}\u{1F980}é 12\n";
        let d = diags(src, 5);
        assert_eq!(d[0]["range"]["start"], json!({ "line": 1, "character": 0 }));
        assert_eq!(
            d[0]["range"]["end"],
            json!({ "line": 1, "character": 8 }),
            "two crabs (2 units each) + é (1) + ` 12` (3)"
        );
    }

    /// A signature on line 1 and two branches on line 2.
    fn describe() -> Func {
        Func {
            points: vec![
                Point {
                    line: 2,
                    branch: "if",
                },
                Point {
                    line: 2,
                    branch: "&&",
                },
            ],
            ..func("describe", 1, 3)
        }
    }

    #[test]
    fn hovering_the_first_line_gives_the_whole_breakdown() {
        let text = hover_at(&[describe()], 1).expect("the first line answers");
        assert_eq!(
            text,
            "`describe` — trait fn complexity 3\n\n- line 2 — if\n- line 2 — &&"
        );
    }

    #[test]
    fn hovering_a_decision_point_line_also_answers() {
        let text = hover_at(&[describe()], 2).expect("a counted line answers");
        assert!(text.contains("complexity 3"));
    }

    #[test]
    fn hovering_an_uncounted_line_answers_nothing() {
        assert!(hover_at(&[describe()], 3).is_none());
        assert!(hover_at(&[describe()], 99).is_none());
    }

    #[test]
    fn the_innermost_function_wins() {
        let closure = Func {
            depth: 1,
            ..func("<closure>", 2, 2)
        };
        let text = hover_at(&[describe(), closure], 2).expect("line 2 answers");
        assert!(text.contains("<closure>"), "got: {text}");
    }

    fn open(server: &mut Server<Vec<u8>>, uri: &str, text: &str) {
        let params = json!({ "textDocument": { "uri": uri, "text": text } });
        server
            .notification("textDocument/didOpen", &params)
            .unwrap();
    }

    fn initialize(server: &mut Server<Vec<u8>>, options: Value) {
        let params = json!({ "rootUri": null, "initializationOptions": options });
        server.request("initialize", json!(1), &params).unwrap();
        sent(server);
    }

    fn hover_line(server: &Server<Vec<u8>>, line: u64) -> Value {
        server.hover(&json!({
            "textDocument": { "uri": "file:///x/a.fk" },
            "position": { "line": line, "character": 0 },
        }))
    }

    #[test]
    fn hovering_a_line_beyond_any_function_line_number_answers_null() {
        let mut s = server();
        initialize(&mut s, Value::Null);
        open(&mut s, "file:///x/a.fk", "f 3\n");
        assert_ne!(hover_line(&s, 0), Value::Null, "line 0 is f");
        assert_eq!(hover_line(&s, u64::from(u32::MAX)), Value::Null);
        assert_eq!(
            hover_line(&s, u64::from(u32::MAX) + 1),
            Value::Null,
            "must not wrap round to line 0"
        );
    }

    fn logs(server: &mut Server<Vec<u8>>) -> Vec<String> {
        sent(server)
            .iter()
            .filter(|m| m["method"] == "window/logMessage")
            .map(|m| m["params"]["message"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn an_alias_logs_each_config_key_it_ignores_at_start_and_on_reload() {
        let dir = std::env::temp_dir().join(format!("cmplx-lsp-ignored-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("cmplx.toml");
        std::fs::write(&config, "bogus = true\n").unwrap();
        let mut s = Server::new("fakecx", &Registry::alias(&FAKE), Vec::new());
        let params = json!({ "rootUri": format!("file://{}", dir.display()) });
        s.request("initialize", json!(1), &params).unwrap();
        assert_eq!(
            logs(&mut s),
            [format!("fakecx: config: {}: ignoring unknown key `bogus`", config.display())]
        );

        std::fs::write(&config, "[other]\nx = 1\n").unwrap();
        let later = std::fs::metadata(&config).unwrap().modified().unwrap()
            + std::time::Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(&config)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(later))
            .unwrap();
        open(&mut s, "file:///x/a.fk", "f 1");
        assert_eq!(
            logs(&mut s),
            [format!("fakecx: config: {}: ignoring unknown table `[other]`", config.display())]
        );
        open(&mut s, "file:///x/a.fk", "f 1");
        assert!(logs(&mut s).is_empty(), "an unchanged file is not re-reported");
    }

    #[test]
    fn an_unclaimed_document_publishes_an_empty_list() {
        let mut s = server();
        initialize(&mut s, Value::Null);
        open(&mut s, "file:///x/README.md", "f 50");
        assert_eq!(published(&mut s), vec![json!([])]);
        assert!(s.docs.is_empty(), "nothing is cached for hover");
    }

    #[test]
    fn a_skipped_document_publishes_an_empty_list() {
        let mut s = server();
        initialize(&mut s, json!({ "fake": { "skip-all": true } }));
        open(&mut s, "file:///x/a.fk", "f 50");
        assert_eq!(published(&mut s), vec![json!([])]);
    }

    #[test]
    fn a_claimed_document_is_scored_at_the_default_threshold() {
        let mut s = server();
        initialize(&mut s, Value::Null);
        open(&mut s, "file:///x/a.fk", "low 10\nhigh 11\n");
        let lists = published(&mut s);
        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0].as_array().unwrap().len(), 1, "{}", lists[0]);
    }

    #[test]
    fn initialization_options_win_over_the_default_threshold() {
        let mut s = server();
        initialize(&mut s, json!({ "threshold": 3 }));
        assert_eq!(s.settings().threshold, Some(3));
        open(&mut s, "file:///x/a.fk", "f 3");
        assert_eq!(published(&mut s)[0].as_array().unwrap().len(), 1);
    }

    #[test]
    fn an_unknown_initialization_option_is_logged_with_the_program_name() {
        let mut s = server();
        let params = json!({ "initializationOptions": { "treshold": 3 } });
        s.request("initialize", json!(1), &params).unwrap();
        let logs: Vec<String> = sent(&mut s)
            .iter()
            .filter(|m| m["method"] == "window/logMessage")
            .map(|m| m["params"]["message"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(logs.len(), 1, "{logs:?}");
        assert!(logs[0].starts_with("cmplx: "), "{}", logs[0]);
        assert!(logs[0].contains("treshold"), "{}", logs[0]);
    }

    #[test]
    fn a_parse_failure_publishes_nothing_and_logs_once() {
        let mut s = server();
        initialize(&mut s, Value::Null);
        open(&mut s, "file:///x/a.fk", "f 20");
        sent(&mut s);

        open(&mut s, "file:///x/a.fk", "broken");
        let first = sent(&mut s);
        assert_eq!(first.len(), 1, "one log line, no publish: {first:?}");
        assert_eq!(first[0]["method"], "window/logMessage");
        assert_eq!(
            first[0]["params"]["message"],
            "cmplx: file:///x/a.fk: 1: expected `name score`"
        );

        open(&mut s, "file:///x/a.fk", "broken");
        assert!(
            sent(&mut s).is_empty(),
            "an identical error is not logged again"
        );
        assert_eq!(
            s.docs["file:///x/a.fk"].funcs.len(),
            1,
            "hover keeps the last good parse"
        );
    }

    #[test]
    fn a_recovered_document_forgets_its_last_error() {
        let mut s = server();
        initialize(&mut s, Value::Null);
        open(&mut s, "file:///x/a.fk", "broken");
        open(&mut s, "file:///x/a.fk", "f 1");
        open(&mut s, "file:///x/a.fk", "broken");
        let logs = sent(&mut s)
            .into_iter()
            .filter(|m| m["method"] == "window/logMessage")
            .count();
        assert_eq!(
            logs, 2,
            "a file that breaks, is fixed, then breaks the same way logs twice"
        );
    }

    #[test]
    fn exit_stops_the_loop_and_a_frame_without_a_method_is_ignored() {
        let mut s = server();
        assert!(s.handle(&Value::Null).unwrap());
        assert!(!s
            .handle(&json!({ "jsonrpc": "2.0", "method": "exit" }))
            .unwrap());
    }
}
