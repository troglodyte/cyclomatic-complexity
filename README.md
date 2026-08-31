# rustcx

Cyclomatic complexity for Rust functions. One binary walks a source tree, parses
every file with `syn`, and prints one row per function: score, line, kind, name.
It gates CI through `--max` and points at what to refactor through `--top` and
`--explain`. It is a sibling to [`jscx`](https://github.com/troglodyte/jscx), which
does the same job for JavaScript and TypeScript, and whose CLI, output format,
and exit codes it copies so the two are interchangeable in a build script.

```console
$ rustcx --no-tests --min 5 src/
src/analyze.rs
    CC   Line  Kind         Name
     5     39  method       FnKind::label
     5    243  method       Walker::visit_local

src/lsp.rs
    CC   Line  Kind         Name
     5    116  method       Server::update
     7    233  fn           read_message
     5    261  method       Server::request
     6    287  method       Server::notification
     5    327  fn           run

src/main.rs
    CC   Line  Kind         Name
     8    127  fn           parse_args
     5    229  fn           write_grouped
     5    338  fn           collect
     5    365  fn           select
     5    436  fn           run
     7    572  fn           main

13 functions in 3 files · mean 5.6 · worst 8 (parse_args at src/main.rs:127)
```

## Install

```bash
cargo install --path .
```

## Usage

```
rustcx [OPTIONS] <PATH>...     files or directories (recursive)
rustcx [OPTIONS] -             read from stdin
```

| Option | Effect |
| --- | --- |
| `-m, --min <N>` | only report functions scoring N or higher (default 1) |
| `-M, --max <N>` | exit 1 if any function scores above N — for CI |
| `-t, --top <N>` | show only the N worst functions |
| `-s, --sort` | sort by score, highest first (one flat table) |
| `--no-header` | do not print the table header |
| `-j, --json` | machine-readable output |
| `-e, --explain` | list the decision points behind each score |
| `--question` | count `?` as a branch (off by default) |
| `--no-logical` | don't count `&&`, `\|\|` |
| `--no-tests` | skip `#[test]` functions and `#[cfg(test)]` modules |
| `--lsp` | run as a Language Server over stdio |
| `-h, --help` | print help |
| `-V, --version` | print version |

Directories are walked recursively for `.rs` files, skipping `target`, `.git`,
`.hg`, `.svn`, and `.jj`. A file named explicitly on the command line is read
whatever its extension.

`--sort` orders rows by score across the whole run, which leaves them
interleaved between files — so grouping by file would reprint a file's header
every time its rows resurfaced. Sorted output is one flat table instead, each
row carrying its own location, which is what jscx does:

```console
$ rustcx --sort --min 4 fixtures/
    CC  Location                 Kind         Name
     8  fixtures/patterns.rs:16  fn           describe
     5  fixtures/simple.rs:9     fn           branchy
     4  fixtures/patterns.rs:27  fn           unwrap_pair

3 functions in 2 files · mean 5.7 · worst 8 (describe at fixtures/patterns.rs:16)
```

Note that `--top` on its own does not imply `--sort` here, as it does in jscx:
it picks the N worst and then restores source order, so the table still reads
file by file. Add `--sort` to see them ranked.

## As a CI gate

```bash
rustcx --max 10 src/
```

| Exit code | Meaning |
| --- | --- |
| 0 | clean |
| 1 | a function scored above `--max` |
| 2 | a path could not be read **or parsed** |

A file `syn` rejects prints `path:line: parse error: <message>` to stderr,
contributes no rows, and does not stop the scan — the rest of the tree is still
analyzed and the summary still prints, and the run exits 2 at the end.

## Finding what to refactor first

`--top N` keeps the N worst functions and `--explain` lists the decision points
behind each score:

```console
$ rustcx --explain --min 4 fixtures/patterns.rs
fixtures/patterns.rs
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

2 functions in 1 file · mean 6.0 · worst 8 (describe at fixtures/patterns.rs:16)
```

Every listed line is one point, so the score is always `1 + the number of
explain lines`. A score and its explanation can never disagree.

## In your editor

`rustcx --lsp` speaks LSP over stdio. It publishes a warning for every function
at or above a threshold — an error at twice it — and answers hover over a
function with the decision points behind its score.

Hover works on a function's signature line and on any line it was charged a
point for. Those are the branchy lines, which are the ones worth hovering; a
blank line inside a function answers nothing.

### Neovim

Neovim 0.11 or newer, for `vim.lsp.config`:

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

## Scoring

Every function starts at 1 and gains a point per decision point.

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

Three consequences worth knowing:

- **Nested functions and closures are scored independently.** A closure gets its
  own row, indented under its parent, and the parent is not charged for the
  branches inside it. A closure is named after whatever it attaches to — its
  `let` binding, the method or function it is passed to, or the struct field it
  initializes — falling back to `closure`.
- **`?` is free by default.** Each `?` is a conditional early return, but
  idiomatic Rust is saturated with them, and a function whose only branching is
  error propagation has no branching logic of its own. `return` scores 0, so `?`
  follows it. Turn it on with `--question`.
- **A trailing catch-all `match` arm is free**, the way `default` is in a
  `switch`. A guarded arm (`_ if c =>`) is not a catch-all: it can be reached and
  still fail, so it is a real decision.

## Comparison to jscx

The rules follow ESLint's `complexity` rule wherever the two languages line up —
`if`, loops, logical operators, and `switch` cases against `match` arms — so
scores from the two tools are directly comparable across a mixed codebase.

## How it works

`syn` parses each file to a full AST, and a `syn::visit::Visit` walker descends
it holding a stack of in-progress functions. Every decision point pushes a
`Point` onto the innermost frame, which is what makes nested functions score
independently, and a function's complexity is derived as `1 + points.len()` —
never accumulated separately.

One surprise worth naming: `proc-macro2`'s span table is a thread-local that
retains the text of every file ever parsed. Spans are resolved to line numbers
during the walk, and `invalidate_current_thread_spans()` is called after each
file, so memory stays flat across a large tree.

## Limitations

- **Macro bodies are opaque.** `syn` keeps a macro invocation's contents as an
  unparsed token stream, so branches inside `assert!`, `matches!`, `write!`, or
  any custom macro are not counted, and `macro_rules!` definitions are not scored
  at all. This is the largest single blind spot.
- **A file that does not parse yields no rows.** Nightly-only syntax and syntax
  errors take out the whole file — the granularity is one file, not the run.
- **Branches outside a function are not scored** — `const` and `static`
  initializers, array lengths.
- **`#[cfg]`-disabled code is scored anyway.** `syn` does not evaluate `cfg`, so
  a platform-specific branch is counted on every platform. `--no-tests` handles
  the common case, and recognises only the bare `#[cfg(test)]` form — not a
  compound predicate like `#[cfg(all(test, feature = "x"))]`.
- **Cyclomatic complexity is a blunt instrument.** It counts branches, not how
  tangled they are. A flat 20-arm `match` scores 21 and is usually fine; a nested
  8 is often worse. Read it as a place to look, not a verdict.
- **In the editor, hover and diagnostics reflect the last successful parse**, so
  both lag a buffer that is mid-edit, and hover answers only on a function's
  signature line and its decision-point lines.
- **The server analyzes one document at a time on its message loop.** A single
  very large file blocks the loop for the length of its parse.
- **A panic takes the server down.** The release profile sets `panic = "abort"`,
  so a panic inside `syn` ends the process rather than being caught. Editors
  restart a dead language server; the alternative would hide real analyzer bugs
  behind a silently degraded session.

## Development

```bash
cargo test                        # unit, CLI, and fixture tests
cargo run -- fixtures/ --explain  # see the analyzer's reasoning
cargo clippy -- -D warnings
```

rustcx gates itself at 10:

```bash
cargo run -- --max 10 --no-tests src/
```

`fixtures/` holds hand-checked sample files whose expected scores are asserted in
`tests/fixtures.rs`; `fixtures/syntax.rs` in particular covers the lexical traps
(raw strings, nested block comments, lifetimes against char literals, generics
against shift operators) that motivated parsing to a real AST.
