# rustcx — design

Cyclomatic complexity for Rust functions. A sibling to
[`jscx`](https://github.com/trog/jscx), which does the same job for JavaScript
and TypeScript, and whose CLI, output format, and exit codes this tool copies so
the two are interchangeable in a build script.

## Goal

A single binary that walks Rust source and prints one row per function: score,
line, kind, name. It gates CI through `--max`, and it points at what to refactor
through `--top` and `--explain`.

Scope for this spec is the **core CLI only**. The LSP server and the Zed,
JetBrains, and Neovim extensions that `jscx` ships are deliberately out of scope;
the analyzer's public API is shaped so they can be added later without changing
it.

## Approach

`jscx` hand-rolls a lexer because the JavaScript alternatives (`swc`, `oxc`,
`tree-sitter`) are large dependencies. Rust inverts that trade-off. `syn` is
small, pure Rust, present in every developer's cargo cache, and — with
`default-features = false` — pulls in only `proc-macro2` and `unicode-ident`.
Meanwhile Rust's lexical surface is harder to hand-scan than JavaScript's: raw
strings (`r#"…"#`), nested block comments, `'a` lifetimes against `'a'` char
literals, `<>` against shift operators, turbofish, raw identifiers.

So rustcx parses to a real AST and walks it with `syn::visit::Visit`. Scoring
becomes an exhaustive match over expression kinds rather than a pile of
heuristics, and the ambiguities that fill jscx's Limitations section do not
arise. The cost is that a file which fails to parse yields nothing rather than
degrading gracefully; see [Failure modes](#failure-modes).

## Module layout

```
src/lib.rs      pub mod analyze;
src/analyze.rs  the Visit walker: function discovery, nesting, decision counting
src/main.rs     argument parsing, file walk, table and JSON output
```

Three files against jscx's four — `lexer.rs` has no counterpart here, and
`lsp.rs` is out of scope. Argument parsing is hand-rolled in `main.rs`, matching
jscx; there is no `clap` dependency.

If `analyze.rs` passes roughly 500 lines, split the scoring rules into
`src/score.rs` and leave discovery and nesting in `analyze.rs`.

### Public API

```rust
pub struct Options {
    pub logical: bool,    // count && and ||            (default true)
    pub question: bool,   // count ?                    (default false)
    pub tests: bool,      // score #[test] / #[cfg(test)] items (default true)
}

pub enum FnKind { Fn, Method, TraitFn, Closure }

pub struct Point { pub line: u32, pub branch: &'static str }

pub struct Func {
    pub name: String,
    pub kind: FnKind,
    pub line: u32,
    pub complexity: u32,
    pub depth: u32,
    pub points: Vec<Point>,
}

pub struct ParseError { pub line: u32, pub message: String }

pub fn analyze(src: &str, opts: Options) -> Result<Vec<Func>, ParseError>;
```

`ParseError` carries the line and message from `syn::Error`, already resolved to
owned data so it survives span invalidation like `Func` does.

`Func` matches jscx's field for field, so the table writer, the JSON writer, and
a future `lsp.rs` port across unchanged. Every field is owned data — no
borrowing from the source, and no `syn` or `proc_macro2` type appears in the
signature. That is a hard requirement, not a style preference; see
[Span lifetime](#span-lifetime-invariant).

## Dependencies

```toml
[dependencies]
syn = { version = "3", default-features = false, features = ["full", "parsing", "visit", "clone-impls"] }
proc-macro2 = { version = "1", features = ["span-locations"] }
colored = "2.0"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
```

Disabling syn's default `printing` and `proc-macro` features drops `quote`.
Function names come from `Signature.ident` directly, so nothing needs
`ToTokens`. The added dependency tree is **syn, proc-macro2, unicode-ident**.

Pin syn 3 specifically. Its AST differs from syn 2 in two places this design
depends on: match guards moved into the pattern as `Pat::Guard`, and `let…else`
lives at `LocalInit.diverge`.

Release profile copies jscx's: `opt-level = 3`, `lto = true`,
`codegen-units = 1`, `panic = "abort"`.

## Span lifetime invariant

`proc-macro2` resolves a `Span` to a line and column through a thread-local
source map. That map is a `Vec<FileInfo>`, each entry holding the **full source
text** of a parsed file plus a line index and a byte-offset `BTreeMap`. It only
ever grows. Parsing a large repository in one thread would retain every file's
text in memory, and the span coordinate space is a `u32` that advances past the
end of each file.

`proc_macro2::extra::invalidate_current_thread_spans()` clears it, and exists
for exactly this use case — its own documentation example walks a crates.io
tarball. rustcx calls it after each file.

That imposes an invariant on `analyze`:

> Resolve every span to a `u32` line **during** the walk. Never retain a `Span`,
> or any `syn` node, past the end of `analyze` for that file.

Violating it does not fail to compile. It produces wrong line numbers, or trips
an internal assertion in `offset_line_column`. `Func` holding only owned data
makes the invariant fall out naturally, but it is load-bearing, so:

- `analyze` takes `&str` and returns owned `Func`s. The AST is dropped before it
  returns.
- The call to `invalidate_current_thread_spans()` lives in `main.rs`'s per-file
  loop, immediately after `analyze` returns, with a comment pointing here.
- An integration test scans a multi-file fixture directory and asserts line
  numbers are correct for every file, not just the first.

## Scoring

Every function starts at 1 and gains a point per decision point.

| Construct | Points | syn node |
| --- | --- | --- |
| `if`, `if let` | +1 each | `ExprIf` |
| `else` | +0 (an `else if` is a nested `ExprIf`, so it scores +1 as an `if`) | `ExprIf.else_branch` |
| `match` arm | +1 each; a final catch-all arm scores +0 | `ExprMatch.arms` |
| pattern alternative | +1 per alternative beyond the first, counted recursively | `Pat::Or.cases` |
| match guard | +1 | `Pat::Guard` |
| `while`, `while let` | +1 | `ExprWhile` |
| `for` | +1 | `ExprForLoop` |
| `loop` | +1 | `ExprLoop` |
| `&&`, `\|\|` | +1 each, unless `--no-logical` | `BinOp::And`, `BinOp::Or` |
| `let … else` | +1 | `LocalInit.diverge` |
| `?` | +0, or +1 with `--question` | `ExprTry` |

Scoring nothing: `return`, `break`, `continue`, `await`, `&`, `|` (not
short-circuiting), `?` by default, and the `try` block expression.

### Rationale for the contested rules

**`?` defaults to 0.** By McCabe's definition each `?` is a conditional early
return and does create a path, so counting it is defensible. But idiomatic Rust
is saturated with `?`, and a function whose only "branching" is error
propagation has no branching logic of its own:

```rust
fn load(p: &Path) -> Result<Cfg> {
    let s = fs::read_to_string(p)?;     // default: +0    --question: +1
    let c: Cfg = toml::from_str(&s)?;   // default: +0    --question: +1
    c.validate()?;                      // default: +0    --question: +1
    Ok(c)
}                                       // CC 1           CC 4
```

Counting it by default would push every score across a Rust codebase upward and
make `--max` thresholds incomparable with jscx's. The flag mirrors jscx's
`--optional-chain`, which makes the same call for `?.`.

The rule is also consistent: a plain `return` scores 0 here exactly as it does in
ESLint's `complexity` rule, and `?` is a conditional `return`.

**A `match` arm is +1, a catch-all is +0.** This is the direct analog of
ESLint's `switch`, where each `case` is +1 and `default` is +0. It keeps rustcx's
numbers comparable to jscx's.

An arm is a catch-all when *all* of the following hold. Anything else is +1:

- it is the **last** arm of the match, and
- its pattern is `_` (`Pat::Wild`) or a bare binding (`Pat::Ident` with no
  subpattern), and
- it has no guard.

The guard clause matters: `_ if slow_path() => …` can be reached and still fail,
so it is a genuine decision and scores +1, plus +1 for the guard itself.

**Alternatives and guards count.** A `|` alternative is the analog of JavaScript
fall-through (`case A: case B:`), which ESLint scores 2. A guard is a second
decision layered on the pattern. Both are branches a reader has to follow, so
both appear in the number. Counting arms alone would score a heavily-guarded
four-arm match identically to a trivial one.

Alternatives are counted recursively over the whole pattern, so `Some(A | B)`
contributes +1 — not just top-level `Pat::Or` nodes.

**These rules apply uniformly to every refutable pattern position** — `match`
arms, `if let`, `while let`, and `let … else` — not only inside `match`.

### Worked example

```rust
fn handle(&mut self, ev: Event) -> Result<()> {         // 1
    let Some(w) = self.window else { return Ok(()) };   // +1  let-else
    match ev {
        Key(k) if k.ctrl => self.command(k),            // +1 arm, +1 guard
        Click | Drag => self.pointer(ev),               // +1 arm, +1 alternative
        Resize(a, b) if a > 0 && b > 0 => self.fit(),   // +1 arm, +1 guard, +1 &&
        Resize(..) => Ok(()),                           // +1 arm
        _ => self.ignore(),                             // +0 catch-all
    }?;                                                 // +0 (default)
    Ok(())
}                                                       // CC 10
```

## Functions, naming, nesting

### What is scored

| Kind | Source | Column label |
| --- | --- | --- |
| `FnKind::Fn` | `ItemFn`, including functions nested in a function body | `fn` |
| `FnKind::Method` | `ImplItemFn`, in inherent and trait impls alike | `method` |
| `FnKind::TraitFn` | `TraitItemFn` **with** a default body | `trait fn` |
| `FnKind::Closure` | `ExprClosure` | `closure` |

A `TraitItemFn` with no default body is a signature, not a function, and produces
no row. `async` and `unsafe` do not affect scoring or kind. `macro_rules!`
definitions produce no rows.

### Independent scoring

A nested function or closure is scored as its own row, and its decision points
are **not** charged to the enclosing function. This matches jscx, which matches
ESLint. A function containing five closures is not charged for their branches;
each closure gets a row. It keeps the number attached to the unit you would
actually extract.

Implementation: the walker holds a stack of in-progress functions. A decision
point is credited to the innermost frame only. Entering a function-like node
pushes a frame with `depth = stack.len()`; leaving it pops.

Decision points encountered with an **empty** stack — in a `const` or `static`
initializer, or an array-length expression — are dropped. This is a documented
limitation.

### Names

- Free function: the bare identifier — `parse_args`.
- Impl item: `Type::method` — `Walker::visit_expr`. The type comes from the
  enclosing `ItemImpl.self_ty`; for a trait impl the trait name is omitted as
  noise, since the file and line already locate the row.
- Trait default method: `Trait::method`.
- Module paths are **not** prefixed. The file path is already printed above the
  table.
- Closure: named after what it is attached to, the way jscx names arrows, in
  descending preference:
  1. the `let` binding it initializes — `let render = |x| …` becomes `render`
  2. the struct field it initializes in a struct literal
  3. the callee it is passed to — `.unwrap_or_else(|| …)` becomes
     `unwrap_or_else`
  4. otherwise, `closure`

### Output nesting

Rows are printed in source order, with `depth` rendered as indentation, so
closures appear indented under the function containing them — identical to
jscx's treatment of nested functions.

## CLI

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
| `-h, --help` | usage |
| `-V, --version` | version |

jscx's `--jsx`/`--no-jsx`, `--include-dts` and `--ext` have no Rust counterpart
and are dropped; stdin is always Rust.

`--no-tests` is the one flag with no jscx counterpart. Rust keeps unit tests
inline in the file under test, and `syn` does not evaluate `cfg`, so a
`#[cfg(test)]` module is walked and scored like any other code. The flag
suppresses rows for functions carrying `#[test]` and for everything inside a
module carrying `#[cfg(test)]`.

### File discovery

Directories are walked recursively for `*.rs`. Skipped: `target`, and the VCS
directories `.git`, `.hg`, `.svn`, `.jj`. Files named explicitly on the command
line are read regardless of extension or location.

### Output

The table matches jscx exactly — columns score, line, kind, name, a blank line
between files, and a closing summary:

```console
$ rustcx src/
src/analyze.rs
    CC   Line  Kind         Name
    12     84  method       Walker::visit_expr
     3    141  closure        unwrap_or_else
     7    203  fn           score_pattern

42 functions in 7 files · mean 3.1 · worst 12 (Walker::visit_expr at src/analyze.rs:84)
```

`--json` emits the same rows and summary through serde.

## Failure modes

Exit codes match jscx:

| Code | Meaning |
| --- | --- |
| 0 | clean |
| 1 | a function exceeded `--max` |
| 2 | a path could not be read **or parsed** |

A file that `syn` rejects — a syntax error, or nightly-only syntax — prints
`path:line: parse error: <message>` to stderr, contributes no rows, and does not
stop the scan. Other files are still analyzed and the summary still prints; the
run exits 2 at the end.

The granularity of the all-or-nothing parse is one file, not the whole run. This
is the main cost of the AST approach over jscx's token scanner and is documented
in the README rather than worked around.

## Testing

Test-driven, per the project's usual workflow. Write the failing test first.

**Unit tests in `analyze.rs`** — one per row of the scoring table, plus the
interaction cases where a rule could plausibly be read two ways:

- a guarded catch-all is not a catch-all (`_ if c => …` scores 2, not 0)
- a catch-all that is not the last arm scores +1
- nested alternatives: `Some(A | B)` scores 1 alternative
- `else if` chains score once per `if`, and a bare `else` adds nothing
- `&`/`|` score 0 while `&&`/`||` score 1
- a bodyless `TraitItemFn` produces no row
- a closure's branches are not charged to its parent
- `--no-logical` and `--question` each change only their own construct

**Fixtures** — `fixtures/`, hand-checked files with expected scores, covering
nesting, closures in iterator chains, trait defaults, generics and turbofish,
macro invocations, `async fn`, raw strings and raw identifiers, and a file mixing
all of them.

**Integration tests** — exit codes for clean / `--max` breach / unreadable path /
unparseable file, `--json` shape, stdin, `--min`/`--top`/`--sort` interaction,
and the span-invalidation invariant: scan a multi-file fixture directory and
assert every file's line numbers, not just the first.

## Limitations

To document in the README:

- **Macro bodies are opaque.** `syn` keeps a macro invocation's contents as an
  unparsed token stream, so branches inside `assert!`, `matches!`, `write!`, or
  any custom macro are not counted, and `macro_rules!` definitions are not scored
  at all. This is the largest single blind spot, and the counterpart to jscx's
  longer list of lexical edge cases.
- **A file that does not parse yields no rows.** Nightly-only syntax and syntax
  errors take out the whole file.
- **Branches outside a function are not scored** — `const` and `static`
  initializers, array lengths.
- **`#[cfg]`-disabled code is scored anyway.** `syn` does not evaluate `cfg`, so
  a platform-specific branch is counted on every platform. `--no-tests` handles
  the common `#[cfg(test)]` case specifically.
- **Cyclomatic complexity is a blunt instrument.** It counts branches, not how
  tangled they are. A flat 20-arm `match` scores 21 and is usually fine; a nested
  8 is often worse. Read it as a place to look, not a verdict.

## Build order

1. Scoring core — the rule table, under test, against inline snippets.
2. Function discovery, naming, and nesting.
3. CLI argument parsing and the file walk, including span invalidation.
4. Table and JSON output.
5. README, with the worked example and the limitations above.

## Out of scope

Deliberately not in this project, listed so the API stays ready for them:

- `--lsp` and the LSP diagnostics server.
- The Zed, JetBrains, and Neovim extensions.
- Cognitive complexity, or any second metric.
- Parallel file scanning. Worth revisiting only if a real repository is slow;
  note that it interacts with the thread-local span map, which is per-thread and
  so is actually made easier by parallelism, not harder.
