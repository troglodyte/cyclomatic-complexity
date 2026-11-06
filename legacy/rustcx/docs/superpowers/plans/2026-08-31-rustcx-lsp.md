# rustcx LSP Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `rustcx --lsp` speaks LSP over stdio — publishing a diagnostic per over-threshold function, answering hover with the `--explain` breakdown, and reading a `rustcx.toml` that the CLI reads too.

**Architecture:** `src/config.rs` owns `rustcx.toml` (parse, walk-up discovery, precedence merge, mtime reload) and is shared by both front ends. `src/lsp.rs` is a hand-rolled JSON-RPC server ported from `jscx/src/lsp.rs`: a blocking `Content-Length` loop, `serde_json::json!` literals, no `lsp-types`, no `tower-lsp`, no async. It holds a `HashMap<String, Doc>` of the last **successful** analysis per URI, which is what lets hover answer and what keeps diagnostics stable while a buffer is mid-edit. `src/analyze.rs` is not touched.

**Tech Stack:** Rust 2021, `syn` 3, `proc-macro2`, `serde`, `serde_json`, `colored`, plus one new dependency: `toml`.

**Spec:** `docs/superpowers/specs/2026-08-31-rustcx-lsp-design.md`

## Global Constraints

- **Do not modify `src/analyze.rs`.** The core spec shaped `analyze`, `Options`, and `Func` so the LSP could be added without touching them. If a task seems to need an analyzer change, stop and raise it rather than editing.
- **`Func` has no end line.** It is `{ name, kind, line, complexity, depth, points }`. Hover must work from `line` and `points` alone; adding a field breaks the field-for-field parity with jscx that the core spec calls a hard requirement.
- **LSP character offsets are UTF-16 code units** — not bytes, not `char`s. Every column is `s.chars().map(char::len_utf16).sum::<usize>()`.
- **LSP line numbers are 0-based; `Func.line` and `Point.line` are 1-based.** Convert at exactly one place per direction and say so in a comment.
- **stdout is the protocol.** Never `println!` in `src/lsp.rs`. Diagnostics of our own go through `window/logMessage`.
- **The pin on `toml`** is whatever `cargo add toml` resolves to at implementation time. Do not copy a version from this plan.
- **The type is `FileConfig`, not `Config`.** `src/main.rs` already has a private `struct Config`; the spec's API block was renamed to match this plan.
- **Every new public item gets a doc comment**, matching the density in `src/analyze.rs` and `src/main.rs`.
- **`cargo clippy -- -D warnings` must pass** at every commit, as must `cargo run -- --max 10 --no-tests src/` (rustcx gates itself at 10).

---

### Task 1: `rustcx.toml` — parsing, discovery, precedence, reload

Everything about the config file, with no protocol involved. This is the whole of `src/config.rs` and it is fully testable on its own.

**Files:**
- Create: `src/config.rs`
- Modify: `src/lib.rs` (add `pub mod config;`)
- Modify: `Cargo.toml` (add `toml`)
- Test: `src/config.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `rustcx::analyze::Options` (fields `logical`, `question`, `tests`; `Default` is `logical: true, question: false, tests: true`).
- Produces:
  - `FileConfig { max: Option<u32>, threshold: Option<u32>, question: Option<bool>, logical: Option<bool>, tests: Option<bool> }` — `Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq`
  - `FileConfig::parse(path: &Path, text: &str) -> Result<FileConfig, ConfigError>`
  - `FileConfig::or(self, other: FileConfig) -> FileConfig` — self's `Some` fields win
  - `FileConfig::options(&self) -> Options`
  - `FileConfig::discover(start: &Path) -> Result<Option<(PathBuf, FileConfig)>, ConfigError>`
  - `ConfigError { path: PathBuf, message: String }`, implements `Display` as `{path}: {message}`
  - `Watched::discover(start: &Path) -> (Watched, Option<ConfigError>)`
  - `Watched::refresh(&mut self) -> Option<ConfigError>`
  - `Watched::config(&self) -> FileConfig`

- [ ] **Step 1: Add the dependency**

```bash
cargo add toml
```

Confirm `Cargo.toml` gained a `toml` line under `[dependencies]`. Do not add `serde` — it is already there with `derive`.

- [ ] **Step 2: Declare the module**

In `src/lib.rs`, which currently reads `pub mod analyze;`:

```rust
pub mod analyze;
pub mod config;
```

- [ ] **Step 3: Write the failing tests**

Create `src/config.rs` containing **only** the test module below plus the imports it needs. It will not compile yet — that is the point.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A unique empty directory under the system temp dir. No `tempfile`
    /// dependency: one counter plus the pid is enough for a test process.
    fn temp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("rustcx-{}-{}-{}", tag, std::process::id(), n));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_empty_file_sets_nothing() {
        let cfg = FileConfig::parse(Path::new("rustcx.toml"), "").unwrap();
        assert_eq!(cfg, FileConfig::default());
        assert_eq!(cfg.max, None, "unset is not the same as zero");
    }

    #[test]
    fn every_key_parses() {
        let text = "\
max = 15
threshold = 8
question = true
logical = false
tests = false
";
        let cfg = FileConfig::parse(Path::new("rustcx.toml"), text).unwrap();
        assert_eq!(cfg.max, Some(15));
        assert_eq!(cfg.threshold, Some(8));
        assert_eq!(cfg.question, Some(true));
        assert_eq!(cfg.logical, Some(false));
        assert_eq!(cfg.tests, Some(false));
    }

    #[test]
    fn an_unknown_key_is_an_error() {
        let err = FileConfig::parse(Path::new("rustcx.toml"), "treshold = 8").unwrap_err();
        assert!(
            err.message.contains("treshold") || err.message.contains("unknown"),
            "a typo must be reported, got: {}",
            err.message
        );
    }

    #[test]
    fn a_wrong_type_is_an_error() {
        assert!(FileConfig::parse(Path::new("rustcx.toml"), "max = \"fifteen\"").is_err());
    }

    #[test]
    fn options_layers_over_the_defaults() {
        let d = Options::default();
        assert_eq!(FileConfig::default().options().logical, d.logical);
        assert_eq!(FileConfig::default().options().question, d.question);
        assert_eq!(FileConfig::default().options().tests, d.tests);

        let cfg = FileConfig { tests: Some(false), ..FileConfig::default() };
        assert!(!cfg.options().tests);
        assert!(cfg.options().logical, "an unset key keeps its default");
    }

    #[test]
    fn or_prefers_the_left_hand_side() {
        let high = FileConfig { threshold: Some(20), ..FileConfig::default() };
        let low = FileConfig { threshold: Some(5), max: Some(9), ..FileConfig::default() };
        let merged = high.or(low);
        assert_eq!(merged.threshold, Some(20), "the left side wins where it is set");
        assert_eq!(merged.max, Some(9), "and falls through where it is not");
    }

    #[test]
    fn discover_finds_a_file_in_a_parent_directory() {
        let root = temp_dir("discover");
        let nested = root.join("src").join("deep");
        fs::create_dir_all(&nested).unwrap();
        fs::write(root.join("rustcx.toml"), "threshold = 7").unwrap();

        let (path, cfg) = FileConfig::discover(&nested).unwrap().expect("walks up to the root");
        assert_eq!(path, root.join("rustcx.toml"));
        assert_eq!(cfg.threshold, Some(7));
    }

    #[test]
    fn discover_returns_none_when_there_is_no_file() {
        let root = temp_dir("nofile");
        assert!(FileConfig::discover(&root).unwrap().is_none());
    }

    #[test]
    fn a_reload_picks_up_a_changed_file() {
        let root = temp_dir("reload");
        let path = root.join("rustcx.toml");
        fs::write(&path, "threshold = 5").unwrap();

        let (mut watched, err) = Watched::discover(&root);
        assert!(err.is_none());
        assert_eq!(watched.config().threshold, Some(5));

        // A filesystem mtime can have one-second granularity, so set it
        // explicitly rather than racing it.
        fs::write(&path, "threshold = 30").unwrap();
        bump_mtime(&path);

        assert!(watched.refresh().is_none());
        assert_eq!(watched.config().threshold, Some(30));
    }

    #[test]
    fn a_broken_reload_keeps_the_previous_config() {
        let root = temp_dir("broken");
        let path = root.join("rustcx.toml");
        fs::write(&path, "threshold = 5").unwrap();
        let (mut watched, _) = Watched::discover(&root);

        fs::write(&path, "threshold = = 5").unwrap();
        bump_mtime(&path);

        let err = watched.refresh().expect("the error is reported");
        assert!(err.to_string().contains("rustcx.toml"));
        assert_eq!(
            watched.config().threshold,
            Some(5),
            "a broken edit must not silently revert the thresholds someone is working against"
        );
    }

    /// Push a file's mtime one minute into the future, so a reload is detected
    /// regardless of filesystem timestamp granularity.
    fn bump_mtime(path: &Path) {
        let meta = fs::metadata(path).unwrap();
        let later = meta.modified().unwrap() + std::time::Duration::from_secs(60);
        filetime_set(path, later);
    }
}
```

`filetime_set` needs no crate — add this helper next to the tests, still inside `mod tests`:

```rust
    /// `File::set_times` has been stable since Rust 1.75.
    fn filetime_set(path: &Path, when: std::time::SystemTime) {
        let f = fs::OpenOptions::new().write(true).open(path).unwrap();
        f.set_times(fs::FileTimes::new().set_modified(when)).unwrap();
    }
```

- [ ] **Step 4: Run the tests to verify they fail**

Run: `cargo test --lib config`
Expected: compile errors — `cannot find type FileConfig`, `cannot find type Watched`.

- [ ] **Step 5: Write the implementation**

Put this **above** the test module in `src/config.rs`:

```rust
//! `rustcx.toml`, shared by the CLI and the language server.
//!
//! Every field is `Option` so that "unset" is distinguishable from "set to the
//! value that happens to be the default". That distinction is what makes the
//! precedence layering work: without it, a `logical = true` in a file could not
//! be told apart from silence, and a CLI flag could not know whether it was
//! overriding anything.

use crate::analyze::Options;
use serde::Deserialize;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The name looked for when walking up from a starting directory.
pub const FILE_NAME: &str = "rustcx.toml";

/// A parsed `rustcx.toml`. Unset fields fall through to the next layer.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileConfig {
    /// CLI: the default for `--max`. Unrelated to `threshold` — see the spec.
    pub max: Option<u32>,
    /// LSP: warn at this score, error at twice it.
    pub threshold: Option<u32>,
    pub question: Option<bool>,
    pub logical: Option<bool>,
    pub tests: Option<bool>,
}

/// A config file that could not be read or parsed.
#[derive(Clone, Debug)]
pub struct ConfigError {
    pub path: PathBuf,
    pub message: String,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.message)
    }
}

impl FileConfig {
    /// Parse TOML text. Unknown keys are rejected: a typo'd `treshold` that
    /// silently does nothing is a worse failure than a loud one.
    pub fn parse(path: &Path, text: &str) -> Result<FileConfig, ConfigError> {
        toml::from_str(text).map_err(|e| ConfigError {
            path: path.to_path_buf(),
            message: e.to_string(),
        })
    }

    /// Read and parse one file.
    pub fn load(path: &Path) -> Result<FileConfig, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|e| ConfigError {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        FileConfig::parse(path, &text)
    }

    /// Layer `self` over `other`: a field set here wins, an unset one falls
    /// through. Used for both `flag > file` and `initializationOptions > file`.
    pub fn or(self, other: FileConfig) -> FileConfig {
        FileConfig {
            max: self.max.or(other.max),
            threshold: self.threshold.or(other.threshold),
            question: self.question.or(other.question),
            logical: self.logical.or(other.logical),
            tests: self.tests.or(other.tests),
        }
    }

    /// Analyzer options with this config layered over the built-in defaults.
    pub fn options(&self) -> Options {
        let d = Options::default();
        Options {
            logical: self.logical.unwrap_or(d.logical),
            question: self.question.unwrap_or(d.question),
            tests: self.tests.unwrap_or(d.tests),
        }
    }

    /// Walk up from `start` to the filesystem root, loading the first
    /// `rustcx.toml` found. `Ok(None)` means there is no config file.
    pub fn discover(start: &Path) -> Result<Option<(PathBuf, FileConfig)>, ConfigError> {
        for dir in start.ancestors() {
            let path = dir.join(FILE_NAME);
            if path.is_file() {
                let cfg = FileConfig::load(&path)?;
                return Ok(Some((path, cfg)));
            }
        }
        Ok(None)
    }
}

/// A discovered config file plus the mtime it was read at.
///
/// The language server re-reads on change rather than registering a client file
/// watcher: a `stat` is microseconds against a parse that is milliseconds, and
/// unlike `workspace/didChangeWatchedFiles` it needs no client capability and
/// behaves the same in every editor.
#[derive(Clone, Debug, Default)]
pub struct Watched {
    path: Option<PathBuf>,
    mtime: Option<SystemTime>,
    config: FileConfig,
}

impl Watched {
    /// Discover a config file under `start`. Any error is returned alongside a
    /// usable (default) config rather than replacing it, so a bad file at
    /// startup degrades to built-in defaults instead of preventing a session.
    pub fn discover(start: &Path) -> (Watched, Option<ConfigError>) {
        match FileConfig::discover(start) {
            Ok(Some((path, config))) => {
                let mtime = mtime_of(&path);
                (Watched { path: Some(path), mtime, config }, None)
            }
            Ok(None) => (Watched::default(), None),
            Err(e) => {
                // Remember the path so a later fix to the file is picked up.
                let path = e.path.clone();
                let mtime = mtime_of(&path);
                (Watched { path: Some(path), mtime, config: FileConfig::default() }, Some(e))
            }
        }
    }

    /// Re-read the file if its mtime changed. Returns an error to log.
    ///
    /// On a parse failure the previous config is kept: reverting to defaults
    /// mid-session would silently change the thresholds someone is working
    /// against. The mtime is still updated, so one bad edit logs once.
    pub fn refresh(&mut self) -> Option<ConfigError> {
        let path = self.path.clone()?;
        let now = mtime_of(&path);
        if now == self.mtime {
            return None;
        }
        self.mtime = now;
        match FileConfig::load(&path) {
            Ok(config) => {
                self.config = config;
                None
            }
            Err(e) => Some(e),
        }
    }

    /// The config as last successfully read.
    pub fn config(&self) -> FileConfig {
        self.config
    }
}

/// A file's mtime, or `None` if it cannot be stat'd. A missing file compares
/// equal to a missing file, so a deleted config does not re-read every cycle.
fn mtime_of(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test --lib config`
Expected: 9 passed.

Then `cargo clippy --all-targets -- -D warnings`. Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock src/lib.rs src/config.rs
git commit -m "feat: rustcx.toml — parsing, discovery, precedence, mtime reload"
```

---

### Task 2: `--lsp`, framing, and the message loop

The server that starts, handshakes, and stops cleanly. No diagnostics and no hover yet — this task exists so the framing is proven by an end-to-end test before any feature rides on it, which is where hand-rolled LSP servers actually break.

**Files:**
- Create: `src/lsp.rs`
- Create: `tests/lsp.rs`
- Modify: `src/lib.rs`
- Modify: `src/main.rs` (`HELP`, `struct Config`, `Default`, `FLAGS`, `parse_args`, `main`)

**Interfaces:**
- Consumes: `rustcx::config::{FileConfig, Watched}` from Task 1.
- Produces:
  - `rustcx::lsp::run() -> std::io::Result<()>`
  - `rustcx::lsp::DEFAULT_THRESHOLD: u32 = 11`
  - `struct Server` (private) with `docs: HashMap<String, Doc>`, `config: Watched`, `init: FileConfig` — Tasks 3–5 add methods to it
  - `tests/lsp.rs` helper `Server` with `start()`, `send(serde_json::Value)`, `recv() -> serde_json::Value` — reused by Tasks 3–5

- [ ] **Step 1: Write the failing tests**

Create `tests/lsp.rs`:

```rust
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test lsp`
Expected: FAIL. The binary does not know `--lsp`, so it exits 2 with `unknown option: --lsp`, and `recv` panics on a closed stdout.

- [ ] **Step 3: Write the server**

Create `src/lsp.rs`:

```rust
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
use std::io::{self, BufRead, Read, Write};
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
```

- [ ] **Step 4: Wire up the module and the flag**

In `src/lib.rs`:

```rust
pub mod analyze;
pub mod config;
pub mod lsp;
```

In `src/main.rs`, add to `HELP` after the `--no-tests` line:

```
        --lsp          run as a Language Server (stdio)
```

Add the field to `struct Config`:

```rust
    explain: bool,
    lsp: bool,
    opts: Options,
```

and to its `Default`:

```rust
            explain: false,
            lsp: false,
            opts: Options::default(),
```

Add a row to `FLAGS`, after the `--no-tests` row:

```rust
    ("", "--lsp", Action::Set(|c| c.lsp = true)),
```

In `parse_args`, the "no paths given" check must not fire for a server that reads no paths. Replace:

```rust
    if cfg.paths.is_empty() && !cfg.stdin {
        return Err("no paths given".to_string());
    }
```

with:

```rust
    if cfg.paths.is_empty() && !cfg.stdin && !cfg.lsp {
        return Err("no paths given".to_string());
    }
```

In `main`, hand off before `run(&cfg)`:

```rust
    if cfg.lsp {
        if let Err(e) = rustcx::lsp::run() {
            eprintln!("rustcx: LSP error: {e}");
            return ExitCode::from(2);
        }
        return ExitCode::SUCCESS;
    }
    run(&cfg)
```

- [ ] **Step 5: Add the argument test**

In `src/main.rs`'s existing `mod tests`, next to `scoring_flags_flip_their_options`:

```rust
    #[test]
    fn the_lsp_flag_needs_no_paths() {
        let c = parse(&["--lsp"]);
        assert!(c.lsp);
        assert!(c.paths.is_empty(), "a server reads no paths");
    }
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test`
Expected: all pass, including 5 in `tests/lsp.rs` and 6 new unit tests in `src/lsp.rs`.

Then `cargo clippy --all-targets -- -D warnings`.

- [ ] **Step 7: Commit**

```bash
git add src/lib.rs src/lsp.rs src/main.rs tests/lsp.rs
git commit -m "feat: --lsp, JSON-RPC framing, and the message loop"
```

---

### Task 3: Diagnostics

**Files:**
- Modify: `src/lsp.rs`
- Modify: `tests/lsp.rs`

**Interfaces:**
- Consumes: `Server`, `Doc`, `Server::log`, `Server::send`, `Server::threshold`, `Server::settings` (Task 2); `rustcx::analyze::{analyze, Func}`.
- Produces:
  - `fn diagnostics_for(funcs: &[Func], text: &str, threshold: u32) -> Vec<serde_json::Value>`
  - `Server::update(&mut self, uri: &str, text: &str) -> io::Result<()>`
  - `Server::publish(&mut self, uri: &str, diagnostics: Vec<serde_json::Value>) -> io::Result<()>`

- [ ] **Step 1: Write the failing tests**

Add to `src/lsp.rs`'s `mod tests`:

```rust
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
        let src = format!("// 🦀🦀 a four-byte crab is two UTF-16 units\n{}\n", one_liner("f", 9));
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
```

Add to `tests/lsp.rs`:

```rust
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
    s.did_open("file:///x.rs", &format!("{}{}", branchy("small", 1), branchy("big", 20)));

    let params = s.diagnostics();
    assert_eq!(params["uri"], "file:///x.rs");
    let list = params["diagnostics"].as_array().unwrap();
    assert_eq!(list.len(), 1, "only `big` is over the default threshold of 11");
    assert_eq!(list[0]["message"], "Complexity is 21 for fn 'big'");
    assert_eq!(list[0]["severity"], 1, "21 is at least twice 11");
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test`
Expected: `cannot find function diagnostics_for`, and the `tests/lsp.rs` cases hang or fail because no diagnostics are ever published.

- [ ] **Step 3: Write the implementation**

In `src/lsp.rs`, add above `mod tests`:

```rust
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
```

Add to `impl Server`:

```rust
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

        let Ok(funcs) = result else { return Ok(()) };
        let diagnostics = diagnostics_for(&funcs, text, self.threshold());
        self.docs.entry(uri.to_string()).or_default().funcs = funcs;
        self.publish(uri, diagnostics)
    }
```

Add the notification arms to `run`'s `match`, before the `_` arm:

```rust
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
```

Note the `let…else`/`if let` shape: jscx uses `params["textDocument"]["uri"].as_str().unwrap()` at `src/lsp.rs:185` and `:192`, which panics on a malformed notification. A missing field skips the frame here.

Finally, delete the three `#[allow(dead_code)]` attributes Task 2 put on `struct Doc`, on `Server::docs`, and on `Server::threshold`. All three now have real consumers, and leaving the attributes behind would suppress a future warning that means something.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test`
Expected: all pass — 5 new unit tests and 4 new end-to-end tests.

Then `cargo clippy --all-targets -- -D warnings`.

- [ ] **Step 5: Commit**

```bash
git add src/lsp.rs tests/lsp.rs
git commit -m "feat: publish complexity diagnostics on open, change, and close"
```

---

### Task 4: Hover

**Files:**
- Modify: `src/lsp.rs`
- Modify: `tests/lsp.rs`

**Interfaces:**
- Consumes: `Doc.funcs` (Task 3), `Server::respond` (Task 2).
- Produces:
  - `fn hover_at(funcs: &[Func], line: u32) -> Option<String>` — `line` is **1-based**; returns Markdown
  - a `textDocument/hover` arm in `run`

- [ ] **Step 1: Write the failing tests**

Add to `src/lsp.rs`'s `mod tests`:

```rust
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
```

Add to `tests/lsp.rs`:

```rust
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test`
Expected: `cannot find function hover_at`; the end-to-end hover tests block waiting for a reply that never comes.

- [ ] **Step 3: Write the implementation**

Add to `src/lsp.rs` above `mod tests`:

```rust
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
```

Add the request arm to `run`'s `match`, before the `_` arm:

```rust
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test`
Expected: all pass — 5 new unit tests and 2 new end-to-end tests.

Then `cargo clippy --all-targets -- -D warnings`.

- [ ] **Step 5: Commit**

```bash
git add src/lsp.rs tests/lsp.rs
git commit -m "feat: hover showing the decision points behind a score"
```

---

### Task 5: Parse failures — keep the last good analysis, log once

**Files:**
- Modify: `src/lsp.rs`
- Modify: `tests/lsp.rs`

**Interfaces:**
- Consumes: `Doc.last_error` (declared in Task 2), `Server::update` (Task 3), `Server::log` (Task 2).
- Produces: no new signatures — `Server::update` gains its `Err` arm.

- [ ] **Step 1: Write the failing tests**

Add to `tests/lsp.rs`:

```rust
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test lsp`
Expected: the log-message tests block — `update` currently returns `Ok(())` silently on a parse error.

- [ ] **Step 3: Write the implementation**

In `src/lsp.rs`, replace the `let Ok(funcs) = result else { return Ok(()) };` line in `Server::update` with:

```rust
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
```

and set `last_error = None` on success, so a file that breaks, is fixed, then breaks the same way again logs both times. In the success path, replace:

```rust
        self.docs.entry(uri.to_string()).or_default().funcs = funcs;
```

with:

```rust
        let doc = self.docs.entry(uri.to_string()).or_default();
        doc.funcs = funcs;
        doc.last_error = None;
```

Add a unit test to `mod tests` covering the dedupe rule directly:

```rust
    #[test]
    fn a_recovered_document_forgets_its_last_error() {
        let mut doc = Doc::default();
        doc.last_error = Some("x.rs: 2: expected `}`".to_string());
        // What the success path does.
        doc.last_error = None;
        assert!(
            doc.last_error.is_none(),
            "a file that breaks, is fixed, then breaks the same way logs twice"
        );
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test`
Expected: all pass.

Then `cargo clippy --all-targets -- -D warnings`.

- [ ] **Step 5: Commit**

```bash
git add src/lsp.rs tests/lsp.rs
git commit -m "feat: keep the last good analysis when a buffer stops parsing"
```

---

### Task 6: The CLI reads `rustcx.toml`

Flags win over the file, the file wins over the built-in defaults. This needs `parse_args` to record **whether a flag appeared**, not just its effect — which is why `FileConfig`'s fields are `Option`.

**Files:**
- Modify: `src/main.rs`
- Test: `src/main.rs` (`mod tests`), `tests/cli.rs`

**Interfaces:**
- Consumes: `rustcx::config::FileConfig` (Task 1).
- Produces:
  - `struct Config` gains `cli: FileConfig`
  - `fn resolve(cfg: &mut Config, start: &Path) -> Result<(), String>`

- [ ] **Step 1: Write the failing tests**

Replace the existing `scoring_flags_flip_their_options` test in `src/main.rs`'s `mod tests` — flags now record intent, and `opts` is resolved later:

```rust
    #[test]
    fn scoring_flags_record_their_intent() {
        assert_eq!(parse(&["--no-logical", "x.rs"]).cli.logical, Some(false));
        assert_eq!(parse(&["--question", "x.rs"]).cli.question, Some(true));
        assert_eq!(parse(&["--no-tests", "x.rs"]).cli.tests, Some(false));
        assert_eq!(
            parse(&["x.rs"]).cli.tests,
            None,
            "an absent flag is unset, not false — that is what lets a file win"
        );
    }
```

and add:

```rust
    #[test]
    fn a_flag_beats_the_file_and_the_file_beats_the_default() {
        use rustcx::config::FileConfig;

        // Flag set, file set: the flag wins.
        let mut cfg = parse(&["--no-tests", "x.rs"]);
        let file = FileConfig { tests: Some(true), max: Some(9), ..FileConfig::default() };
        apply(&mut cfg, file);
        assert!(!cfg.opts.tests, "the flag wins");
        assert_eq!(cfg.max, Some(9), "and an unset flag takes the file's value");

        // Neither set: the built-in default.
        let mut cfg = parse(&["x.rs"]);
        apply(&mut cfg, FileConfig::default());
        assert!(cfg.opts.tests, "the built-in default is to score tests");
        assert_eq!(cfg.max, None);
    }
```

`apply` is the pure half of `resolve`, so it can be tested without a filesystem. Add it to `mod tests`' imports by defining it in the implementation step below.

Add to `tests/cli.rs` — check its existing helpers first and follow them; it already builds and runs the binary:

```rust
#[test]
fn a_config_file_supplies_the_max_gate() {
    let dir = temp_dir("cli-config");
    std::fs::write(dir.join("rustcx.toml"), "max = 2\n").unwrap();
    std::fs::write(dir.join("busy.rs"), "fn f(a: bool) { if a {} if a {} if a {} }\n").unwrap();

    // Complexity 4 is over `max = 2`, so the gate fails with no --max flag.
    let out = run_in(&dir, &["busy.rs"]);
    assert_eq!(out.status.code(), Some(1), "rustcx.toml gates the run");
}

#[test]
fn a_flag_overrides_the_config_file() {
    let dir = temp_dir("cli-override");
    std::fs::write(dir.join("rustcx.toml"), "max = 2\n").unwrap();
    std::fs::write(dir.join("busy.rs"), "fn f(a: bool) { if a {} if a {} if a {} }\n").unwrap();

    let out = run_in(&dir, &["--max", "10", "busy.rs"]);
    assert_eq!(out.status.code(), Some(0), "the flag wins over the file");
}

#[test]
fn a_broken_config_file_is_reported() {
    let dir = temp_dir("cli-broken");
    std::fs::write(dir.join("rustcx.toml"), "treshold = 8\n").unwrap();
    std::fs::write(dir.join("x.rs"), "fn f() {}\n").unwrap();

    let out = run_in(&dir, &["x.rs"]);
    assert_eq!(out.status.code(), Some(2), "a typo'd key is loud, not silent");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("rustcx.toml"), "got: {err}");
}
```

If `tests/cli.rs` has no `temp_dir` or `run_in` helper that sets the working directory, add them, modelled on whatever spawn helper is already there:

```rust
fn run_in(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_rustcx"))
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run rustcx")
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test`
Expected: `no field cli on type Config`, `cannot find function apply`.

- [ ] **Step 3: Write the implementation**

In `src/main.rs`, add the import:

```rust
use rustcx::config::FileConfig;
```

Add the field to `struct Config` and its `Default`:

```rust
    /// What the flags asked for, before the config file is layered under them.
    cli: FileConfig,
```

```rust
            cli: FileConfig::default(),
```

Change the three scoring rows and the `--max` row in `FLAGS` to record intent rather than mutate `opts` directly:

```rust
    ("-M", "--max", Action::Num(|c, n| c.cli.max = Some(n))),
```

```rust
    ("", "--question", Action::Set(|c| c.cli.question = Some(true))),
    ("", "--no-logical", Action::Set(|c| c.cli.logical = Some(false))),
    ("", "--no-tests", Action::Set(|c| c.cli.tests = Some(false))),
```

`parse_args` is otherwise unchanged: it stays pure and filesystem-free.

Add the resolution functions above `main`:

```rust
/// Layer a config file under the flags. Pure, so it is testable without a
/// filesystem; `resolve` supplies the file.
fn apply(cfg: &mut Config, file: FileConfig) {
    let merged = cfg.cli.or(file);
    cfg.opts = merged.options();
    cfg.max = merged.max;
}

/// Find a `rustcx.toml` by walking up from `start`, and layer it under the
/// flags. An unreadable or unparseable file is an error, not a shrug: a typo'd
/// key that silently does nothing is the failure this guards against.
fn resolve(cfg: &mut Config, start: &Path) -> Result<(), String> {
    let file = match FileConfig::discover(start) {
        Ok(Some((_, file))) => file,
        Ok(None) => FileConfig::default(),
        Err(e) => return Err(e.to_string()),
    };
    apply(cfg, file);
    Ok(())
}
```

In `main`, resolve after parsing and before the `--lsp` handoff — the server does its own discovery from `rootUri`, so it does not need this, but a flag error should still be reported first:

```rust
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut cfg = match parse_args(&args) {
        Ok(Some(cfg)) => cfg,
        Ok(None) => return ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("rustcx: {msg}");
            return ExitCode::from(2);
        }
    };

    if cfg.lsp {
        if let Err(e) = rustcx::lsp::run() {
            eprintln!("rustcx: LSP error: {e}");
            return ExitCode::from(2);
        }
        return ExitCode::SUCCESS;
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if let Err(msg) = resolve(&mut cfg, &cwd) {
        eprintln!("rustcx: {msg}");
        return ExitCode::from(2);
    }

    run(&cfg)
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test`
Expected: all pass. In particular `defaults_match_the_spec` must still pass — it asserts on `c.opts`, which `parse_args` still leaves at `Options::default()`.

Then `cargo clippy --all-targets -- -D warnings`, and confirm the self-gate still holds: `cargo run -- --max 10 --no-tests src/`.

- [ ] **Step 5: Commit**

```bash
git add src/main.rs tests/cli.rs
git commit -m "feat: the CLI reads rustcx.toml, under its flags"
```

---

### Task 7: README

**Files:**
- Modify: `README.md`

**Interfaces:**
- Consumes: the finished binary, for real captured output.

- [ ] **Step 1: Capture real output**

```bash
cargo build --release
printf 'Content-Length: 91\r\n\r\n{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"rootUri":null,"capabilities":{}}}' | ./target/release/rustcx --lsp
```

Use what it actually prints. Do not invent a capabilities response.

- [ ] **Step 2: Add the options-table row**

In the existing options table in `README.md`, after the `--no-tests` row:

```markdown
| `--lsp` | run as a Language Server over stdio |
```

- [ ] **Step 3: Write the new sections**

Add after the "Finding what to refactor first" section, before "Scoring":

````markdown
## In your editor

`rustcx --lsp` speaks LSP over stdio. It publishes a warning for every function
at or above a threshold — an error at twice it — and answers hover over a
function with the decision points behind its score.

Hover works on a function's signature line and on any line it was charged a
point for. Those are the branchy lines, which are the ones worth hovering; a
blank line inside a function answers nothing.

### Neovim

```lua
vim.lsp.config('rustcx', {
  cmd = { 'rustcx', '--lsp' },
  filetypes = { 'rust' },
  root_markers = { 'Cargo.toml', '.git' },
})
vim.lsp.enable('rustcx')
```

### Helix

In `languages.toml`:

```toml
[language-server.rustcx]
command = "rustcx"
args = ["--lsp"]

[[language]]
name = "rust"
language-servers = ["rust-analyzer", "rustcx"]
```

### VS Code and Zed

Neither can attach an arbitrary stdio language server from settings alone —
both need a small extension to register the binary. None is shipped here.

### While you type

A buffer is unparseable for most of the time you are typing into it. rustcx
keeps the last diagnostics it published and keeps answering hover from the last
successful parse, rather than clearing warnings on every keystroke or reporting
a syntax error rust-analyzer has already reported. The parse failure is written
to the language-server log (`:LspLog` in Neovim), once per distinct error.

## Configuration

`rustcx.toml`, found by walking up from the working directory — or from the
workspace root, under `--lsp`:

```toml
max       = 15     # CI: fail above this score
threshold = 11     # editor: warn from this score, error from twice it
question  = false  # count `?` as a branch
logical   = true   # count `&&` and `||`
tests     = true   # score #[test] functions and #[cfg(test)] modules
```

Command-line flags win over the file; the file wins over the defaults above.
Under `--lsp`, an editor's `initializationOptions` take the flags' place. An
unknown key is an error rather than a shrug — a typo'd `treshold` that silently
did nothing would be worse.

The server re-reads the file whenever its modification time changes, so a new
threshold takes effect on the next keystroke without restarting anything.

**`max` and `threshold` are independent.** `max` gates CI; `threshold` is the
editor's warning level, defaulting to 11. Setting only `max = 15` leaves your
editor warning from 11 while CI fails only above 15. That is deliberate — it
lets the editor nag before the build breaks — but if you want them identical,
set both.
````

- [ ] **Step 4: Extend the Limitations section**

Add to the existing bulleted list in `README.md`'s "Limitations":

```markdown
- **In the editor, hover and diagnostics reflect the last successful parse**, so
  both lag a buffer that is mid-edit, and hover answers only on a function's
  signature line and its decision-point lines.
- **The server analyzes one document at a time on its message loop.** A single
  very large file blocks the loop for the length of its parse.
- **A panic takes the server down.** The release profile sets `panic = "abort"`,
  so a panic inside `syn` ends the process rather than being caught. Editors
  restart a dead language server; the alternative would hide real analyzer bugs
  behind a silently degraded session.
```

- [ ] **Step 5: Verify every command in the README runs**

Run each command block. Confirm the Neovim and Helix snippets match the current
documented API for each (`vim.lsp.config` requires Neovim 0.11+; say so in the
snippet's surrounding prose if the check shows an older API is still common).

- [ ] **Step 6: Commit**

```bash
git add README.md
git commit -m "docs: editor setup, rustcx.toml, and the LSP's limitations"
```

---

## Done

`rustcx --lsp` publishes diagnostics, answers hover with the `--explain`
breakdown, survives a buffer that does not parse, and shares a `rustcx.toml`
with the CLI. `src/analyze.rs` is unchanged, which is what the core spec's API
shaping was for.
