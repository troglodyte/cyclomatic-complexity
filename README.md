# rustcx

Cyclomatic complexity for Rust functions. One binary walks a source tree, parses
every file with `syn`, and prints one row per function: score, line, kind, name.
It gates CI through `--max` and points at what to refactor through `--top` and
`--explain`. It is a sibling to [`jscx`](https://github.com/trog/jscx), which
does the same job for JavaScript and TypeScript, and whose CLI, output format,
and exit codes it copies so the two are interchangeable in a build script.

```console
$ rustcx --no-tests --min 5 src/
src/analyze.rs
    CC   Line  Kind         Name
     5     39  method       FnKind::label
     5    243  method       Walker::visit_local

src/main.rs
    CC   Line  Kind         Name
    27     65  fn           parse_args
     9    148  fn           write_table
     5    214  fn           collect
     5    241  fn           select
    14    312  fn           run

7 functions in 2 files · mean 10.0 · worst 27 (parse_args at src/main.rs:65)
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
| `-s, --sort` | sort by score, highest first |
| `--no-header` | do not print the table header |
| `-j, --json` | machine-readable output |
| `-e, --explain` | list the decision points behind each score |
| `--question` | count `?` as a branch (off by default) |
| `--no-logical` | don't count `&&`, `\|\|` |
| `--no-tests` | skip `#[test]` functions and `#[cfg(test)]` modules |
| `-h, --help` | print help |
| `-V, --version` | print version |

Directories are walked recursively for `.rs` files, skipping `target`, `.git`,
`.hg`, `.svn`, and `.jj`. A file named explicitly on the command line is read
whatever its extension.

## As a CI gate

```bash
rustcx --max 15 src/
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

## Development

```bash
cargo test                        # unit, CLI, and fixture tests
cargo run -- fixtures/ --explain  # see the analyzer's reasoning
cargo clippy -- -D warnings
```

`fixtures/` holds hand-checked sample files whose expected scores are asserted in
`tests/fixtures.rs`; `fixtures/syntax.rs` in particular covers the lexical traps
(raw strings, nested block comments, lifetimes against char literals, generics
against shift operators) that motivated parsing to a real AST.
