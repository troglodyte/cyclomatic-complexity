# rustcx LSP — design

Diagnostics and hover for Rust complexity, in the editor. A port of
[`jscx`](https://github.com/trog/jscx)'s `src/lsp.rs`, extended with the two
things rustcx has that jscx does not: a `points` breakdown worth showing, and an
analyzer that can fail to parse.

The core CLI spec (`2026-08-31-rustcx-design.md`) listed `--lsp` as out of
scope, and shaped `analyze`, `Options`, and `Func` so it could be added without
changing them. This spec is that addition, and it holds `src/analyze.rs` to that
bet: **no change to the analyzer.**

## Goal

`rustcx --lsp` speaks LSP over stdio. It publishes one diagnostic per function
at or above a threshold, and answers hover over a function with the decision
points behind its score — the `--explain` output, in the editor. Configuration
lives in a `rustcx.toml` the CLI reads too, and takes effect without restarting
the server.

## Approach

Hand-rolled JSON-RPC, as in jscx: `serde_json::json!` literals rather than
`lsp-types`, a blocking `Content-Length` read loop rather than `tower-lsp`, and
no async runtime. The server handles one message at a time; an analysis is
milliseconds, so there is nothing to overlap and no state to share between
threads.

This matches the CLI's hand-rolled argument parsing. The whole binary stays
dependency-light and synchronous.

## Module layout

```
src/lib.rs      pub mod analyze; pub mod config; pub mod lsp;
src/analyze.rs  unchanged
src/config.rs   new, ~120 lines — rustcx.toml, shared by both front ends
src/lsp.rs      new, ~450 lines — framing, dispatch, diagnostics, hover
src/main.rs     a --lsp arm; config feeding flag defaults
```

`src/lsp.rs` carries the same split point the core spec set for `analyze.rs`: if
it passes roughly 500 lines, the protocol plumbing (framing, dispatch, send)
separates from the feature handlers (diagnostics, hover).

### Public API

```rust
// src/lsp.rs
pub const DEFAULT_THRESHOLD: u32 = 11;
pub fn run() -> std::io::Result<()>;

// src/config.rs
pub struct FileConfig {
    pub max: Option<u32>,
    pub threshold: Option<u32>,
    pub question: Option<bool>,
    pub logical: Option<bool>,
    pub tests: Option<bool>,
}
impl FileConfig {
    /// Walk up from `start` to the filesystem root, loading the first
    /// `rustcx.toml` found. `Ok(None)` when there is none.
    pub fn discover(start: &Path) -> Result<Option<(PathBuf, FileConfig)>, ConfigError>;
    /// Analyzer options with this config layered over the built-in defaults.
    pub fn options(&self) -> Options;
}
```

Every field is `Option`, so "unset" is distinguishable from "set to the value
that happens to be the default". That distinction is what makes the precedence
layering work; without it a `logical = true` in a file could not be told apart
from silence, and a CLI flag could not know whether it was overriding anything.

## Dependencies

One addition: `toml`, pinned to the current release at implementation time. `serde` is already a dependency. No `lsp-types`, no
`tower-lsp`, no `tokio`, no file-watching crate.

## Protocol surface

Advertised at `initialize`:

```json
{ "capabilities": { "textDocumentSync": 1, "hoverProvider": true } }
```

`textDocumentSync: 1` is full sync — every `didChange` carries the entire
buffer. Incremental sync would mean reconstructing the document from ranges for
no gain, since `analyze` re-parses the whole file regardless.

| Method | Handling |
| --- | --- |
| `initialize` | read `initializationOptions`, reply with capabilities |
| `initialized` | ignored (notification) |
| `textDocument/didOpen` | analyze, publish |
| `textDocument/didChange` | analyze, publish |
| `textDocument/didSave` | no `text` under full sync; deliberately a no-op |
| `textDocument/didClose` | drop the cached doc, publish an empty diagnostic list |
| `textDocument/hover` | answer from the cached functions |
| `shutdown` | reply `null` |
| `exit` | return |
| any other request (has `id`) | reply `-32601 MethodNotFound` |
| any other notification | ignored |

## Document state

Hover receives a URI and a position, nothing else, so the server keeps state
where jscx's is a pure function of each notification:

```rust
struct Doc {
    funcs: Vec<Func>,           // last SUCCESSFUL analysis
    last_error: Option<String>, // last parse error logged, for dedupe
}
// HashMap<String, Doc>, keyed by URI
```

The document text is deliberately **not** stored. Full sync means every change
carries the whole buffer, so there is nothing to reconstruct; diagnostics are
built at the moment a parse succeeds, and the UTF-16 line widths they need are
computed there and discarded.

`Func` is owned data — the core spec's span-lifetime invariant — so caching it
across messages is safe with no `syn` or `proc_macro2` value held anywhere.

## Diagnostics

Published on `didOpen` and `didChange`, for every function whose complexity is
at or above `threshold`:

| Field | Value |
| --- | --- |
| `range` | the function's line, character 0 to the line's UTF-16 length |
| `severity` | `2` (Warning), or `1` (Error) at `2 × threshold` |
| `source` | `"rustcx"` |
| `message` | `Complexity is {n} for {kind} '{name}'` |

LSP character offsets are UTF-16 code units — not bytes, not chars. Line widths
come from `line.chars().map(char::len_utf16).sum()`. A line containing an emoji
or CJK text gets this wrong under any other measure, and the error is silent:
the range simply ends in the wrong column.

The message format matches jscx's exactly, so an editor showing both tools reads
consistently. `FnKind::label()` supplies the kind (jscx spells it `as_str`).

## Hover

`Func` carries a start line and no end line, so the server cannot ask "which
function contains this position" directly. Adding `end_line` would break the
field-for-field parity with jscx that the core spec calls a hard requirement, so
hover instead answers on the lines it already knows about:

- the function's **signature line** → the full breakdown, and
- any line in its **`points`** → that decision point, named, plus the parent's
  total.

Every point is a line the function was charged for, which makes the branchy
lines — the ones worth hovering — exactly the ones that answer. Where two
functions could match, the innermost wins, so hovering inside a closure reports
the closure rather than its parent.

Rendered as Markdown:

```
parse_args — complexity 12

  line 121  if
  line 124  match arm
  line 124  &&
  line 130  for
```

**Limitation:** hovering a blank or unbranching line inside a function returns
nothing. The alternative was an `end_line` field on `Func`; this is the cost of
not having one.

Because hover answers from the last good parse, its line numbers can lag the
buffer while a file is mid-edit. This is the same staleness the diagnostics
have, and for the same reason.

## Configuration

`rustcx.toml`, read by both front ends:

```toml
max       = 15     # CLI: --max default
threshold = 11     # LSP: warn at this, error at 2×
question  = false
logical   = true
tests     = false
```

### Precedence

```
CLI:  flag                  > rustcx.toml > built-in
LSP:  initializationOptions > rustcx.toml > built-in (threshold 11)
```

### Discovery

One function, used by both: walk up from a starting directory until a
`rustcx.toml` or the filesystem root. The CLI starts at the current directory;
the server starts at `rootUri` (falling back to the first `workspaceFolders`
entry, then the current directory).

### Reload

Before each analysis, `fs::metadata(path).modified()`. If the mtime differs from
the one cached, re-read. A `stat` is microseconds against a parse that is
milliseconds, it needs no client capability, and it behaves identically in every
editor — unlike `workspace/didChangeWatchedFiles`, which depends on the client
supporting dynamic registration.

If the reloaded file fails to parse, log the error and **keep the previous
config**. Reverting to defaults mid-session would silently change the thresholds
someone is working against.

### Unknown keys

`deny_unknown_fields`. A typo'd `treshold` that silently does nothing is a worse
failure than a loud one. The CLI reports it on stderr and exits 2; the server
logs it and falls back to built-ins rather than dying.

### `max` and `threshold` are independent

Neither key implies the other: `max` is the CI gate, `threshold` is the editor's
warning level with a built-in default of 11. Setting only `max = 15` therefore
leaves the editor warning from 11 while CI fails only above 15. This is
deliberate — it lets the editor nag before CI breaks — but it means the two
numbers diverge by default, and the README must say so plainly.

## Failure modes

### A file that does not parse

`analyze` returns `Err(ParseError)`. The server:

1. publishes nothing — the last good diagnostics stay on screen,
2. keeps answering hover from the cached functions,
3. sends `window/logMessage` type 4 (`Log`) with `{line}: {message}`,
   **only when the message differs** from the last one logged for that document.

A buffer is unparseable for most of the time someone is typing into it, so
clearing diagnostics on each failure would make them flash, and publishing the
syntax error would duplicate what rust-analyzer already reports. Type 4 rather
than type 1 because a mid-edit parse failure is the normal state of a file being
edited, not a fault. The dedupe stops a held keystroke inside an unclosed brace
from writing an identical line per character.

`window/logMessage` routes to the client's language-server log — `:LspLog` in
Neovim, the output channel in VS Code and Zed. stderr is left for a genuine
crash: it bypasses the client's log-level filtering, and its capture is a
convention rather than a guarantee.

### Malformed frames

A `Content-Length` that is missing, unparseable, zero, or over the 32 MB cap
skips the frame rather than killing the server. A body that is not JSON, or
carries no `method`, is skipped the same way. Ported from jscx unchanged, cap
included: without it a bogus length asks for an arbitrary allocation.

### Panics

`Cargo.toml` sets `panic = "abort"` in the release profile. A panic inside `syn`
therefore takes the server down with no possibility of `catch_unwind`. This
stays as it is: every editor client restarts a dead language server, and
switching to unwinding to swallow panics would hide real analyzer bugs behind a
silently degraded session. Documented, not fixed.

### Fixes relative to jscx

Three things are corrected rather than ported faithfully:

| jscx | Here |
| --- | --- |
| `params["textDocument"]["uri"].as_str().unwrap()` panics on a malformed notification (`src/lsp.rs:185`, `:192`) | `let…else`, skip the frame |
| `_ => {}` ignores unknown **requests**, so a client waiting on the `id` can hang | `-32601 MethodNotFound` for anything with an `id` |
| `didSave` reads a `text` that full sync never sends, and silently does nothing | an explicit no-op, with the reason in a comment |

## Testing

TDD throughout, as in the core plan.

**`src/lsp.rs` unit tests** — `parse_content_length` (wrong header name, zero,
over-cap, odd whitespace); `threshold_from_params`; `diagnostics_for` at both
severity boundaries (`threshold` → warning, `2 × threshold` → error) and against
a line containing non-ASCII text, since UTF-16 offsets are the classic LSP bug;
hover on a signature line, on a point line, on a blank line, and inside a nested
closure where the innermost function must win; and the parse-failure path —
diagnostics unchanged, hover still answering, one log line for a repeated
identical error.

**`src/config.rs` unit tests** — each precedence layer; an unknown key erroring;
walk-up discovery finding a parent directory's file; a mid-session reload
keeping the old config when the new file does not parse.

**`tests/lsp.rs`, end to end** — spawn the built binary with `--lsp`, write
genuinely framed `initialize` and `didOpen` messages to its stdin, parse the
framed responses off its stdout. jscx has no test at this layer, and framing is
where hand-rolled servers actually break. Document text comes from `fixtures/`,
whose scores are hand-checked in `tests/fixtures.rs`, so the expected
diagnostics follow from numbers already asserted elsewhere.

## Editor setup

A README section, not packaged extensions.

**Neovim** and **Helix** attach an arbitrary stdio server from plain config —
`vim.lsp.config` with `cmd = { "rustcx", "--lsp" }`, or a `[language-server]`
block in `languages.toml`. Both are a handful of lines and both are documented.

**VS Code** and **Zed** cannot: registering a new language server binary
requires a thin extension in each. The README says so rather than shipping
config that does not work. Those extensions stay out of scope.

## Limitations

- Everything the analyzer cannot see, the server cannot either: macro bodies are
  opaque, `#[cfg]`-disabled code is scored anyway, and a file that does not parse
  yields nothing new. The core spec's limitations section applies verbatim.
- Hover answers only on a function's signature line and its decision-point
  lines.
- Diagnostics and hover both reflect the last successful parse, so they lag a
  buffer that is mid-edit.
- `max` and `threshold` diverge unless both are set.
- One document is analyzed at a time, on the message loop's thread. A single
  very large file blocks the loop for the length of its parse.

## Build order

1. `src/config.rs` — `FileConfig`, discovery, precedence, reload. Testable with no
   protocol involved.
2. `--lsp` flag and `src/lsp.rs` framing, dispatch, `initialize`/`shutdown`/
   `exit`. Verified by `tests/lsp.rs` before any feature exists.
3. Diagnostics.
4. Hover.
5. Parse-failure handling and `window/logMessage`.
6. CLI reads `rustcx.toml`.
7. README: editor setup, configuration, the `max`/`threshold` caveat.

## Out of scope

- The Zed, JetBrains, VS Code, and Neovim **extensions** as packaged artifacts.
- Code lens, inlay hints, code actions, `textDocument/documentSymbol`.
- Workspace-wide diagnostics. Only open documents are analyzed.
- `workspace/didChangeConfiguration`. `rustcx.toml` plus `initializationOptions`
  covers configuration; editor-settings changes need a server restart.
- Incremental `textDocumentSync`.
- Cognitive complexity, or any second metric.
