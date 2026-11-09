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
//! with that language's options, in one worker process kept for the session
//! (see [`crate::supervisor`]). An analyzer crash becomes one warning on that
//! document; the worker is restarted on the next document, at most
//! [`crate::supervisor::MAX_WORKER_RESTARTS`] times a minute. A document no language claims, or one the
//! language skips (a `.d.ts`, say), is published with no diagnostics.
//!
//! **stdout is the protocol.** Nothing in this module may print to it except
//! through `Server::send`.

use crate::config::{FileConfig, Watched};
use crate::language::{Lang, LangOptions, Registry};
use crate::model::{CrashReason, Func, ParseError, ParseErrorKind};
use crate::report::shown_complexity;
use crate::run::{size_label, MAX_FILE_BYTES};
use crate::supervisor::{Supervisor, SupervisorConfig};
use serde_json::{json, Value};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Score at which a function starts warning. Twice this value is an error.
pub const DEFAULT_THRESHOLD: u32 = 11;

/// Upper bound on a JSON-RPC body, so a bogus `Content-Length` cannot ask us to
/// allocate an arbitrary amount of memory.
const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;

/// Longest header line read. LSP headers are a few dozen bytes; a longer line
/// is consumed up to its newline and its frame skipped.
const MAX_HEADER_LINE_BYTES: usize = 8 * 1024;

/// Most bytes consumed looking for the newline that ends an over-long header
/// line. Past this the stream has no recognisable framing and the session ends.
const MAX_HEADER_DISCARD_BYTES: usize = MAX_BODY_BYTES;

/// Most header lines in one frame (LSP defines two). Past this the frame is
/// skipped and reading resumes with the next line.
const MAX_HEADER_LINES: usize = 64;

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
    /// Largest document scored; [`MAX_FILE_BYTES`] outside tests.
    max_bytes: u64,
    /// Scores every document, in one worker for the whole session.
    supervisor: Supervisor,
    /// Per URI, the text whose analysis crashed and how, so that the same
    /// text is answered from here rather than crashing another worker.
    crashed: HashMap<String, Crash>,
    out: W,
}

/// A document text that crashed the analyzer.
struct Crash {
    /// [`fingerprint`] of the text, its language and its options.
    fingerprint: u64,
    error: ParseError,
    /// When it crashed.
    at: Instant,
}

/// How long a remembered timeout stands. Unlike a crash, a timeout can be a
/// busy machine rather than the text, so it is tried again after this.
const TIMEOUT_RETRY_AFTER: Duration = Duration::from_secs(60);

impl<W: Write> Server<W> {
    /// A server that scores in process: for tests of everything around scoring.
    #[cfg(test)]
    fn new(program: &'static str, reg: &Registry, out: W) -> Server<W> {
        Server::with_supervisor(program, reg, out, Supervisor::in_process())
    }

    fn with_supervisor(
        program: &'static str,
        reg: &Registry,
        out: W,
        supervisor: Supervisor,
    ) -> Server<W> {
        Server {
            program,
            reg: reg.clone(),
            docs: HashMap::new(),
            init: FileConfig::default(),
            config: None,
            max_bytes: MAX_FILE_BYTES,
            supervisor,
            crashed: HashMap::new(),
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
        if text.len() as u64 > self.max_bytes {
            return self.too_large(uri);
        }
        let fingerprint = fingerprint(lang, &opts, text);
        let remembered = self
            .crashed
            .get(uri)
            .filter(|c| c.fingerprint == fingerprint && !c.stale());
        if let Some(crash) = remembered {
            let message = crash.error.message.clone();
            return self.publish_crash(uri, text, &message);
        }
        let result = self.supervisor.analyze(lang, text, &path, &opts);
        for warning in self.supervisor.take_warnings() {
            self.log(&warning)?;
        }
        if !matches!(
            &result,
            Err(ParseError {
                kind: ParseErrorKind::Crashed { .. },
                ..
            })
        ) {
            self.crashed.remove(uri);
        }
        let funcs = match result {
            Ok(funcs) => funcs,
            Err(
                e @ ParseError {
                    kind: ParseErrorKind::TooDeep { .. },
                    ..
                },
            ) => return self.too_deep(uri, text, &e),
            Err(
                e @ ParseError {
                    kind: ParseErrorKind::Crashed { .. },
                    ..
                },
            ) => return self.crashed(uri, text, fingerprint, e),
            Err(
                e @ ParseError {
                    kind: ParseErrorKind::Refused,
                    ..
                },
            ) => return self.unscored(uri, format!("{uri}: {}", e.message)),
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

    /// Publish one warning saying why a too-deeply-nested document has no
    /// scores. Unlike a syntax error this lasts until the file is edited, so
    /// keeping the last good diagnostics would leave them stale indefinitely
    /// with no hint why; and no other server reports it.
    fn too_deep(&mut self, uri: &str, text: &str, error: &ParseError) -> io::Result<()> {
        let doc = self.docs.entry(uri.to_string()).or_default();
        doc.funcs.clear();
        doc.last_error = None;
        let row = error.line.saturating_sub(1);
        let diagnostic = json!({
            "range": line_range(text, row),
            "severity": SEVERITY_WARNING,
            "source": self.program,
            "message": error.message,
        });
        self.publish(uri, vec![diagnostic])
    }

    /// Log a crash and publish it as one warning on line 0. Every crash but
    /// a paused or failed restart is remembered against this exact text, so that
    /// re-sending it (a save, an undo) does not crash another worker.
    fn crashed(
        &mut self,
        uri: &str,
        text: &str,
        fingerprint: u64,
        error: ParseError,
    ) -> io::Result<()> {
        self.log(&format!("{uri}: {}", error.message))?;
        let message = error.message.clone();
        // A paused or failed restart says nothing about this text.
        let about_the_text = !matches!(
            error.kind,
            ParseErrorKind::Crashed {
                reason: CrashReason::GaveUp | CrashReason::SpawnFailed { .. }
            }
        );
        if about_the_text {
            self.crashed.insert(
                uri.to_string(),
                Crash {
                    fingerprint,
                    error,
                    at: Instant::now(),
                },
            );
        }
        self.publish_crash(uri, text, &message)
    }

    /// Publish the one warning for a document the analyzer crashed on, and
    /// forget its scores: like a too-deep document, it stays unscored until
    /// edited, so the last good diagnostics would only mislead.
    fn publish_crash(&mut self, uri: &str, text: &str, message: &str) -> io::Result<()> {
        let doc = self.docs.entry(uri.to_string()).or_default();
        doc.funcs.clear();
        doc.last_error = None;
        let diagnostic = json!({
            "range": line_range(text, 0),
            "severity": SEVERITY_WARNING,
            "source": self.program,
            "message": message,
        });
        self.publish(uri, vec![diagnostic])
    }

    /// Publish no diagnostics for a document over the size cap, as the CLI
    /// scores no file over it, and forget its scores so hover does not answer
    /// from a stale version. Logged once until the message changes.
    fn too_large(&mut self, uri: &str) -> io::Result<()> {
        let message = format!(
            "{uri}: larger than {}; not scored",
            size_label(self.max_bytes)
        );
        self.unscored(uri, message)
    }

    /// Publish no diagnostics for a document that will not be scored until
    /// it changes (too large, or refused by the worker), forget its scores,
    /// and log `message` once until it changes.
    fn unscored(&mut self, uri: &str, message: String) -> io::Result<()> {
        let doc = self.docs.entry(uri.to_string()).or_default();
        doc.funcs.clear();
        if doc.last_error.as_deref() != Some(message.as_str()) {
            doc.last_error = Some(message.clone());
            self.log(&message)?;
        }
        self.publish(uri, Vec::new())
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
                self.crashed.remove(uri);
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
    let supervisor = Supervisor::new(SupervisorConfig::lsp());
    let mut server = Server::with_supervisor(program, reg, io::stdout(), supervisor);
    serve_frames(&mut stdin, &mut server)
}

/// Read frames from `input` and hand each to `server` until `exit` or end of
/// input, logging every frame that had to be skipped.
fn serve_frames<W: Write>(input: &mut impl BufRead, server: &mut Server<W>) -> io::Result<()> {
    let mut notes = Vec::new();
    loop {
        let frame = read_message(input, &mut notes)?;
        for note in notes.drain(..) {
            server.log(&note)?;
        }
        match frame {
            Frame::Message(msg) => {
                if !server.handle(&msg)? {
                    return Ok(());
                }
            }
            Frame::Skipped => {}
            Frame::End => return Ok(()),
        }
    }
}

/// Identifies a document text and the language and options it was scored
/// with, so new settings try it again. Collisions are negligible at 64 bits;
/// one would only re-show a crash warning.
fn fingerprint(lang: Lang, opts: &LangOptions, text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    lang.id().hash(&mut hasher);
    opts.iter().for_each(|option| option.hash(&mut hasher));
    text.hash(&mut hasher);
    hasher.finish()
}

impl Crash {
    /// A timeout past [`TIMEOUT_RETRY_AFTER`], to be tried again.
    fn stale(&self) -> bool {
        let timed_out = matches!(
            self.error.kind,
            ParseErrorKind::Crashed {
                reason: CrashReason::Timeout
            }
        );
        timed_out && self.at.elapsed() >= TIMEOUT_RETRY_AFTER
    }
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

/// Parse a `Content-Length` header value: a positive byte count, of any size.
/// [`read_message`] decides what to do with one past [`MAX_BODY_BYTES`].
fn parse_content_length(line: &str) -> Option<usize> {
    let (name, value) = line.split_once(':')?;
    if !name.trim().eq_ignore_ascii_case("content-length") {
        return None;
    }
    let len: usize = value.trim().parse().ok()?;
    (len > 0).then_some(len)
}

/// One read from the client.
enum Frame {
    /// A decoded body; `Value::Null` when it is not JSON, which the caller ignores.
    Message(Value),
    /// A frame that could not be used and was consumed whole.
    Skipped,
    /// End of input: the client is gone, and the session ends cleanly.
    End,
}

/// One header line read by [`read_header_line`].
enum HeaderLine {
    /// The line's bytes, newline included.
    Line(Vec<u8>),
    /// A line past [`MAX_HEADER_LINE_BYTES`], consumed through its newline.
    TooLong,
    /// End of input, or an over-long line with no newline in reach.
    End,
}

/// Read one header line of at most [`MAX_HEADER_LINE_BYTES`]. A longer line
/// is consumed through its newline, at most [`MAX_HEADER_DISCARD_BYTES`]
/// further; past that, or at end of input, the stream is over.
fn read_header_line(input: &mut impl BufRead, notes: &mut Vec<String>) -> io::Result<HeaderLine> {
    let mut line = Vec::new();
    let cap = MAX_HEADER_LINE_BYTES as u64 + 1;
    let n = Read::take(&mut *input, cap).read_until(b'\n', &mut line)?;
    if line.ends_with(b"\n") {
        return Ok(HeaderLine::Line(line));
    }
    // No newline: either the input ended (a partial line is no frame) or the
    // line is over-long.
    if n <= MAX_HEADER_LINE_BYTES {
        return Ok(HeaderLine::End);
    }
    if skip_through_newline(input, MAX_HEADER_DISCARD_BYTES)? {
        notes.push(format!(
            "skipped a frame: a header line longer than {MAX_HEADER_LINE_BYTES} bytes"
        ));
        return Ok(HeaderLine::TooLong);
    }
    notes.push(format!(
        "a header line ran past {MAX_HEADER_DISCARD_BYTES} bytes without a newline; closing the session"
    ));
    Ok(HeaderLine::End)
}

/// Consume input through the next `\n`, at most `limit` bytes, without
/// buffering it. `false` if the input ended or the limit came first. Each pass
/// consumes at least one byte or returns, so it makes at most `limit` passes.
fn skip_through_newline(input: &mut impl BufRead, limit: usize) -> io::Result<bool> {
    let mut consumed = 0;
    while consumed < limit {
        let buf = input.fill_buf()?;
        if buf.is_empty() {
            return Ok(false);
        }
        let window = &buf[..buf.len().min(limit - consumed)];
        if let Some(at) = window.iter().position(|&b| b == b'\n') {
            input.consume(at + 1);
            return Ok(true);
        }
        let taken = window.len();
        input.consume(taken);
        consumed += taken;
    }
    Ok(false)
}

/// Read one framed message. A header line that is not UTF-8 is skipped; a
/// frame with an over-long header line or a body past [`MAX_BODY_BYTES`] is
/// consumed whole and skipped. Each such event pushes one line onto `notes`
/// for the server log. End of input anywhere, including part-way through a
/// body, is [`Frame::End`].
fn read_message(input: &mut impl BufRead, notes: &mut Vec<String>) -> io::Result<Frame> {
    let mut len = None;
    let mut bad = false;
    for _ in 0..MAX_HEADER_LINES {
        let line = match read_header_line(input, notes)? {
            HeaderLine::Line(line) => line,
            HeaderLine::TooLong => {
                bad = true;
                continue;
            }
            HeaderLine::End => return Ok(Frame::End),
        };
        if line == b"\r\n" || line == b"\n" {
            return read_body(input, len, bad, notes);
        }
        let Ok(line) = std::str::from_utf8(&line) else {
            notes.push("skipped a header line that is not valid UTF-8".to_string());
            continue;
        };
        // Anything that is not a Content-Length is skipped rather than fatal:
        // a malformed frame must not take the server down.
        if let Some(n) = parse_content_length(line) {
            len = Some(n);
        }
    }
    notes.push(format!(
        "skipped a frame: more than {MAX_HEADER_LINES} header lines"
    ));
    Ok(Frame::Skipped)
}

/// Read the body after a header block. Without a length nothing can be read
/// and the frame is skipped; a `bad` frame or one past [`MAX_BODY_BYTES`] has
/// exactly `len` bytes discarded, so the next frame starts where it should.
fn read_body(
    input: &mut impl BufRead,
    len: Option<usize>,
    bad: bool,
    notes: &mut Vec<String>,
) -> io::Result<Frame> {
    let Some(len) = len else {
        return Ok(Frame::Skipped); // header block with no usable length
    };
    if bad || len > MAX_BODY_BYTES {
        if len > MAX_BODY_BYTES {
            notes.push(format!(
                "skipped a frame of {len} bytes: larger than {MAX_BODY_BYTES} bytes"
            ));
        }
        let wanted = len as u64;
        let discarded = io::copy(&mut Read::take(&mut *input, wanted), &mut io::sink())?;
        return Ok(if discarded < wanted {
            Frame::End
        } else {
            Frame::Skipped
        });
    }
    let mut body = vec![0u8; len];
    match input.read_exact(&mut body) {
        Ok(()) => Ok(Frame::Message(
            serde_json::from_slice(&body).unwrap_or(Value::Null),
        )),
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(Frame::End),
        Err(e) => Err(e),
    }
}

/// The LSP range covering 0-based line `row` of `text`, in UTF-16 units.
fn line_range(text: &str, row: u32) -> Value {
    let end = text
        .lines()
        .nth(row as usize)
        .map_or(0, |line| line.encode_utf16().count());
    json!({
        "start": { "line": row, "character": 0 },
        "end": { "line": row, "character": end },
    })
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
                    shown_complexity(f.complexity),
                    f.kind.label,
                    f.name
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
        f.name,
        f.kind.label,
        shown_complexity(f.complexity)
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
    use crate::model::{Kind, ParseError, ParseErrorKind, Point};
    use crate::supervisor::{RestartLimit, SupervisorConfig};

    const SKIP_ALL: OptionSpec = OptionSpec {
        key: "skip-all",
        on_flag: None,
        off_flag: None,
        default: Some(false),
        help: "",
    };
    static OPTIONS: [OptionSpec; 1] = [SKIP_ALL];

    /// Each source line `name score` is one function on that line; a line
    /// of more `(` than the nesting cap is too deep, and any other line
    /// without a space a syntax error. `skip-all` makes the language skip every path.
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
        /// A line of `(` is that many levels deep, on its own.
        fn first_line_deeper_than(
            &self,
            src: &str,
            _: &Path,
            _: &LangOptions,
            limit: usize,
        ) -> Option<u32> {
            src.lines()
                .position(|line| line.chars().filter(|&c| c == '(').count() > limit)
                .map(|i| i as u32 + 1)
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
                        kind: ParseErrorKind::Syntax,
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
        let mut notes = Vec::new();
        let mut messages = Vec::new();
        while let Frame::Message(msg) = read_message(&mut input, &mut notes).unwrap() {
            messages.push(msg);
        }
        assert!(notes.is_empty(), "the server wrote a bad frame: {notes:?}");
        messages
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
    }

    /// Every frame `read_message` makes of `bytes`, by kind, and the notes.
    fn frames(bytes: &[u8]) -> (Vec<String>, Vec<String>) {
        let mut input = io::Cursor::new(bytes.to_vec());
        let mut notes = Vec::new();
        let mut kinds = Vec::new();
        for _ in 0..16 {
            match read_message(&mut input, &mut notes).unwrap() {
                Frame::Message(msg) => kinds.push(msg["id"].to_string()),
                Frame::Skipped => kinds.push("skipped".into()),
                Frame::End => break,
            }
        }
        (kinds, notes)
    }

    const SHUTDOWN_7: &str = r#"{"id":7,"method":"shutdown"}"#;

    fn frame(body: &str) -> String {
        format!("Content-Length: {}\r\n\r\n{body}", body.len())
    }

    #[test]
    fn an_oversized_body_is_discarded_exactly_without_being_allocated() {
        let len = MAX_BODY_BYTES + 1;
        let mut bytes = format!("Content-Length: {len}\r\n\r\n").into_bytes();
        bytes.resize(bytes.len() + len, b'\n');
        bytes.extend_from_slice(frame(SHUTDOWN_7).as_bytes());
        let (kinds, notes) = frames(&bytes);
        assert_eq!(kinds, ["skipped", "7"]);
        assert_eq!(notes.len(), 1, "logged once: {notes:?}");
    }

    #[test]
    fn a_body_cut_short_by_end_of_input_ends_cleanly() {
        let oversized = format!("Content-Length: {}\r\n\r\n{{}}", MAX_BODY_BYTES + 1);
        assert_eq!(frames(oversized.as_bytes()).0, Vec::<String>::new());
        assert_eq!(
            frames(b"Content-Length: 10\r\n\r\n{}").0,
            Vec::<String>::new()
        );
    }

    #[test]
    fn an_over_long_header_line_skips_its_frame_body_and_all() {
        let junk = "a".repeat(MAX_HEADER_LINE_BYTES * 3);
        let bytes = format!("X: {junk}\r\n{}{}", frame(SHUTDOWN_7), frame(SHUTDOWN_7));
        let (kinds, notes) = frames(bytes.as_bytes());
        assert_eq!(kinds, ["skipped", "7"]);
        assert_eq!(notes.len(), 1, "{notes:?}");
    }

    #[test]
    fn a_header_line_exactly_at_the_cap_is_read() {
        let pad = "a".repeat(MAX_HEADER_LINE_BYTES - "X: \r\n".len());
        let bytes = format!("X: {pad}\r\n{}", frame(SHUTDOWN_7));
        assert_eq!(frames(bytes.as_bytes()), (vec!["7".to_string()], vec![]));
    }

    #[test]
    fn a_header_line_with_no_newline_in_reach_ends_the_session() {
        let bytes = "a".repeat(MAX_HEADER_LINE_BYTES + MAX_HEADER_DISCARD_BYTES + 2);
        let (kinds, notes) = frames(bytes.as_bytes());
        assert!(kinds.is_empty());
        assert_eq!(notes.len(), 1, "{notes:?}");
    }

    #[test]
    fn a_non_utf8_header_line_is_skipped_and_the_frame_is_read() {
        let mut bytes = b"X: \xff\r\n".to_vec();
        bytes.extend_from_slice(frame(SHUTDOWN_7).as_bytes());
        let (kinds, notes) = frames(&bytes);
        assert_eq!(kinds, ["7"]);
        assert_eq!(notes.len(), 1, "{notes:?}");
    }

    #[test]
    fn too_many_header_lines_skip_the_frame() {
        let bytes = "X: y\r\n".repeat(MAX_HEADER_LINES + 1);
        let (kinds, notes) = frames(bytes.as_bytes());
        assert_eq!(kinds, ["skipped"]);
        assert_eq!(notes.len(), 1, "{notes:?}");
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
    fn a_diagnostic_shows_a_score_above_the_cap_as_100_plus() {
        assert_eq!(
            diags("f 100", 5)[0]["message"],
            "Complexity is 100 for trait fn 'f'"
        );
        assert_eq!(
            diags("f 101", 5)[0]["message"],
            "Complexity is 100+ for trait fn 'f'"
        );
    }

    #[test]
    fn the_hover_header_shows_a_score_above_the_cap_as_100_plus() {
        let at = hover_at(&[func("f", 1, 100)], 1).unwrap();
        assert!(at.starts_with("`f` — trait fn complexity 100\n"), "{at}");
        let past = hover_at(&[func("f", 1, 101)], 1).unwrap();
        assert!(
            past.starts_with("`f` — trait fn complexity 100+\n"),
            "{past}"
        );
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
            [format!(
                "fakecx: config: {}: ignoring unknown key `bogus`",
                config.display()
            )]
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
            [format!(
                "fakecx: config: {}: ignoring unknown table `[other]`",
                config.display()
            )]
        );
        open(&mut s, "file:///x/a.fk", "f 1");
        assert!(
            logs(&mut s).is_empty(),
            "an unchanged file is not re-reported"
        );
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
    fn a_too_deep_document_publishes_one_warning_at_the_reported_line() {
        let mut s = server();
        initialize(&mut s, Value::Null);
        open(&mut s, "file:///x/a.fk", "f 20\n");
        sent(&mut s);

        open(
            &mut s,
            "file:///x/a.fk",
            &format!("f 20\n{}\n", "(".repeat(101)),
        );
        let lists = published(&mut s);
        assert_eq!(
            lists,
            vec![json!([{
                "range": {
                    "start": { "line": 1, "character": 0 },
                    "end": { "line": 1, "character": 101 },
                },
                "severity": 2,
                "source": "cmplx",
                "message": "nesting deeper than 100 levels; this code needs restructuring",
            }])]
        );
        assert_eq!(
            hover_line(&s, 0),
            Value::Null,
            "the last scores are dropped"
        );

        open(&mut s, "file:///x/a.fk", "f 20\n");
        assert_eq!(
            published(&mut s)[0].as_array().unwrap().len(),
            1,
            "fixed, scored again"
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
    fn a_document_over_the_size_cap_has_no_diagnostics_and_is_logged_once() {
        let mut s = server();
        initialize(&mut s, Value::Null);
        s.max_bytes = 8;
        open(&mut s, "file:///x/a.fk", "f 20");
        sent(&mut s);

        open(&mut s, "file:///x/a.fk", "big 20\nf 1\n");
        let first = sent(&mut s);
        let logs: Vec<&Value> = first
            .iter()
            .filter(|m| m["method"] == "window/logMessage")
            .collect();
        assert_eq!(logs.len(), 1, "{first:?}");
        assert_eq!(
            logs[0]["params"]["message"],
            "cmplx: file:///x/a.fk: larger than 8 bytes; not scored"
        );
        let lists: Vec<&Value> = first
            .iter()
            .filter(|m| m["method"] == "textDocument/publishDiagnostics")
            .collect();
        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0]["params"]["diagnostics"], json!([]));
        assert_eq!(hover_line(&s, 0), Value::Null, "no stale scores either");

        open(&mut s, "file:///x/a.fk", "big 20\nf 2\n");
        assert_eq!(logs_only(&mut s), 0, "logged once");

        open(&mut s, "file:///x/a.fk", "f 20");
        assert_eq!(
            published(&mut s)[0].as_array().unwrap().len(),
            1,
            "scored again"
        );
    }

    #[test]
    fn the_server_caps_documents_at_the_cli_file_cap() {
        assert_eq!(server().max_bytes, crate::run::MAX_FILE_BYTES);
    }

    fn logs_only(server: &mut Server<Vec<u8>>) -> usize {
        sent(server)
            .iter()
            .filter(|m| m["method"] == "window/logMessage")
            .count()
    }

    /// A server whose worker aborts on a request holding `CRASH` and
    /// otherwise answers with one function `f` of complexity 20 on line 1.
    fn crashing_server(restarts: RestartLimit) -> Server<Vec<u8>> {
        let script = r#"
            while IFS= read -r line; do
                case "$line" in *CRASH*) echo boom >&2; exit 3;; esac
                id=${line#\{\"id\":}; id=${id%%,*}
                printf '{"id":%s,"functions":[{"n":"f","k":"fn","l":1,"c":20}]}\n' "$id"
            done
        "#;
        server_with_worker(script, restarts)
    }

    fn server_with_worker(script: &str, restarts: RestartLimit) -> Server<Vec<u8>> {
        let supervisor = Supervisor::new(SupervisorConfig {
            command: Some(vec!["sh".into(), "-c".into(), script.into()]),
            timeout: std::time::Duration::from_secs(5),
            restarts,
            timeouts: crate::supervisor::TimeoutLimit::Unlimited,
        });
        let mut s =
            Server::with_supervisor("cmplx", &Registry::all(&[&FAKE]), Vec::new(), supervisor);
        initialize(&mut s, Value::Null);
        s
    }

    #[test]
    fn a_refused_document_has_no_diagnostics_and_is_logged_once() {
        let script = r#"
            while IFS= read -r line; do
                id=${line#\{\"id\":}; id=${id%%,*}
                printf '{"id":%s,"error":{"kind":"invalid","line":0,"message":"text larger than 30 MB"}}\n' "$id"
            done
        "#;
        let mut s = server_with_worker(script, per_minute(10));
        open(&mut s, "file:///x/a.fk", "f 20\n");
        let messages = sent(&mut s);
        let logs: Vec<&Value> = messages
            .iter()
            .filter(|m| m["method"] == "window/logMessage")
            .collect();
        assert_eq!(logs.len(), 1, "{messages:?}");
        assert_eq!(
            logs[0]["params"]["message"],
            "cmplx: file:///x/a.fk: not scored: text larger than 30 MB"
        );
        open(&mut s, "file:///x/a.fk", "f 20\n");
        assert_eq!(
            published(&mut s),
            vec![json!([])],
            "nothing published, and no second log"
        );
        assert_eq!(s.supervisor.spawned(), 1);
    }

    fn per_minute(max: usize) -> RestartLimit {
        RestartLimit::PerWindow {
            max,
            window: crate::supervisor::RESTART_WINDOW,
        }
    }

    const CRASHED: &str = "not scored: the Fake analyzer crashed (exit code 3): boom";

    fn crash_warning(text_width: usize) -> Value {
        json!([{
            "range": {
                "start": { "line": 0, "character": 0 },
                "end": { "line": 0, "character": text_width },
            },
            "severity": 2,
            "source": "cmplx",
            "message": CRASHED,
        }])
    }

    #[test]
    fn a_crash_publishes_one_warning_at_line_0_and_logs_it() {
        let mut s = crashing_server(per_minute(10));
        open(&mut s, "file:///x/a.fk", "f 20\n");
        assert_eq!(published(&mut s)[0].as_array().unwrap().len(), 1);

        open(&mut s, "file:///x/a.fk", "CRASH\n");
        let messages = sent(&mut s);
        let logs: Vec<&Value> = messages
            .iter()
            .filter(|m| m["method"] == "window/logMessage")
            .collect();
        assert_eq!(logs.len(), 1, "{messages:?}");
        assert_eq!(
            logs[0]["params"]["message"],
            format!("cmplx: file:///x/a.fk: {CRASHED}")
        );
        let lists: Vec<&Value> = messages
            .iter()
            .filter(|m| m["method"] == "textDocument/publishDiagnostics")
            .collect();
        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0]["params"]["diagnostics"], crash_warning(5));
        assert_eq!(hover_line(&s, 0), Value::Null, "no stale scores");
    }

    #[test]
    fn identical_text_that_crashed_is_answered_from_memory_without_a_restart() {
        let mut s = crashing_server(per_minute(10));
        open(&mut s, "file:///x/a.fk", "CRASH\n");
        sent(&mut s);
        assert_eq!(s.supervisor.spawned(), 1);

        open(&mut s, "file:///x/a.fk", "CRASH\n");
        let messages = sent(&mut s);
        assert_eq!(messages.len(), 1, "a publish and no log: {messages:?}");
        assert_eq!(messages[0]["params"]["diagnostics"], crash_warning(5));
        assert_eq!(s.supervisor.spawned(), 1, "the text was not sent again");

        open(&mut s, "file:///x/a.fk", "CRASH \n");
        assert_eq!(logs_only(&mut s), 1, "changed text is tried again");
        assert_eq!(s.supervisor.spawned(), 2);
    }

    #[test]
    fn a_settings_change_retries_a_remembered_crash() {
        let mut s = crashing_server(per_minute(10));
        open(&mut s, "file:///x/a.fk", "CRASH\n");
        assert_eq!(s.supervisor.spawned(), 1);
        s.init.set_option(&FAKE, "skip-all", false);
        open(&mut s, "file:///x/a.fk", "CRASH\n");
        assert_eq!(
            s.supervisor.spawned(),
            2,
            "new options, so the text is tried again"
        );
    }

    #[test]
    fn a_remembered_timeout_is_retried_after_a_minute() {
        let mut s = crashing_server(per_minute(10));
        open(&mut s, "file:///x/a.fk", "CRASH\n");
        let crash = s.crashed.get_mut("file:///x/a.fk").unwrap();
        crash.error.kind = ParseErrorKind::Crashed {
            reason: CrashReason::Timeout,
        };
        open(&mut s, "file:///x/a.fk", "CRASH\n");
        assert_eq!(
            s.supervisor.spawned(),
            1,
            "a fresh timeout is answered from memory"
        );

        let crash = s.crashed.get_mut("file:///x/a.fk").unwrap();
        crash.at = std::time::Instant::now()
            .checked_sub(TIMEOUT_RETRY_AFTER)
            .expect("the clock is past a minute");
        open(&mut s, "file:///x/a.fk", "CRASH\n");
        assert_eq!(
            s.supervisor.spawned(),
            2,
            "a minute-old timeout is tried again"
        );
    }

    #[test]
    fn a_crash_leaves_every_other_document_unaffected() {
        let mut s = crashing_server(per_minute(10));
        open(&mut s, "file:///x/bad.fk", "CRASH\n");
        sent(&mut s);
        open(&mut s, "file:///x/good.fk", "f 20\n");
        let lists = published(&mut s);
        assert_eq!(lists[0][0]["message"], "Complexity is 20 for fn 'f'");
    }

    #[test]
    fn a_fixed_then_rebroken_document_crashes_again_and_close_forgets_it() {
        let mut s = crashing_server(per_minute(10));
        open(&mut s, "file:///x/a.fk", "CRASH\n");
        open(&mut s, "file:///x/a.fk", "f 20\n");
        open(&mut s, "file:///x/a.fk", "CRASH\n");
        assert_eq!(logs_only(&mut s), 2, "the fix cleared the memory");
        s.notification(
            "textDocument/didClose",
            &json!({ "textDocument": { "uri": "file:///x/a.fk" } }),
        )
        .unwrap();
        assert!(s.crashed.is_empty());
    }

    #[test]
    fn over_the_restart_rate_documents_get_the_paused_warning_and_are_retried_later() {
        let mut s = crashing_server(per_minute(0));
        open(&mut s, "file:///x/a.fk", "CRASH\n");
        sent(&mut s);
        open(&mut s, "file:///x/b.fk", "f 20\n");
        let paused = "not scored: the analyzer kept crashing; restarting is paused for a minute";
        assert_eq!(published(&mut s)[0][0]["message"], paused);
        open(&mut s, "file:///x/b.fk", "f 20\n");
        assert_eq!(
            logs_only(&mut s),
            1,
            "a paused document is not remembered as crashing"
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
