Cyclomatic complexity CLI and language server for JavaScript, TypeScript and Rust.

One binary, `cmplx`, walks a source tree and prints one row per function: score,
line, kind, name. It gates CI through `--max`, points at what to refactor
through `--top` and `--explain`, and runs as a language server (`--lsp`) for
editor diagnostics and hover. There is no Node toolchain and no external
parser process; JavaScript and TypeScript are scanned by a built-in lexer, Rust
is parsed with `syn`.

```console
$ cmplx --min 4 crates/cmplx-rust/fixtures/patterns.rs
crates/cmplx-rust/fixtures/patterns.rs
    CC   Line  Kind         Name
     8     16  fn           describe
     4     27  fn           unwrap_pair

2 functions in 1 file · mean 6.0 · worst 8 (describe at crates/cmplx-rust/fixtures/patterns.rs:16)
```

Columns are score, line, kind and name. Nested functions and closures are
indented under their parent.

## Install

```console
cargo install --path crates/cmplx            # cmplx: every language
cargo install --path crates/aliases/jscx     # jscx:  shortcut for cmplx --js
cargo install --path crates/aliases/rustcx   # rustcx: shortcut for cmplx --rust
```

Each is its own package, so installing an alias compiles only its own language.

## Usage

```
cmplx [OPTIONS] [--js|--rust] <PATH>...     files or directories (recursive)
cmplx [OPTIONS] --<LANG> -                  read from stdin
```

With no language flag, every file goes to the language that claims its
extension. With one or more language flags, only those languages' files are
scanned. Reading stdin needs exactly one language flag; `--ext <EXT>` picks the
extension to assume (it chooses JS versus TS/JSX lexing).

### Shared options

| Option | Effect |
| --- | --- |
| `-m, --min <N>` | only report functions scoring N or higher (default 1) |
| `-M, --max <N>` | exit 1 if any function scores above N, for CI |
| `-t, --top <N>` | show only the N worst functions |
| `-s, --sort` | sort by score, highest first (one flat table with a Location column) |
| `--no-header` | do not print the table header |
| `-j, --json` | machine-readable output |
| `-e, --explain` | list the decision points behind each score |
| `--ext <EXT>` | extension to assume for stdin |
| `--lsp` | run as a Language Server over stdio |
| `-h, --help` | print help |
| `-V, --version` | print version |

### JavaScript/TypeScript options (`--js`)

| Option | Effect |
| --- | --- |
| `--no-logical` | turn off counting `&&`, `\|\|`, `??` and their assignment forms |
| `--optional-chain` | count `?.` as a branch (off by default) |
| `--jsx`, `--no-jsx` | force JSX lexing on or off (default: on, except for `.ts`/`.mts`/`.cts`) |
| `--include-dts` | also scan `.d.ts`/`.d.mts`/`.d.cts` declaration files |

### Rust options (`--rust`)

| Option | Effect |
| --- | --- |
| `--no-logical` | turn off counting `&&` and `\|\|` |
| `--question` | count the `?` operator as a branch (off by default) |
| `--no-tests` | skip `#[test]` functions and `#[cfg(test)]` modules |

A language's own flags are accepted only when that language is active.

Directories are walked recursively, skipping `node_modules`, `dist`, `build`,
`coverage`, `.next`, `target`, `__pycache__`, `.venv`, `venv`, `vendor` and VCS
directories. A directory or entry that cannot be read is reported as
`path: <error>` and the rest of the tree is still scanned (exit 2). A file named explicitly on the command line is scored as the
active language when you pass a single language flag or use an alias; under bare
`cmplx` a file no language claims is an error.

### Exit codes

| Code | Meaning |
| --- | --- |
| 0 | clean |
| 1 | a function scored above `--max` |
| 2 | a path could not be read or parsed, or the arguments were invalid |

A Rust file that fails to parse prints `path:line: parse error: <message>` to
stderr and contributes no rows; the rest of the tree is still analyzed and the
run exits 2 at the end.

### As a CI gate

```console
$ cmplx --max 10 crates/cmplx-js/fixtures
...
14 functions in 2 files · mean 8.6 · worst 25 (applyPricingRules at crates/cmplx-js/fixtures/test.ts:71)
5 functions above the limit of 10
$ echo $?
1
```

### Finding what to refactor first

`--explain` prints each decision point that contributed, so a score is a list
rather than a number you have to hunt for. Every listed line is one point: the
score is always `1 + the number of explain lines`.

```console
$ cmplx --explain --min 4 crates/cmplx-rust/fixtures/patterns.rs
crates/cmplx-rust/fixtures/patterns.rs
    CC   Line  Kind         Name
     8     16  fn           describe
           18  match arm
           18  guard
           18  &&
           19  match arm
           20  match arm
           20  pattern alternative
           21  match arm
     4     27  fn           unwrap_pair
           28  let else
           31  if let
           31  pattern alternative

2 functions in 1 file · mean 6.0 · worst 8 (describe at crates/cmplx-rust/fixtures/patterns.rs:16)
```

`--top N` keeps the N worst functions but restores source order; add `--sort`
to see them ranked. `--sort` produces one flat table, because ranking
interleaves files:

```console
$ cmplx --sort --min 4 crates/cmplx-rust/fixtures
    CC  Location                                   Kind         Name
     8  crates/cmplx-rust/fixtures/patterns.rs:16  fn           describe
     5  crates/cmplx-rust/fixtures/simple.rs:9     fn           branchy
     4  crates/cmplx-rust/fixtures/patterns.rs:27  fn           unwrap_pair

3 functions in 2 files · mean 5.7 · worst 8 (describe at crates/cmplx-rust/fixtures/patterns.rs:16)
```

## Scoring

Each function starts at 1 and gains a point for every decision point. Nested
functions are scored independently: a callback or closure gets its own row and
its parent is not charged for its branches, so the number stays attached to the
unit you would actually extract.

### JavaScript and TypeScript

These are the same rules as ESLint's [`complexity`][eslint] rule, so scores are
directly comparable, and the test suite checks the examples from that rule's
documentation.

| Construct | Points |
| --- | --- |
| `if` | +1 each (`else` adds nothing; `else if` is the nested `if`) |
| `for`, `for..in`, `for..of`, `while`, `do..while` | +1 each |
| `case` | +1 each (`default` adds nothing) |
| `catch` | +1 |
| `&&`, `\|\|`, `??`, `&&=`, `\|\|=`, `??=` | +1 each, unless `--no-logical` |
| ternary `?:` | +1 each |
| `?.` | 0 unless `--optional-chain` |

Parameter defaults count toward the enclosing function: `function f(a = b || c)`
scores 2, because that branch runs on every call to `f`.

[eslint]: https://eslint.org/docs/latest/rules/complexity

### Rust

Rust has no external reference tool; these are the rules `rustcx` established.
They follow ESLint's `complexity` rule wherever the two languages line up (`if`,
loops, logical operators, and `switch` cases against `match` arms), so scores are
comparable across a mixed codebase.

| Construct | Points |
| --- | --- |
| `if`, `if let` | +1 each |
| `else` | +0 (an `else if` is a nested `if`, so it scores +1 as an `if`) |
| `match` arm | +1 each; a final catch-all arm scores +0 |
| pattern alternative | +1 per alternative beyond the first, counted recursively |
| match guard | +1 |
| `while`, `while let` | +1 |
| `for` | +1 |
| `loop` | +1 |
| `&&`, `\|\|` | +1 each, unless `--no-logical` |
| `let … else` | +1 |
| `?` | +0, or +1 with `--question` |

Scoring nothing: `return`, `break`, `continue`, `await`, `&` and `|` (not
short-circuiting), `?` by default, and the `try` block expression.

- **`?` is free by default.** Each `?` is a conditional early return, but
  idiomatic Rust is saturated with them, and a function whose only branching is
  error propagation has no branching logic of its own.
- **A trailing catch-all `match` arm is free**, the way `default` is in a
  `switch`. A guarded arm (`_ if c =>`) is not a catch-all: it can be reached
  and still fail, so it is a real decision.
- **A closure is named** after whatever it attaches to (its `let` binding, the
  function it is passed to, or the struct field it initializes), falling back to
  `closure`.

## Configuration

`cmplx.toml`, found by walking up from the path being scanned: from a
directory argument itself, or from the directory holding a file argument.
Stdin, and a run with no path, start from the working directory; `--lsp`
starts from the workspace root. With several paths, the file found for the
first path applies to all of them, and if a later path would have found a
different file (or none) one warning on stderr says so. Layering, lowest to
highest: built-in defaults, `cmplx.toml`, editor `initializationOptions`,
command-line flags.

```toml
max = 15          # CLI --max default
threshold = 11    # LSP: warn at N, error at 2N
logical = true    # default for every language that supports it

[js]
optional-chain = false
jsx = true

[rust]
question = false
tests = true
```

A per-language table may override a shared key (`[rust] logical = false`). An
unknown key is an error, not a shrug, for `cmplx`. An alias such as `jscx`
cannot tell a typo from a key that belongs to a language it does not include,
so it ignores the key and prints a warning naming it, such as
``jscx: /repo/cmplx.toml: ignoring unknown key `question` ``. The language
server writes the same warning to its log. The server re-reads the file it found at startup whenever that
file changes, so a new threshold applies without a restart. A config file
created after the server started is not picked up until it restarts.

`rustcx.toml` is still read as a fallback when no `cmplx.toml` is found, and
only for Rust. Its keys are top-level (`max`, `threshold`, `question`,
`logical`, `tests`).

`max` and `threshold` are independent: `max` gates CI, `threshold` is the
editor's warning level (default 11). Set both if you want them identical.
`threshold` must be at least 1: `threshold = 0` in the file is an error, and in
`initializationOptions` it is logged and ignored, keeping the other options.

## Aliases

`jscx` is `cmplx --js` and `rustcx` is `cmplx --rust`, with the alias name in
`--help` and `--version`. Output is identical to `cmplx` run with that language
flag, and `jscx --lsp` / `rustcx --lsp` serve only their language, so existing
editor and build-script setups keep working.

## Language server

`cmplx --lsp` speaks LSP over stdio. It publishes a warning for every function
at or above the threshold (default 11) and an error at twice it, and answers
hover on a function's signature line, or on any line it was charged a point for,
with the decision points behind the score. It re-reads its `cmplx.toml` on change and
skips files the active languages skip (such as `.d.ts`). A buffer that does not
parse keeps its last diagnostics, and the failure is written to the server log.

`initializationOptions` take the shape of `cmplx.toml`: shared keys at the top
level and per-language tables, e.g. `{ "threshold": 15, "rust": { "tests": false } }`.
A flat key that belongs to exactly one active language is accepted as that
language's key.

Each editor integration lives under `extensions/` with its own README:

| Editor | Directory | Install |
| --- | --- | --- |
| Neovim | [`extensions/nvim`](extensions/nvim/README.md) | put the directory on the runtimepath, then `vim.lsp.enable("cmplx")` |
| Zed | [`extensions/zed`](extensions/zed/README.md) | *Extensions: Install Dev Extension* |
| JetBrains | [`extensions/jetbrains`](extensions/jetbrains/README.md) | `./gradlew buildPlugin`, then install the zip from disk |
| VS Code | [`extensions/vscode`](extensions/vscode/README.md) | package the extension, then `code --install-extension` |

Helix needs no extension:

```toml
[language-server.cmplx]
command = "cmplx"
args = ["--lsp"]

[[language]]
name = "rust"
language-servers = ["rust-analyzer", "cmplx"]
```

## JSON output

`--json` prints one line, the same shape from every binary:

```json
{
  "summary": { "functions": 1, "files": 1, "mean": 3.0,
               "limit": null, "overLimit": 0,
               "worst": { "file": "a.rs", "name": "f", "line": 1, "complexity": 3 } },
  "functions": [
    { "file": "a.rs", "language": "rust", "name": "f", "kind": "fn",
      "line": 1, "depth": 0, "complexity": 3,
      "points": [ { "line": 1, "branch": "if" } ] }
  ]
}
```

`limit` is the `--max` value or `null`; `overLimit` counts functions above it.
A function scoring above 100 also carries `"capped": true` (see [Limits](#limits)).
`kind` is a JSON id (`trait_fn`) where the table shows a label (`trait fn`).

## Limitations

- **Cyclomatic complexity is a blunt instrument.** It counts branches, not how
  tangled they are. A flat 20-arm `match` or `switch` scores 21 and is usually
  fine; a nested 8 is often worse. Read it as a place to look, not a verdict.
- **JS/TS is token-level, not a parser.** A bare class field with a
  function-type annotation and no modifier (`onChange: (v: string) => void;`) is
  read as an arrow function, and function types in unusual positions (such as
  explicit call-site type arguments) can add one extra row scoring 1. Filter
  with `--min 2` if that is noisy. Decorators do not affect scoring.
- **Rust macro bodies are opaque.** Branches inside `assert!`, `matches!`,
  `write!` or any custom macro are not counted, and `macro_rules!` definitions
  are not scored.
- **A Rust file that does not parse yields no rows**, and `#[cfg]`-disabled code
  is scored anyway. `--no-tests` recognises only the bare `#[cfg(test)]` form.
- **Branches outside a function are not scored** (`const`/`static` initializers,
  array lengths).
- **The server analyzes one document at a time.** An analyzer crash costs one
  document (see [Crash protection](#limits)); a panic anywhere else ends the
  process (`panic = "abort"`). Some editors restart a dead language server;
  others, such as Neovim, leave it stopped until you restart it.

## Limits

Inputs past these caps are refused rather than risk a crash or unbounded
memory. Each refusal is a per-file problem: a warning on stderr naming the
file, the rest of the run continues, and the run exits 2.

| Cap | Value | At the cap |
| --- | --- | --- |
| Nesting depth, every language | 100 levels of `(`, `[` or `{` | `path:line: nesting deeper than 100 levels; this code needs restructuring`, at the line where the depth is first exceeded. In the language server, one warning on that line, with the same text, instead of scores. |
| Directory depth | 64 levels below each path given | `path: directory nesting deeper than 64; not scanned`. The directory is skipped and the rest of the tree is scanned. |
| File size | 10 MB (10,485,760 bytes), files and stdin | `path: larger than 10 MB; not scored` (`<stdin>` for stdin). The language server publishes no diagnostics for a larger document and logs once. |

The depth is counted before a file is analyzed, in one iterative pass: lexing
is linear in the file whatever its depth, and only the counting stops at the
first line past the cap. Brackets inside strings, characters and comments do
not count, nor do those in JS/TS template text, regular expressions and JSX
text. Rust needs the cap because `syn` parses recursively; Rust files are also
parsed on their own 16 MB thread. JS/TS is held to the same cap so that the
limit does not depend on the language.

**Crash protection.** Files are analyzed in a worker: a second copy of the same
binary, started once per run (or once per language-server session) and fed one
file at a time. A file that crashes the worker fails alone, whatever the cause
(a shape the nesting cap does not catch, such as thousands of nested closures,
can still overflow `syn`'s stack). The next file starts a fresh worker. The
CLI prints, after `path: ` (with the file's language in place of `Rust`), and
exits 2 as for any other per-file failure:

| Case | Message |
| --- | --- |
| Stack overflow | `not scored: the Rust analyzer crashed (stack overflow); the file is probably nested too deeply to analyze` |
| No answer within 10 s | `not scored: the Rust analyzer took longer than 10 s` (the worker is killed and replaced; this is not a restart) |
| No answer within 2 s, after 10 timeouts in one run | `not scored: the Rust analyzer took longer than 2 s (cut from 10 s after 10 timeouts in this run)` |
| Any other crash | `not scored: the Rust analyzer crashed (exit code N)` or `(signal NAME)`, then the worker's first stderr line, cut to 200 characters |
| An answer that is not a response | `not scored: the Rust analyzer sent a bad response: …` |
| A restart that cannot be started | `not scored: the Rust analyzer could not be restarted: <error>` (counts as a restart) |
| More than 10 restarts in one run | `not scored: the analyzer kept crashing; gave up after 10 restarts`, for every remaining file |

The language server keeps one worker for the session and restarts it on the
next document after a crash. It shows the crash as one warning on line 0 of
that document; other documents are unaffected. It remembers the exact text
that crashed and shows the same warning, without trying again, until the text
or the settings change (a timeout is tried again after a minute). It restarts at most 10 times in any 60 seconds (replacing a worker killed for a timeout does not count); past that, documents
get `not scored: the analyzer kept crashing; restarting is paused for a minute`
until the minute is up. If the first worker cannot be started at all, the
binary prints (or logs) `running without crash protection: <error>` once and
analyzes in its own process for the rest of the run, where a crash ends the
run. On macOS, a worker stuck in an infinite loop can outlive a parent that
was killed with SIGKILL; it then runs until killed.

Scores above 100 show as `100+` in the table, in language-server diagnostics
and in hover. JSON keeps the real number in `complexity` and adds
`"capped": true` to those functions only. `--max`, `--min`, `--top`,
`--sort` and `overLimit` all use the real number.

## Development

The workspace has `crates/cmplx-core` (model, CLI, config, output, LSP, no
language knowledge), `crates/cmplx-js` and `crates/cmplx-rust` (the analyzers),
and the binaries `crates/cmplx` and `crates/aliases/*`.

```console
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p cmplx -- --explain crates/cmplx-js/fixtures
```

The tests need [`jq`](https://jqlang.org) on `PATH`: `hook_compat` runs the
Claude Code hook's jq filter over the binaries' `--json` output, and fails
loudly (`install jq to run hook_compat`) rather than skipping when jq is missing.

`tests/golden/` holds output recorded from the pre-merge tools; it is historical
and is never regenerated (see its README). The fixtures under
`crates/cmplx-*/fixtures/` are hand-checked samples whose expected scores the
analyzer tests assert.
