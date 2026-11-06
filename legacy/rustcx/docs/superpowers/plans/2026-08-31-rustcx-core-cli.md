# rustcx Core CLI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A single binary that walks Rust source and prints the cyclomatic complexity of every function, with the CLI, output format, and exit codes of the existing `jscx` tool.

**Architecture:** `src/analyze.rs` parses each file with `syn` and walks the AST with a `syn::visit::Visit` implementation that holds a stack of in-progress functions; every decision point pushes a `Point` onto the innermost frame, and a function's complexity is `1 + points.len()`. `src/main.rs` parses arguments by hand, walks the filesystem, and renders a table or JSON. There is no lexer and no heuristics — the scoring rules are an exhaustive match over syn node types.

**Tech Stack:** Rust 2021, `syn` 3 (`full` + `visit`, no `printing`), `proc-macro2` with `span-locations`, `colored`, `serde`, `serde_json`. No `clap`.

**Spec:** `docs/superpowers/specs/2026-08-31-rustcx-design.md`

## Global Constraints

- **Pin syn 3, not syn 2.** The AST differs in ways this plan depends on: match guards are `Pat::Guard` (not an `Arm::guard` field), `let…else` is `LocalInit.diverge`, and `ExprClosure` uses `inputs_begin` / `inputs_end` (not `or1_token` / `or2_token`).
- **`syn` is built with `default-features = false`.** That removes `printing`, so **`syn::spanned::Spanned` is not available**. Every span must come from a concrete token field. Keyword tokens expose `.span` (a `Span`); punctuation tokens expose `.spans` (a `[Span; N]`) — use `.spans[0]`.
- **Span lifetime invariant.** Resolve every span to a `u32` line *during* the walk. Never let a `Span` or any `syn` node escape `analyze()`. `main.rs` calls `proc_macro2::extra::invalidate_current_thread_spans()` after each file; violating the invariant produces wrong line numbers or an internal panic, and does **not** fail to compile.
- **Complexity is always derived, never accumulated:** `complexity = 1 + points.len()`. Never increment a counter separately from pushing a `Point`. This is what keeps `--explain` and the score from ever disagreeing.
- **`?` scores 0 by default** (opt in with `--question`). **`&&` and `||` score 1 by default** (opt out with `--no-logical`).
- Every scoring rule is tested by an inline `#[cfg(test)]` test in `src/analyze.rs`. Every CLI behavior is tested in `tests/cli.rs`.
- Commit after every task.

---

## File Structure

| File | Responsibility |
| --- | --- |
| `Cargo.toml` | Manifest. Lib + bin, pinned dependencies, release profile. |
| `src/lib.rs` | Two lines: `pub mod analyze;`. |
| `src/analyze.rs` | The whole analyzer: types, the `Walker` visitor, scoring rules, naming. Inline unit tests. |
| `src/main.rs` | Argument parsing, file discovery, the run loop, table and JSON output. |
| `tests/cli.rs` | Integration tests driving the built binary. |
| `fixtures/*.rs` | Hand-checked Rust files with known scores. |
| `README.md` | Usage, scoring table, limitations. |

`src/analyze.rs` is expected to land near 450 lines. If it passes ~500, split the scoring rules into `src/score.rs` and leave discovery and naming in `analyze.rs`.

---

### Task 1: Project skeleton, core types, and a function that scores 1

Establishes the manifest, the public API, syn parsing, and span-to-line resolution. Every later task adds rules to the `Walker` built here.

**Files:**
- Create: `Cargo.toml`
- Create: `src/lib.rs`
- Create: `src/analyze.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `analyze::{analyze, Options, FnKind, Point, Func, ParseError}`; internal `Walker` with `open`, `close`, `point`, `finish`. Every later analyzer task adds `Visit` methods to this `Walker`.

- [ ] **Step 1: Write the manifest**

Create `Cargo.toml`:

```toml
[package]
name = "rustcx"
version = "0.1.0"
edition = "2021"
description = "Cyclomatic complexity for Rust functions"
license = "MIT"

[lib]
name = "rustcx"
path = "src/lib.rs"

[dependencies]
syn = { version = "3", default-features = false, features = ["full", "parsing", "visit", "clone-impls"] }
proc-macro2 = { version = "1", features = ["span-locations"] }
colored = "2.0"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"

[profile.release]
opt-level = 3
lto = true
codegen-units = 1
panic = "abort"
```

Create `src/lib.rs`:

```rust
pub mod analyze;
```

- [ ] **Step 2: Write the failing test**

Create `src/analyze.rs` containing *only* the test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Analyze `src` with default options and return (name, complexity) pairs
    /// in source order. Panics on a parse error — tests that want the error
    /// call `analyze` directly.
    fn scores(src: &str) -> Vec<(String, u32)> {
        analyze(src, Options::default())
            .expect("fixture should parse")
            .into_iter()
            .map(|f| (f.name, f.complexity))
            .collect()
    }

    #[test]
    fn plain_function_scores_one() {
        assert_eq!(scores("fn f() { let x = 1; }"), [("f".to_string(), 1)]);
    }

    #[test]
    fn records_line_kind_and_depth() {
        let out = analyze("\n\nfn f() {}", Options::default()).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].line, 3);
        assert_eq!(out[0].kind, FnKind::Fn);
        assert_eq!(out[0].depth, 0);
        assert!(out[0].points.is_empty());
    }

    #[test]
    fn syntax_error_reports_line_and_message() {
        let err = analyze("fn f( {", Options::default()).unwrap_err();
        assert!(err.line >= 1);
        assert!(!err.message.is_empty());
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test`
Expected: FAIL to compile — `cannot find function 'analyze' in this scope`.

- [ ] **Step 4: Write minimal implementation**

Prepend to `src/analyze.rs`, above the test module:

```rust
//! Cyclomatic complexity for Rust functions.
//!
//! Every function starts at 1 and gains a point per decision point. Complexity
//! is always derived as `1 + points.len()` so a score and its `--explain`
//! listing can never disagree.

use proc_macro2::Span;
use serde::Serialize;
use syn::visit::{self, Visit};

/// Which constructs count as decision points.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Count `&&` and `||`. Default true.
    pub logical: bool,
    /// Count `?`. Default false — see the spec's rationale.
    pub question: bool,
    /// Score `#[test]` functions and `#[cfg(test)]` modules. Default true.
    pub tests: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { logical: true, question: false, tests: true }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FnKind {
    Fn,
    Method,
    TraitFn,
    Closure,
}

impl FnKind {
    /// The string shown in the Kind column.
    pub fn label(self) -> &'static str {
        match self {
            FnKind::Fn => "fn",
            FnKind::Method => "method",
            FnKind::TraitFn => "trait fn",
            FnKind::Closure => "closure",
        }
    }
}

/// One decision point, for `--explain`.
#[derive(Clone, Debug, Serialize)]
pub struct Point {
    pub line: u32,
    pub branch: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct Func {
    pub name: String,
    pub kind: FnKind,
    pub line: u32,
    pub complexity: u32,
    /// Nesting level. 0 is a top-level function; a closure inside it is 1.
    pub depth: u32,
    pub points: Vec<Point>,
}

#[derive(Clone, Debug)]
pub struct ParseError {
    pub line: u32,
    pub message: String,
}

/// Score every function in one Rust source file.
///
/// Returns functions in source order: a parent is immediately followed by the
/// functions nested inside it, which is what the table's indentation relies on.
///
/// All spans are resolved to line numbers before this returns, so the caller is
/// free to call `proc_macro2::extra::invalidate_current_thread_spans()`
/// afterwards. See the Global Constraints in the plan.
pub fn analyze(src: &str, opts: Options) -> Result<Vec<Func>, ParseError> {
    let file = syn::parse_file(src).map_err(|e| ParseError {
        line: e.span().start().line as u32,
        message: e.to_string(),
    })?;
    let mut walker = Walker::new(opts);
    walker.visit_file(&file);
    Ok(walker.finish())
}

struct Walker {
    opts: Options,
    /// Functions in source order (pre-order: parent before its children).
    funcs: Vec<Func>,
    /// Indices into `funcs` for the functions currently being walked, innermost
    /// last. A decision point is credited to the innermost frame only, which is
    /// what makes nested functions score independently.
    stack: Vec<usize>,
}

impl Walker {
    fn new(opts: Options) -> Self {
        Walker { opts, funcs: Vec::new(), stack: Vec::new() }
    }

    fn open(&mut self, name: String, kind: FnKind, span: Span) {
        let depth = self.stack.len() as u32;
        self.funcs.push(Func {
            name,
            kind,
            line: span.start().line as u32,
            complexity: 1,
            depth,
            points: Vec::new(),
        });
        self.stack.push(self.funcs.len() - 1);
    }

    fn close(&mut self) {
        self.stack.pop();
    }

    /// Credit a decision point to the innermost open function. Points found
    /// outside any function — in a `const` initializer, say — are dropped.
    fn point(&mut self, span: Span, branch: &'static str) {
        if let Some(&idx) = self.stack.last() {
            self.funcs[idx].points.push(Point {
                line: span.start().line as u32,
                branch,
            });
        }
    }

    fn finish(mut self) -> Vec<Func> {
        for f in &mut self.funcs {
            f.complexity = 1 + f.points.len() as u32;
        }
        self.funcs
    }
}

impl<'ast> Visit<'ast> for Walker {
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        self.open(node.sig.ident.to_string(), FnKind::Fn, node.sig.fn_token.span);
        visit::visit_item_fn(self, node);
        self.close();
    }
}
```

Note `self.opts` is unused until Task 2; add `#[allow(dead_code)]` only if the warning blocks you, and remove it in Task 2.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test`
Expected: 3 passed.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock src/lib.rs src/analyze.rs
git commit -m "feat: analyzer skeleton scoring every function at 1"
```

---

### Task 2: Branching statements and logical operators

Adds `if` / `else if`, `while`, `for`, `loop`, and `&&` / `||`. The `if let` and `while let` *labels* land here too (one `matches!` each); their pattern behavior is tested in Task 4.

**Files:**
- Modify: `src/analyze.rs` — add `Visit` methods to the existing `impl`, add tests

**Interfaces:**
- Consumes: `Walker::point`, `Options::logical` from Task 1.
- Produces: point branch labels `"if"`, `"if let"`, `"while"`, `"while let"`, `"for"`, `"loop"`, `"&&"`, `"||"`.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `src/analyze.rs`:

```rust
    #[test]
    fn if_scores_one_and_plain_else_scores_nothing() {
        assert_eq!(scores("fn f(a: bool) { if a { } else { } }"), [("f".into(), 2)]);
    }

    #[test]
    fn else_if_scores_as_a_nested_if() {
        let src = "fn f(a: bool, b: bool) { if a { } else if b { } else { } }";
        assert_eq!(scores(src), [("f".into(), 3)]);
    }

    #[test]
    fn loops_each_score_one() {
        assert_eq!(scores("fn f(a: bool) { while a { } }"), [("f".into(), 2)]);
        assert_eq!(scores("fn f(v: Vec<u8>) { for _x in v { } }"), [("f".into(), 2)]);
        assert_eq!(scores("fn f() { loop { break; } }"), [("f".into(), 2)]);
    }

    #[test]
    fn logical_operators_score_one_each() {
        assert_eq!(scores("fn f(a: bool, b: bool, c: bool) -> bool { a && b || c }"),
                   [("f".into(), 3)]);
    }

    #[test]
    fn bitwise_operators_score_nothing() {
        assert_eq!(scores("fn f(a: u8, b: u8) -> u8 { a & b | a }"), [("f".into(), 1)]);
    }

    #[test]
    fn no_logical_option_disables_them() {
        let opts = Options { logical: false, ..Options::default() };
        let out = analyze("fn f(a: bool, b: bool) -> bool { a && b }", opts).unwrap();
        assert_eq!(out[0].complexity, 1);
    }

    #[test]
    fn return_and_break_score_nothing() {
        let src = "fn f(a: bool) -> u8 { loop { if a { break; } return 1; } }";
        // 1 base + 1 loop + 1 if
        assert_eq!(scores(src), [("f".into(), 3)]);
    }

    #[test]
    fn points_record_the_branch_label() {
        let out = analyze("fn f(a: bool, b: bool) { if a && b { } }", Options::default()).unwrap();
        let labels: Vec<&str> = out[0].points.iter().map(|p| p.branch).collect();
        assert_eq!(labels, ["if", "&&"]);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test`
Expected: the new tests FAIL, each asserting a complexity of 1 where more was expected.

- [ ] **Step 3: Write the implementation**

Add these methods inside the existing `impl<'ast> Visit<'ast> for Walker` block:

```rust
    fn visit_expr_if(&mut self, node: &'ast syn::ExprIf) {
        // `else if` arrives here as a nested ExprIf inside else_branch, so it
        // scores on its own. A plain `else` is an ExprBlock and scores nothing.
        let label = if matches!(*node.cond, syn::Expr::Let(_)) { "if let" } else { "if" };
        self.point(node.if_token.span, label);
        visit::visit_expr_if(self, node);
    }

    fn visit_expr_while(&mut self, node: &'ast syn::ExprWhile) {
        let label = if matches!(*node.cond, syn::Expr::Let(_)) { "while let" } else { "while" };
        self.point(node.while_token.span, label);
        visit::visit_expr_while(self, node);
    }

    fn visit_expr_for_loop(&mut self, node: &'ast syn::ExprForLoop) {
        self.point(node.for_token.span, "for");
        visit::visit_expr_for_loop(self, node);
    }

    fn visit_expr_loop(&mut self, node: &'ast syn::ExprLoop) {
        self.point(node.loop_token.span, "loop");
        visit::visit_expr_loop(self, node);
    }

    fn visit_expr_binary(&mut self, node: &'ast syn::ExprBinary) {
        if self.opts.logical {
            // Punctuation tokens carry `spans: [Span; N]`, not `span` — and
            // `syn::spanned::Spanned` is unavailable without the `printing`
            // feature. `&` and `|` are not short-circuiting, so they score 0.
            match node.op {
                syn::BinOp::And(t) => self.point(t.spans[0], "&&"),
                syn::BinOp::Or(t) => self.point(t.spans[0], "||"),
                _ => {}
            }
        }
        visit::visit_expr_binary(self, node);
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test`
Expected: all passed.

- [ ] **Step 5: Commit**

```bash
git add src/analyze.rs
git commit -m "feat: score if, loops, and logical operators"
```

---

### Task 3: `match` arms, pattern alternatives, and guards

The subtlest rule set. An arm is +1 unless it is the last arm *and* an unguarded catch-all. Alternatives and guards are handled by `visit_pat_or` and `visit_pat_guard`, which fire for every refutable pattern position — so Task 4 gets `if let` alternatives for free.

**Files:**
- Modify: `src/analyze.rs`

**Interfaces:**
- Consumes: `Walker::point`.
- Produces: point labels `"match arm"`, `"pattern alternative"`, `"guard"`; free function `is_catch_all(&syn::Pat) -> bool`.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module:

```rust
    #[test]
    fn arms_score_one_each_and_a_trailing_catch_all_scores_nothing() {
        let src = "fn f(x: u8) -> u8 { match x { 1 => 1, 2 => 2, 3 => 3, _ => 0 } }";
        // 1 base + 3 arms + 0 catch-all
        assert_eq!(scores(src), [("f".into(), 4)]);
    }

    #[test]
    fn a_bare_binding_is_also_a_catch_all() {
        let src = "fn f(x: u8) -> u8 { match x { 1 => 1, other => other } }";
        assert_eq!(scores(src), [("f".into(), 2)]);
    }

    #[test]
    fn a_guarded_catch_all_is_not_a_catch_all() {
        // The arm can be reached and still fail, so it is a real decision:
        // 1 base + 1 first arm + 1 guarded arm + 1 guard = 4
        let src = "fn f(x: u8, c: bool) -> u8 { match x { 1 => 1, _ if c => 2, _ => 0 } }";
        // 1 base + arm(1) + arm(_ if c) + guard + catch-all(0) = 4
        assert_eq!(scores(src), [("f".into(), 4)]);
    }

    #[test]
    fn a_catch_all_that_is_not_last_still_scores() {
        let src = "fn f(x: u8) -> u8 { match x { _ => 0, 1 => 1 } }";
        assert_eq!(scores(src), [("f".into(), 3)]);
    }

    #[test]
    fn pattern_alternatives_score_one_beyond_the_first() {
        let src = "fn f(x: u8) -> u8 { match x { 1 | 2 | 3 => 1, _ => 0 } }";
        // 1 base + 1 arm + 2 alternatives
        assert_eq!(scores(src), [("f".into(), 4)]);
    }

    #[test]
    fn nested_alternatives_are_counted() {
        let src = "fn f(x: Option<u8>) -> u8 { match x { Some(1 | 2) => 1, _ => 0 } }";
        // 1 base + 1 arm + 1 alternative
        assert_eq!(scores(src), [("f".into(), 3)]);
    }

    #[test]
    fn guards_score_one_and_their_operators_still_count() {
        let src = "fn f(x: u8, a: bool, b: bool) -> u8 { match x { 1 if a && b => 1, _ => 0 } }";
        // 1 base + 1 arm + 1 guard + 1 &&
        assert_eq!(scores(src), [("f".into(), 4)]);
    }

    #[test]
    fn an_empty_match_scores_only_its_base() {
        assert_eq!(scores("fn f(x: std::convert::Infallible) { match x { } }"),
                   [("f".into(), 1)]);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test`
Expected: the eight new tests FAIL with complexities too low.

- [ ] **Step 3: Write the implementation**

Add to the `impl<'ast> Visit<'ast> for Walker` block:

```rust
    fn visit_expr_match(&mut self, node: &'ast syn::ExprMatch) {
        self.visit_expr(&node.expr);
        let last = node.arms.len().saturating_sub(1);
        for (i, arm) in node.arms.iter().enumerate() {
            if !(i == last && is_catch_all(&arm.pat)) {
                self.point(arm.fat_arrow_token.spans[0], "match arm");
            }
            // visit_arm walks the pattern, which picks up alternatives and any
            // guard (a guard is part of the pattern in syn 3), then the body.
            self.visit_arm(arm);
        }
    }

    fn visit_pat_or(&mut self, node: &'ast syn::PatOr) {
        // One point per alternative beyond the first: the punctuation between
        // cases. A leading vert is not punctuation between cases, so `| A | B`
        // correctly scores 1.
        for pair in node.cases.pairs() {
            if let Some(punct) = pair.punct() {
                self.point(punct.spans[0], "pattern alternative");
            }
        }
        visit::visit_pat_or(self, node);
    }

    fn visit_pat_guard(&mut self, node: &'ast syn::PatGuard) {
        self.point(node.if_token.span, "guard");
        // The default walk visits the inner pattern and the guard expression,
        // so operators inside the guard still score.
        visit::visit_pat_guard(self, node);
    }
```

And add this free function alongside `type_name` later in the file (top level, outside the impl):

```rust
/// A catch-all arm: `_` or a bare binding, with no subpattern.
///
/// A guarded arm is a `Pat::Guard`, which matches neither branch here, so
/// `_ if c => ...` is correctly *not* a catch-all.
fn is_catch_all(pat: &syn::Pat) -> bool {
    match pat {
        syn::Pat::Wild(_) => true,
        syn::Pat::Ident(id) => id.subpat.is_none(),
        _ => false,
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test`
Expected: all passed.

- [ ] **Step 5: Commit**

```bash
git add src/analyze.rs
git commit -m "feat: score match arms, pattern alternatives, and guards"
```

---

### Task 4: `let … else`, the `?` operator, and refutable patterns elsewhere

Completes the scoring table. Ends with the spec's worked example as an integration check of every rule at once.

**Files:**
- Modify: `src/analyze.rs`

**Interfaces:**
- Consumes: `Walker::point`, `Options::question`.
- Produces: point labels `"let else"`, `"?"`. `visit_local` is introduced here and **extended in Task 6** for closure naming.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module:

```rust
    #[test]
    fn if_let_and_while_let_score_one() {
        assert_eq!(scores("fn f(o: Option<u8>) { if let Some(_x) = o { } }"),
                   [("f".into(), 2)]);
        assert_eq!(scores("fn f(mut i: std::vec::IntoIter<u8>) { while let Some(_x) = i.next() { } }"),
                   [("f".into(), 2)]);
    }

    #[test]
    fn alternatives_count_outside_match_too() {
        let src = "enum E { A, B, C } fn f(e: E) { if let E::A | E::B = e { } }";
        // 1 base + 1 if let + 1 alternative
        assert_eq!(scores(src), [("f".into(), 3)]);
    }

    #[test]
    fn let_else_scores_one() {
        let src = "fn f(o: Option<u8>) -> u8 { let Some(x) = o else { return 0; }; x }";
        assert_eq!(scores(src), [("f".into(), 2)]);
    }

    #[test]
    fn question_scores_nothing_by_default() {
        let src = "fn f(s: &str) -> Result<u8, std::num::ParseIntError> { Ok(s.parse::<u8>()? + s.parse::<u8>()?) }";
        assert_eq!(scores(src), [("f".into(), 1)]);
    }

    #[test]
    fn question_scores_one_each_when_enabled() {
        let opts = Options { question: true, ..Options::default() };
        let src = "fn f(s: &str) -> Result<u8, std::num::ParseIntError> { Ok(s.parse::<u8>()? + s.parse::<u8>()?) }";
        let out = analyze(src, opts).unwrap();
        assert_eq!(out[0].complexity, 3);
    }

    #[test]
    fn all_rules_combine_on_one_function() {
        // The spec's worked example, written as a free function because impl
        // methods are not discovered until Task 5. Task 5 re-tests the same
        // code inside an `impl`.
        let src = r#"
enum Event { Key(Key), Click, Drag, Resize(i32, i32) }
struct Key { ctrl: bool }
fn handle(window: Option<u8>, ev: Event) -> Result<(), ()> {
    let Some(_w) = window else { return Ok(()) };
    match ev {
        Event::Key(k) if k.ctrl => Ok(()),
        Event::Click | Event::Drag => Ok(()),
        Event::Resize(a, b) if a > 0 && b > 0 => Ok(()),
        Event::Resize(..) => Ok(()),
        _ => Ok(()),
    }?;
    Ok(())
}
"#;
        let out = analyze(src, Options::default()).unwrap();
        // 1 base + let-else + (arm+guard) + (arm+alt) + (arm+guard+&&) + arm
        // + catch-all(0) + ?(0) = 10
        assert_eq!(out[0].complexity, 10);
    }

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test`
Expected: the new tests FAIL with complexities too low.

- [ ] **Step 3: Write the implementation**

Add to the `impl<'ast> Visit<'ast> for Walker` block:

```rust
    fn visit_local(&mut self, node: &'ast syn::Local) {
        // `let ... else`: the diverging branch is a decision point. In syn 3
        // this lives on LocalInit, not on Local.
        if let Some(init) = &node.init {
            if init.diverge.is_some() {
                self.point(node.let_token.span, "let else");
            }
        }
        visit::visit_local(self, node);
    }

    fn visit_expr_try(&mut self, node: &'ast syn::ExprTry) {
        // `?` is a conditional early return. `return` scores 0 here, matching
        // ESLint's complexity rule, so `?` follows it and is opt-in.
        if self.opts.question {
            self.point(node.question_token.spans[0], "?");
        }
        visit::visit_expr_try(self, node);
    }
```

The `if let` / `while let` alternatives need no new code — `visit_pat_or` from Task 3 already fires on those patterns.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test`
Expected: all passed.

- [ ] **Step 5: Commit**

```bash
git add src/analyze.rs
git commit -m "feat: score let-else and the ? operator"
```

---

### Task 5: Methods, trait defaults, closures, and nesting

Function *discovery*. Until now only free `fn` items produced rows. Closures are all named `"closure"` here; Task 6 names them properly.

**Files:**
- Modify: `src/analyze.rs`

**Interfaces:**
- Consumes: `Walker::open` / `close`, `FnKind`.
- Produces: `Walker.type_stack: Vec<String>` (used by Task 6), free function `type_name(&syn::Type) -> String`.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module — including the worked-example test moved here from Task 4:

```rust
    fn rows(src: &str) -> Vec<(String, &'static str, u32, u32)> {
        analyze(src, Options::default())
            .unwrap()
            .into_iter()
            .map(|f| (f.name, f.kind.label(), f.complexity, f.depth))
            .collect()
    }

    #[test]
    fn impl_methods_are_named_for_their_type() {
        let src = "struct S; impl S { fn m(&self, a: bool) { if a { } } }";
        assert_eq!(rows(src), [("S::m".to_string(), "method", 2, 0)]);
    }

    #[test]
    fn trait_defaults_are_scored_and_bare_signatures_are_not() {
        let src = "trait T { fn required(&self); fn provided(&self, a: bool) { if a { } } }";
        assert_eq!(rows(src), [("T::provided".to_string(), "trait fn", 2, 0)]);
    }

    #[test]
    fn a_closure_gets_its_own_row_and_the_parent_is_not_charged() {
        let src = "fn f(v: &[i32]) { v.iter().map(|x| if *x > 0 { 1 } else { 0 }); }";
        let out = rows(src);
        assert_eq!(out[0].0, "f");
        assert_eq!(out[0].2, 1, "parent must not be charged for the closure's if");
        assert_eq!(out[1].1, "closure");
        assert_eq!(out[1].2, 2);
        assert_eq!(out[1].3, 1, "closure nests one level inside f");
    }

    #[test]
    fn nested_functions_score_independently() {
        let src = "fn outer(a: bool) { fn inner(b: bool) { if b { } } if a { } }";
        let out = rows(src);
        assert_eq!(out[0].0, "outer");
        assert_eq!(out[0].2, 2);
        assert_eq!(out[1].0, "inner");
        assert_eq!(out[1].2, 2);
        assert_eq!(out[1].3, 1);
    }

    #[test]
    fn rows_come_back_in_source_order_with_children_after_parents() {
        let src = "fn a() { let _c = || (); } fn b() {}";
        let names: Vec<String> = rows(src).into_iter().map(|r| r.0).collect();
        assert_eq!(names, ["a", "closure", "b"]);
    }

    #[test]
    fn the_spec_worked_example_scores_ten() {
        let src = r#"
enum Event { Key(Key), Click, Drag, Resize(i32, i32) }
struct Key { ctrl: bool }
struct App { window: Option<u8> }
impl App {
    fn handle(&mut self, ev: Event) -> Result<(), ()> {
        let Some(_w) = self.window else { return Ok(()) };
        match ev {
            Event::Key(k) if k.ctrl => self.command(k),
            Event::Click | Event::Drag => self.pointer(ev),
            Event::Resize(a, b) if a > 0 && b > 0 => self.fit(),
            Event::Resize(..) => Ok(()),
            _ => self.ignore(),
        }?;
        Ok(())
    }
    fn command(&self, _k: Key) -> Result<(), ()> { Ok(()) }
    fn pointer(&self, _e: Event) -> Result<(), ()> { Ok(()) }
    fn fit(&self) -> Result<(), ()> { Ok(()) }
    fn ignore(&self) -> Result<(), ()> { Ok(()) }
}
"#;
        let out = analyze(src, Options::default()).unwrap();
        let handle = out.iter().find(|f| f.name == "App::handle").unwrap();
        assert_eq!(handle.complexity, 10);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test`
Expected: FAIL — methods, trait defaults, and closures produce no rows at all, so the vectors are empty.

- [ ] **Step 3: Write the implementation**

Add the field to `Walker` and its initializer:

```rust
struct Walker {
    opts: Options,
    funcs: Vec<Func>,
    stack: Vec<usize>,
    /// Enclosing `impl` or `trait` type names, for qualifying method names.
    type_stack: Vec<String>,
}
```

```rust
    fn new(opts: Options) -> Self {
        Walker { opts, funcs: Vec::new(), stack: Vec::new(), type_stack: Vec::new() }
    }
```

Add a helper to `impl Walker`:

```rust
    /// `Type::method` when inside an impl or trait, otherwise the bare name.
    /// Module paths are deliberately not prefixed — the file path is already
    /// printed above the table.
    fn qualified(&self, ident: &syn::Ident) -> String {
        match self.type_stack.last() {
            Some(ty) => format!("{}::{}", ty, ident),
            None => ident.to_string(),
        }
    }
```

Add to the `Visit` impl:

```rust
    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        self.type_stack.push(type_name(&node.self_ty));
        visit::visit_item_impl(self, node);
        self.type_stack.pop();
    }

    fn visit_item_trait(&mut self, node: &'ast syn::ItemTrait) {
        self.type_stack.push(node.ident.to_string());
        visit::visit_item_trait(self, node);
        self.type_stack.pop();
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        let name = self.qualified(&node.sig.ident);
        self.open(name, FnKind::Method, node.sig.fn_token.span);
        visit::visit_impl_item_fn(self, node);
        self.close();
    }

    fn visit_trait_item_fn(&mut self, node: &'ast syn::TraitItemFn) {
        // A signature with no default body is not a function.
        if node.default.is_none() {
            return;
        }
        let name = self.qualified(&node.sig.ident);
        self.open(name, FnKind::TraitFn, node.sig.fn_token.span);
        visit::visit_trait_item_fn(self, node);
        self.close();
    }

    fn visit_expr_closure(&mut self, node: &'ast syn::ExprClosure) {
        // syn 3 calls the pipes `inputs_begin` / `inputs_end`, and they are
        // punctuation, so the span lives in `.spans[0]`.
        self.open("closure".to_string(), FnKind::Closure, node.inputs_begin.spans[0]);
        visit::visit_expr_closure(self, node);
        self.close();
    }
```

And the free function, next to `is_catch_all`:

```rust
/// The last path segment of an impl's self type: `Walker<'a>` becomes `Walker`.
///
/// Without syn's `printing` feature there is no `ToTokens`, so unusual self
/// types are not stringified; they fall back to `impl`.
fn type_name(ty: &syn::Type) -> String {
    match ty {
        syn::Type::Path(p) => p
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_else(|| "impl".to_string()),
        syn::Type::Reference(r) => type_name(&r.elem),
        _ => "impl".to_string(),
    }
}
```

Nested `fn` items need no new code: syn's default walk descends into `Stmt::Item`, so `visit_item_fn` already fires for them and the stack supplies the depth.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test`
Expected: all passed.

- [ ] **Step 5: Commit**

```bash
git add src/analyze.rs
git commit -m "feat: discover methods, trait defaults, closures, and nesting"
```

---

### Task 6: Closure names

A closure inherits the name of whatever it is attached to, the way jscx names arrow functions.

**Files:**
- Modify: `src/analyze.rs`

**Interfaces:**
- Consumes: `Walker.stack`, `visit_local` from Task 4, `visit_expr_closure` from Task 5.
- Produces: `Walker.closure_name: Option<String>`, free functions `member_name`, `callee_name`.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module:

```rust
    fn names(src: &str) -> Vec<String> {
        analyze(src, Options::default()).unwrap().into_iter().map(|f| f.name).collect()
    }

    #[test]
    fn a_closure_takes_the_name_of_its_let_binding() {
        assert_eq!(names("fn f() { let render = |x: u8| x; }"), ["f", "render"]);
    }

    #[test]
    fn a_closure_takes_the_name_of_the_method_it_is_passed_to() {
        assert_eq!(names("fn f(o: Option<u8>) -> u8 { o.unwrap_or_else(|| 0) }"),
                   ["f", "unwrap_or_else"]);
    }

    #[test]
    fn a_closure_takes_the_name_of_the_function_it_is_passed_to() {
        // A closure passed to a plain call takes the callee's name, so the
        // closure inside `f` is also called "g".
        assert_eq!(names("fn g<F: Fn()>(_f: F) {} fn f() { g(|| ()); }"), ["g", "f", "g"]);
    }

    #[test]
    fn a_closure_takes_the_name_of_the_struct_field_it_initializes() {
        let src = "struct S<F> { on_click: F } fn f() { let _s = S { on_click: |x: u8| x }; }";
        assert_eq!(names(src), ["f", "on_click"]);
    }

    #[test]
    fn an_unattached_closure_falls_back_to_closure() {
        assert_eq!(names("fn f() { (|x: u8| x)(1); }"), ["f", "closure"]);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test`
Expected: FAIL — every closure is currently named `"closure"`.

- [ ] **Step 3: Write the implementation**

Add the field to `Walker` and its initializer:

```rust
struct Walker {
    opts: Options,
    funcs: Vec<Func>,
    stack: Vec<usize>,
    type_stack: Vec<String>,
    /// Name to give the next closure visited. Set by the parent expression
    /// immediately before descending into the closure, consumed by
    /// `visit_expr_closure`.
    closure_name: Option<String>,
}
```

```rust
    fn new(opts: Options) -> Self {
        Walker {
            opts,
            funcs: Vec::new(),
            stack: Vec::new(),
            type_stack: Vec::new(),
            closure_name: None,
        }
    }
```

Change `visit_expr_closure` (from Task 5) to consume it:

```rust
    fn visit_expr_closure(&mut self, node: &'ast syn::ExprClosure) {
        let name = self.closure_name.take().unwrap_or_else(|| "closure".to_string());
        self.open(name, FnKind::Closure, node.inputs_begin.spans[0]);
        visit::visit_expr_closure(self, node);
        self.close();
    }
```

Extend `visit_local` (from Task 4) — keep the `let else` point exactly as it is and add the naming:

```rust
    fn visit_local(&mut self, node: &'ast syn::Local) {
        if let Some(init) = &node.init {
            if init.diverge.is_some() {
                self.point(node.let_token.span, "let else");
            }
            // `let render = |x| ...` names the closure `render`. A typed
            // binding (`let f: Box<dyn Fn()> = ...`) is a Pat::Type, not a
            // Pat::Ident, and falls through to the callee rule.
            if matches!(&*init.expr, syn::Expr::Closure(_)) {
                if let syn::Pat::Ident(id) = &node.pat {
                    self.closure_name = Some(id.ident.to_string());
                }
            }
        }
        visit::visit_local(self, node);
    }
```

Add three more methods. These walk their children by hand so the name can be set immediately before the closure argument is visited:

```rust
    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        self.visit_expr(&node.receiver);
        for arg in &node.args {
            if matches!(arg, syn::Expr::Closure(_)) {
                self.closure_name = Some(node.method.to_string());
            }
            self.visit_expr(arg);
        }
        // Turbofish generics are types; they hold no decision points.
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        self.visit_expr(&node.func);
        let callee = callee_name(&node.func);
        for arg in &node.args {
            if matches!(arg, syn::Expr::Closure(_)) {
                self.closure_name = callee.clone();
            }
            self.visit_expr(arg);
        }
    }

    fn visit_field_value(&mut self, node: &'ast syn::FieldValue) {
        if matches!(&node.expr, syn::Expr::Closure(_)) {
            self.closure_name = Some(member_name(&node.member));
        }
        visit::visit_field_value(self, node);
    }
```

And the two free functions, next to `type_name`:

```rust
fn member_name(m: &syn::Member) -> String {
    match m {
        syn::Member::Named(id) => id.to_string(),
        syn::Member::Unnamed(i) => i.index.to_string(),
    }
}

/// The last path segment of a call's callee, when it is a plain path.
fn callee_name(f: &syn::Expr) -> Option<String> {
    match f {
        syn::Expr::Path(p) => p.path.segments.last().map(|s| s.ident.to_string()),
        _ => None,
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test`
Expected: all passed.

- [ ] **Step 5: Commit**

```bash
git add src/analyze.rs
git commit -m "feat: name closures after what they attach to"
```

---

### Task 7: `--no-tests`

Rust keeps unit tests inline, and syn does not evaluate `cfg`, so a `#[cfg(test)]` module is otherwise walked like any other code.

**Files:**
- Modify: `src/analyze.rs`

**Interfaces:**
- Consumes: `Options::tests`.
- Produces: free functions `has_test_attr(&[syn::Attribute]) -> bool`, `is_cfg_test(&[syn::Attribute]) -> bool`.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module:

```rust
    const WITH_TESTS: &str = r#"
fn real(a: bool) { if a { } }
#[cfg(test)]
mod tests {
    #[test]
    fn t(a: bool) { if a { } }
    fn helper(a: bool) { if a { } }
}
"#;

    #[test]
    fn test_code_is_scored_by_default() {
        let names: Vec<String> = analyze(WITH_TESTS, Options::default())
            .unwrap().into_iter().map(|f| f.name).collect();
        assert_eq!(names, ["real", "t", "helper"]);
    }

    #[test]
    fn no_tests_skips_cfg_test_modules_entirely() {
        let opts = Options { tests: false, ..Options::default() };
        let names: Vec<String> = analyze(WITH_TESTS, opts)
            .unwrap().into_iter().map(|f| f.name).collect();
        assert_eq!(names, ["real"]);
    }

    #[test]
    fn no_tests_skips_a_bare_test_function_outside_a_test_module() {
        let src = "fn real() {} #[test] fn t(a: bool) { if a { } }";
        let opts = Options { tests: false, ..Options::default() };
        let names: Vec<String> = analyze(src, opts)
            .unwrap().into_iter().map(|f| f.name).collect();
        assert_eq!(names, ["real"]);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test`
Expected: the two `no_tests` tests FAIL — all three functions are still reported.

- [ ] **Step 3: Write the implementation**

Add to the `Visit` impl:

```rust
    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        if !self.opts.tests && is_cfg_test(&node.attrs) {
            return;
        }
        visit::visit_item_mod(self, node);
    }
```

And guard the two function entry points. In `visit_item_fn` (Task 1) and `visit_impl_item_fn` (Task 5), insert as the first line of each:

```rust
        if !self.opts.tests && has_test_attr(&node.attrs) {
            return;
        }
```

Add the two free functions:

```rust
fn has_test_attr(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| a.path().is_ident("test"))
}

/// Recognises `#[cfg(test)]`. A compound predicate such as
/// `#[cfg(all(test, feature = "x"))]` is not recognised — a documented
/// limitation, since the common case is the bare form.
fn is_cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        if !a.path().is_ident("cfg") {
            return false;
        }
        let mut found = false;
        let _ = a.parse_nested_meta(|meta| {
            if meta.path.is_ident("test") {
                found = true;
            }
            Ok(())
        });
        found
    })
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test && cargo clippy -- -D warnings`
Expected: all tests pass, clippy clean.

- [ ] **Step 5: Commit**

```bash
git add src/analyze.rs
git commit -m "feat: --no-tests skips #[test] fns and #[cfg(test)] modules"
```

The analyzer is now complete. Tasks 8 onward build the CLI around it.

---

### Task 8: Argument parsing

`main.rs` with a hand-rolled parser, matching jscx. No file walking or output yet — the deliverable is `rustcx --help` and a unit-tested `parse_args`.

**Files:**
- Modify: `Cargo.toml` — add the `[[bin]]` section
- Create: `src/main.rs`

**Interfaces:**
- Consumes: `rustcx::analyze::Options`.
- Produces: `struct Config`, `fn parse_args(&[String]) -> Result<Option<Config>, String>` (`Ok(None)` means help or version was printed and the process should exit 0).

- [ ] **Step 1: Add the bin target**

Append to `Cargo.toml`, after the `[lib]` section:

```toml
[[bin]]
name = "rustcx"
path = "src/main.rs"
```

- [ ] **Step 2: Write the failing tests**

Create `src/main.rs` with only the test module and the imports it needs:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn parse(list: &[&str]) -> Config {
        parse_args(&args(list)).unwrap().unwrap()
    }

    #[test]
    fn defaults_match_the_spec() {
        let c = parse(&["src/"]);
        assert_eq!(c.paths, vec![PathBuf::from("src/")]);
        assert_eq!(c.min, 1);
        assert_eq!(c.max, None);
        assert_eq!(c.top, None);
        assert!(!c.sort);
        assert!(c.header);
        assert!(!c.json);
        assert!(!c.explain);
        assert!(c.opts.logical, "&& and || count by default");
        assert!(!c.opts.question, "? does not count by default");
        assert!(c.opts.tests, "test code is scored by default");
        assert!(!c.stdin);
    }

    #[test]
    fn scoring_flags_flip_their_options() {
        assert!(!parse(&["--no-logical", "x.rs"]).opts.logical);
        assert!(parse(&["--question", "x.rs"]).opts.question);
        assert!(!parse(&["--no-tests", "x.rs"]).opts.tests);
    }

    #[test]
    fn numeric_flags_accept_short_and_long_forms() {
        assert_eq!(parse(&["-m", "3", "x.rs"]).min, 3);
        assert_eq!(parse(&["--min", "3", "x.rs"]).min, 3);
        assert_eq!(parse(&["-M", "15", "x.rs"]).max, Some(15));
        assert_eq!(parse(&["-t", "20", "x.rs"]).top, Some(20));
    }

    #[test]
    fn boolean_flags_are_recognised() {
        assert!(parse(&["-s", "x.rs"]).sort);
        assert!(parse(&["-j", "x.rs"]).json);
        assert!(parse(&["-e", "x.rs"]).explain);
        assert!(!parse(&["--no-header", "x.rs"]).header);
    }

    #[test]
    fn a_lone_dash_means_stdin() {
        let c = parse(&["-"]);
        assert!(c.stdin);
        assert!(c.paths.is_empty());
    }

    #[test]
    fn help_and_version_return_none() {
        assert!(parse_args(&args(&["--help"])).unwrap().is_none());
        assert!(parse_args(&args(&["-V"])).unwrap().is_none());
    }

    #[test]
    fn errors_are_reported_not_panicked() {
        assert!(parse_args(&args(&[])).is_err(), "no paths is an error");
        assert!(parse_args(&args(&["--nope", "x.rs"])).is_err());
        assert!(parse_args(&args(&["-m", "x.rs"])).is_err(), "-m needs a number");
        assert!(parse_args(&args(&["-m"])).is_err(), "-m needs a value at all");
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test`
Expected: FAIL to compile — `Config` and `parse_args` are not defined.

- [ ] **Step 4: Write the implementation**

Prepend to `src/main.rs`:

```rust
use rustcx::analyze::Options;
use std::path::PathBuf;
use std::process::ExitCode;

const HELP: &str = "\
rustcx — cyclomatic complexity for Rust functions

USAGE:
    rustcx [OPTIONS] <PATH>...     files or directories (recursive)
    rustcx [OPTIONS] -             read from stdin

OPTIONS:
    -m, --min <N>      only report functions scoring N or higher (default 1)
    -M, --max <N>      exit 1 if any function scores above N — for CI
    -t, --top <N>      show only the N worst functions
    -s, --sort         sort by score, highest first
        --no-header    do not print the table header
    -j, --json         machine-readable output
    -e, --explain      list the decision points behind each score
        --question     count `?` as a branch (off by default)
        --no-logical   don't count `&&`, `||`
        --no-tests     skip #[test] functions and #[cfg(test)] modules
    -h, --help         print this help
    -V, --version      print version
";

#[derive(Debug)]
struct Config {
    paths: Vec<PathBuf>,
    stdin: bool,
    min: u32,
    max: Option<u32>,
    top: Option<usize>,
    sort: bool,
    header: bool,
    json: bool,
    explain: bool,
    opts: Options,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            paths: Vec::new(),
            stdin: false,
            min: 1,
            max: None,
            top: None,
            sort: false,
            header: true,
            json: false,
            explain: false,
            opts: Options::default(),
        }
    }
}

/// Parse argv (without the program name).
///
/// `Ok(None)` means help or version was printed and the caller should exit 0.
fn parse_args(args: &[String]) -> Result<Option<Config>, String> {
    let mut cfg = Config::default();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "-h" | "--help" => {
                print!("{HELP}");
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("rustcx {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "-m" | "--min" => cfg.min = take_number(args, &mut i, arg)?,
            "-M" | "--max" => cfg.max = Some(take_number(args, &mut i, arg)?),
            "-t" | "--top" => cfg.top = Some(take_number(args, &mut i, arg)? as usize),
            "-s" | "--sort" => cfg.sort = true,
            "--no-header" => cfg.header = false,
            "-j" | "--json" => cfg.json = true,
            "-e" | "--explain" => cfg.explain = true,
            "--question" => cfg.opts.question = true,
            "--no-logical" => cfg.opts.logical = false,
            "--no-tests" => cfg.opts.tests = false,
            "-" => cfg.stdin = true,
            _ => {
                if arg.starts_with('-') && arg.len() > 1 {
                    return Err(format!("unknown option: {arg}"));
                }
                cfg.paths.push(PathBuf::from(arg));
            }
        }
        i += 1;
    }
    if cfg.paths.is_empty() && !cfg.stdin {
        return Err("no paths given".to_string());
    }
    Ok(Some(cfg))
}

/// Consume the value following a flag and parse it as a number, advancing `i`.
fn take_number(args: &[String], i: &mut usize, flag: &str) -> Result<u32, String> {
    *i += 1;
    let raw = args.get(*i).ok_or_else(|| format!("{flag} needs a value"))?;
    raw.parse::<u32>()
        .map_err(|_| format!("{flag} needs a number, got: {raw}"))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let _cfg = match parse_args(&args) {
        Ok(Some(cfg)) => cfg,
        Ok(None) => return ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("rustcx: {msg}");
            return ExitCode::from(2);
        }
    };
    ExitCode::SUCCESS
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test`
Expected: all passed.

- [ ] **Step 6: Verify the binary runs**

Run: `cargo run -- --help`
Expected: the usage text, exit 0.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml src/main.rs
git commit -m "feat: command-line argument parsing"
```

---

### Task 9: The table writer

A pure function over rows, testable before any file walking exists. Column widths are copied from jscx so the two tools' output is interchangeable.

**Files:**
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: `rustcx::analyze::{Func, FnKind}`.
- Produces: `struct Row { file: String, func: Func }`, `struct Summary { funcs: usize, files: usize, mean: f64, worst: Option<Row> }`, `fn summarize(&[Row]) -> Summary`, `fn write_table<W: Write>(w, &[Row], &Summary, header: bool, explain: bool) -> io::Result<()>`.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `src/main.rs`:

```rust
    // `Func` and `Options` already arrive via `use super::*`; importing them
    // again here would be a duplicate-definition error.
    use rustcx::analyze::{FnKind, Point};

    fn func(name: &str, kind: FnKind, line: u32, cc: u32, depth: u32) -> Func {
        Func {
            name: name.to_string(),
            kind,
            line,
            complexity: cc,
            depth,
            points: Vec::new(),
        }
    }

    fn sample() -> Vec<Row> {
        vec![
            Row { file: "src/a.rs".into(), func: func("validate", FnKind::Fn, 12, 14, 0) },
            Row { file: "src/a.rs".into(), func: func("map", FnKind::Closure, 61, 3, 1) },
            Row { file: "src/b.rs".into(), func: func("S::go", FnKind::Method, 4, 2, 0) },
        ]
    }

    fn render(rows: &[Row], header: bool, explain: bool) -> String {
        colored::control::set_override(false);
        let s = summarize(rows);
        let mut out = Vec::new();
        write_table(&mut out, rows, &s, header, explain).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn table_groups_by_file_and_indents_by_depth() {
        let expected = "\
src/a.rs
    CC   Line  Kind         Name
    14     12  fn           validate
     3     61  closure        map

src/b.rs
    CC   Line  Kind         Name
     2      4  method       S::go

3 functions in 2 files · mean 6.3 · worst 14 (validate at src/a.rs:12)
";
        assert_eq!(render(&sample(), true, false), expected);
    }

    #[test]
    fn no_header_drops_the_column_row() {
        let out = render(&sample(), false, false);
        assert!(!out.contains("CC   Line"));
        assert!(out.contains("    14     12  fn           validate"));
    }

    #[test]
    fn explain_lists_the_decision_points() {
        let mut rows = sample();
        rows[0].func.points = vec![
            Point { line: 13, branch: "if" },
            Point { line: 14, branch: "&&" },
        ];
        let out = render(&rows, true, true);
        assert!(out.contains("           13  if\n"), "got:\n{out}");
        assert!(out.contains("           14  &&\n"), "got:\n{out}");
    }

    #[test]
    fn summary_is_singular_for_one_function() {
        let rows = vec![Row { file: "a.rs".into(), func: func("f", FnKind::Fn, 1, 2, 0) }];
        let out = render(&rows, true, false);
        assert!(out.contains("1 function in 1 file · mean 2.0 · worst 2 (f at a.rs:1)"), "got:\n{out}");
    }

    #[test]
    fn empty_input_reports_nothing_found() {
        let out = render(&[], true, false);
        assert_eq!(out, "0 functions in 0 files\n");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test`
Expected: FAIL to compile — `Row`, `Summary`, `summarize`, `write_table` are undefined.

- [ ] **Step 3: Write the implementation**

Add to `src/main.rs`. Extend the imports at the top:

```rust
use colored::Colorize;
use rustcx::analyze::{Func, Options};
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
```

```rust
#[derive(Clone, Debug)]
struct Row {
    file: String,
    func: Func,
}

#[derive(Debug)]
struct Summary {
    funcs: usize,
    files: usize,
    mean: f64,
    worst: Option<Row>,
}

fn summarize(rows: &[Row]) -> Summary {
    let mut files: Vec<&str> = rows.iter().map(|r| r.file.as_str()).collect();
    files.sort_unstable();
    files.dedup();
    let total: u32 = rows.iter().map(|r| r.func.complexity).sum();
    let mean = if rows.is_empty() {
        0.0
    } else {
        f64::from(total) / rows.len() as f64
    };
    let worst = rows
        .iter()
        .max_by_key(|r| r.func.complexity)
        .cloned();
    Summary { funcs: rows.len(), files: files.len(), mean, worst }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Column layout, copied from jscx so the two tools' output lines up:
/// score right-aligned in 6, line right-aligned in 7, two spaces, kind
/// left-aligned in 13, then two spaces of indent per nesting level.
fn write_table<W: Write>(
    w: &mut W,
    rows: &[Row],
    summary: &Summary,
    header: bool,
    explain: bool,
) -> io::Result<()> {
    let mut current = "";
    for row in rows {
        if row.file != current {
            if !current.is_empty() {
                writeln!(w)?;
            }
            writeln!(w, "{}", row.file.bold())?;
            if header {
                writeln!(w, "{:>6}{:>7}  {:<13}{}", "CC", "Line", "Kind", "Name")?;
            }
            current = &row.file;
        }
        let f = &row.func;
        writeln!(
            w,
            "{:>6}{:>7}  {:<13}{}{}",
            f.complexity,
            f.line,
            f.kind.label(),
            " ".repeat(2 * f.depth as usize),
            f.name
        )?;
        if explain {
            for p in &f.points {
                // Six spaces, then the line right-aligned in 7 so it sits under
                // the Line column.
                writeln!(w, "      {:>7}  {}", p.line, p.branch)?;
            }
        }
    }
    if !rows.is_empty() {
        writeln!(w)?;
    }
    write!(
        w,
        "{} function{} in {} file{}",
        summary.funcs,
        plural(summary.funcs),
        summary.files,
        plural(summary.files)
    )?;
    if let Some(worst) = &summary.worst {
        write!(
            w,
            " · mean {:.1} · worst {} ({} at {}:{})",
            summary.mean, worst.func.complexity, worst.func.name, worst.file, worst.func.line
        )?;
    }
    writeln!(w)
}
```

Note `write_table` writes a blank line after the last file's rows and before the summary, which is what the expected string in the first test shows.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test`
Expected: all passed. If the golden-string test fails, diff carefully — the widths are `{:>6}{:>7}  {:<13}` and nothing else.

- [ ] **Step 5: Commit**

```bash
git add src/main.rs
git commit -m "feat: table output with per-file grouping and depth indentation"
```

---

### Task 10: File discovery, the run loop, and exit codes

Wires the analyzer to the table. This is the task that makes `rustcx src/` work, and the one that must honour the span-invalidation invariant.

**Files:**
- Modify: `src/main.rs`
- Create: `tests/cli.rs`
- Create: `fixtures/simple.rs`

**Interfaces:**
- Consumes: `analyze`, `Config`, `write_table`, `summarize`.
- Produces: `fn collect(&Path, &mut Vec<PathBuf>) -> io::Result<()>`, `fn is_source(&Path) -> bool`, `fn run(&Config) -> ExitCode`.

- [ ] **Step 1: Write the fixture**

Create `fixtures/simple.rs`:

```rust
// Hand-checked scores:
//   plain    1
//   branchy  5  = 1 + if + && + for + if
//   add_one  1, plus its closure as a separate row named `map`, scoring 1
pub fn plain() -> u8 {
    7
}

pub fn branchy(a: bool, b: bool, v: &[u8]) -> u8 {
    if a && b {
        return 1;
    }
    for x in v {
        if *x > 3 {
            return *x;
        }
    }
    0
}

pub fn add_one(v: &[u8]) -> Vec<u8> {
    v.iter().map(|x| x + 1).collect()
}
```

- [ ] **Step 2: Write the failing integration tests**

Create `tests/cli.rs`:

```rust
use std::process::Command;

fn rustcx(args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_rustcx"))
        .args(args)
        .output()
        .expect("binary should run");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// The row for `name`, or panic. Used to assert on the score column rather
/// than searching the whole output for a number that could match elsewhere.
fn row_for<'a>(stdout: &'a str, name: &str) -> &'a str {
    stdout
        .lines()
        .find(|l| l.trim_end().ends_with(name))
        .unwrap_or_else(|| panic!("no row for {name} in:\n{stdout}"))
}

#[test]
fn scores_a_file_and_exits_zero() {
    let (stdout, _, code) = rustcx(&["fixtures/simple.rs"]);
    assert_eq!(code, 0);
    assert!(row_for(&stdout, "branchy").starts_with("     5"),
            "branchy should score 5, got:\n{stdout}");
    assert!(row_for(&stdout, "plain").starts_with("     1"));
    assert!(row_for(&stdout, "map").starts_with("     1"),
            "the closure should get its own row");
}

#[test]
fn walks_a_directory() {
    let (stdout, _, code) = rustcx(&["fixtures"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("fixtures/simple.rs"));
}

#[test]
fn max_breach_exits_one() {
    let (_, _, code) = rustcx(&["--max", "4", "fixtures/simple.rs"]);
    assert_eq!(code, 1, "branchy scores 5, which is above 4");
}

#[test]
fn max_not_breached_exits_zero() {
    let (_, _, code) = rustcx(&["--max", "5", "fixtures/simple.rs"]);
    assert_eq!(code, 0, "--max is exclusive: 5 is not above 5");
}

#[test]
fn a_missing_path_exits_two() {
    let (_, stderr, code) = rustcx(&["does/not/exist.rs"]);
    assert_eq!(code, 2);
    assert!(!stderr.is_empty());
}

#[test]
fn an_unparseable_file_warns_and_exits_two_without_stopping() {
    let dir = std::env::temp_dir().join("rustcx-parse-test");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("broken.rs"), "fn f( {").unwrap();
    std::fs::write(dir.join("ok.rs"), "fn g(a: bool) { if a { } }").unwrap();

    let (stdout, stderr, code) = rustcx(&[dir.to_str().unwrap()]);
    assert_eq!(code, 2);
    assert!(stderr.contains("parse error"), "stderr:\n{stderr}");
    assert!(stdout.contains("g"), "the good file must still be scored:\n{stdout}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn stdin_is_read_with_a_dash() {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = Command::new(env!("CARGO_BIN_EXE_rustcx"))
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.as_mut().unwrap()
        .write_all(b"fn f(a: bool) { if a { } }").unwrap();
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0));
    assert!(stdout.contains("f"), "got:\n{stdout}");
    assert!(stdout.contains("<stdin>"), "got:\n{stdout}");
}

#[test]
fn line_numbers_stay_correct_across_many_files() {
    // Guards the span-invalidation invariant: proc-macro2's span table is
    // cleared between files, and a mistake there shows up as wrong lines in
    // every file after the first.
    let dir = std::env::temp_dir().join("rustcx-span-test");
    std::fs::create_dir_all(&dir).unwrap();
    for i in 0..5 {
        std::fs::write(
            dir.join(format!("f{i}.rs")),
            "\n\n\nfn f(a: bool) { if a { } }\n",
        )
        .unwrap();
    }
    let (stdout, _, code) = rustcx(&[dir.to_str().unwrap()]);
    assert_eq!(code, 0);
    let on_line_four = stdout.matches("      4  fn").count();
    assert_eq!(on_line_four, 5, "every file's fn is on line 4:\n{stdout}");

    std::fs::remove_dir_all(&dir).ok();
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test --test cli`
Expected: FAIL — the binary parses arguments and exits 0 without printing anything.

- [ ] **Step 4: Write the implementation**

Add to `src/main.rs`, extending imports with `use std::fs;` and `use std::path::Path;`:

```rust
const SKIP_DIRS: &[&str] = &["target", ".git", ".hg", ".svn", ".jj"];

fn is_source(p: &Path) -> bool {
    p.extension().is_some_and(|e| e == "rs")
}

/// Recursively collect `.rs` files under `root`, skipping build and VCS dirs.
fn collect(root: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    let mut entries: Vec<PathBuf> = fs::read_dir(root)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            let skip = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| SKIP_DIRS.contains(&n));
            if !skip {
                collect(&path, out)?;
            }
        } else if is_source(&path) {
            out.push(path);
        }
    }
    Ok(())
}

fn run(cfg: &Config) -> ExitCode {
    let mut rows: Vec<Row> = Vec::new();
    let mut trouble = false;

    if cfg.stdin {
        let mut src = String::new();
        if let Err(e) = io::Read::read_to_string(&mut io::stdin(), &mut src) {
            eprintln!("rustcx: <stdin>: {e}");
            return ExitCode::from(2);
        }
        trouble |= !score_into(&mut rows, "<stdin>", &src, cfg);
    }

    // Expand the paths given on the command line. A file named explicitly is
    // read whatever its extension; a directory is walked for `.rs`.
    let mut files: Vec<PathBuf> = Vec::new();
    for p in &cfg.paths {
        if p.is_dir() {
            if let Err(e) = collect(p, &mut files) {
                eprintln!("rustcx: {}: {e}", p.display());
                trouble = true;
            }
        } else {
            files.push(p.clone());
        }
    }

    for path in &files {
        match fs::read_to_string(path) {
            Ok(src) => {
                let name = path.display().to_string();
                trouble |= !score_into(&mut rows, &name, &src, cfg);
            }
            Err(e) => {
                eprintln!("rustcx: {}: {e}", path.display());
                trouble = true;
            }
        }
    }

    let summary = summarize(&rows);
    let stdout = io::stdout();
    let mut w = stdout.lock();
    if write_table(&mut w, &rows, &summary, cfg.header, cfg.explain).is_err() {
        return ExitCode::from(2);
    }

    if trouble {
        return ExitCode::from(2);
    }
    if let Some(max) = cfg.max {
        if rows.iter().any(|r| r.func.complexity > max) {
            return ExitCode::from(1);
        }
    }
    ExitCode::SUCCESS
}

/// Analyze one source and append its rows. Returns false on a parse error.
///
/// This function owns the span-lifetime invariant: `analyze` resolves every
/// span to a line before returning, so the thread-local span table — which
/// otherwise retains the full text of every file ever parsed — is cleared here.
/// Nothing borrowed from the AST may outlive this call.
fn score_into(rows: &mut Vec<Row>, name: &str, src: &str, cfg: &Config) -> bool {
    let result = rustcx::analyze::analyze(src, cfg.opts);
    proc_macro2::extra::invalidate_current_thread_spans();
    match result {
        Ok(funcs) => {
            for func in funcs {
                rows.push(Row { file: name.to_string(), func });
            }
            true
        }
        Err(e) => {
            eprintln!("rustcx: {}:{}: parse error: {}", name, e.line, e.message);
            false
        }
    }
}
```

Replace the body of `main` so it calls `run`:

```rust
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cfg = match parse_args(&args) {
        Ok(Some(cfg)) => cfg,
        Ok(None) => return ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("rustcx: {msg}");
            return ExitCode::from(2);
        }
    };
    run(&cfg)
}
```

`proc-macro2` must be a direct dependency of the binary for `score_into` to call it — it already is, from Task 1's manifest.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test`
Expected: all unit and integration tests pass.

- [ ] **Step 6: Try it on itself**

Run: `cargo run -- src/`
Expected: a table of rustcx's own functions, exit 0.

- [ ] **Step 7: Commit**

```bash
git add src/main.rs tests/cli.rs fixtures/simple.rs
git commit -m "feat: file discovery, stdin, exit codes, and span invalidation"
```

---

### Task 11: `--min`, `--top`, and `--sort`

Row selection and ordering, applied between collection and rendering.

**Files:**
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: `Row`, `Config`.
- Produces: `fn select(rows: Vec<Row>, cfg: &Config) -> Vec<Row>`.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `src/main.rs`:

```rust
    fn selected(cfg: Config, rows: Vec<Row>) -> Vec<(String, u32)> {
        select(rows, &cfg).into_iter().map(|r| (r.func.name, r.func.complexity)).collect()
    }

    fn spread() -> Vec<Row> {
        vec![
            Row { file: "a.rs".into(), func: func("low", FnKind::Fn, 1, 1, 0) },
            Row { file: "a.rs".into(), func: func("high", FnKind::Fn, 2, 9, 0) },
            Row { file: "b.rs".into(), func: func("mid", FnKind::Fn, 3, 5, 0) },
        ]
    }

    #[test]
    fn min_filters_below_the_threshold() {
        let cfg = Config { min: 5, ..Config::default() };
        assert_eq!(selected(cfg, spread()), [("high".to_string(), 9), ("mid".to_string(), 5)]);
    }

    #[test]
    fn min_of_one_keeps_everything_in_source_order() {
        let cfg = Config::default();
        let names: Vec<String> = selected(cfg, spread()).into_iter().map(|r| r.0).collect();
        assert_eq!(names, ["low", "high", "mid"]);
    }

    #[test]
    fn sort_orders_by_score_descending() {
        let cfg = Config { sort: true, ..Config::default() };
        let names: Vec<String> = selected(cfg, spread()).into_iter().map(|r| r.0).collect();
        assert_eq!(names, ["high", "mid", "low"]);
    }

    #[test]
    fn top_keeps_the_n_worst_and_restores_source_order() {
        // --top picks the worst 2, but without --sort they print in source
        // order so the table still reads file by file.
        let cfg = Config { top: Some(2), ..Config::default() };
        let names: Vec<String> = selected(cfg, spread()).into_iter().map(|r| r.0).collect();
        assert_eq!(names, ["high", "mid"]);
    }

    #[test]
    fn top_with_sort_stays_sorted() {
        let cfg = Config { top: Some(2), sort: true, ..Config::default() };
        let names: Vec<String> = selected(cfg, spread()).into_iter().map(|r| r.0).collect();
        assert_eq!(names, ["high", "mid"]);
    }
```

Note the fourth test: `high` is at line 2 and `mid` at line 3 in different files, so source order happens to match score order here. Keep the assertion as written — it documents that `--top` alone does not reorder within what it kept.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test`
Expected: FAIL to compile — `select` is undefined.

- [ ] **Step 3: Write the implementation**

Add to `src/main.rs`:

```rust
/// Filter and order rows for display.
///
/// `--min` drops low scores, `--top` keeps the N worst, and `--sort` orders by
/// score descending. Without `--sort` the surviving rows are restored to source
/// order so the table still reads file by file.
fn select(rows: Vec<Row>, cfg: &Config) -> Vec<Row> {
    let mut kept: Vec<(usize, Row)> = rows
        .into_iter()
        .enumerate()
        .filter(|(_, r)| r.func.complexity >= cfg.min)
        .collect();

    if cfg.top.is_some() || cfg.sort {
        kept.sort_by(|a, b| b.1.func.complexity.cmp(&a.1.func.complexity));
    }
    if let Some(n) = cfg.top {
        kept.truncate(n);
    }
    if !cfg.sort {
        kept.sort_by_key(|(i, _)| *i);
    }
    kept.into_iter().map(|(_, r)| r).collect()
}
```

Call it in `run`, immediately before `summarize`:

```rust
    let rows = select(rows, cfg);
    let summary = summarize(&rows);
```

The `--max` check must run against the *selected* rows to stay consistent with what is printed; leave it where it is, after `write_table`, now reading the filtered `rows`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test`
Expected: all passed.

- [ ] **Step 5: Add an integration check**

Add to `tests/cli.rs`:

```rust
#[test]
fn min_hides_trivial_functions() {
    let (stdout, _, code) = rustcx(&["--min", "2", "fixtures/simple.rs"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("branchy"));
    assert!(!stdout.contains("plain"), "plain scores 1 and should be hidden:\n{stdout}");
}
```

Run: `cargo test`
Expected: all passed.

- [ ] **Step 6: Commit**

```bash
git add src/main.rs tests/cli.rs
git commit -m "feat: --min, --top, and --sort"
```

---

### Task 12: `--json`

**Files:**
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: `Row`, `Summary`. `Func`, `FnKind`, and `Point` already derive `Serialize` from Task 1.
- Produces: `fn write_json<W: Write>(w, &[Row], &Summary) -> io::Result<()>`.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `src/main.rs`:

```rust
    #[test]
    fn json_carries_every_row_and_the_summary() {
        let mut rows = sample();
        rows[0].func.points = vec![Point { line: 13, branch: "if" }];
        let mut out = Vec::new();
        let s = summarize(&rows);
        write_json(&mut out, &rows, &s).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();

        assert_eq!(v["functions"].as_array().unwrap().len(), 3);
        assert_eq!(v["functions"][0]["file"], "src/a.rs");
        assert_eq!(v["functions"][0]["name"], "validate");
        assert_eq!(v["functions"][0]["kind"], "fn");
        assert_eq!(v["functions"][0]["complexity"], 14);
        assert_eq!(v["functions"][0]["line"], 12);
        assert_eq!(v["functions"][0]["depth"], 0);
        assert_eq!(v["functions"][0]["points"][0]["branch"], "if");
        assert_eq!(v["functions"][1]["kind"], "closure");

        assert_eq!(v["summary"]["functions"], 3);
        assert_eq!(v["summary"]["files"], 2);
        assert_eq!(v["summary"]["worst"]["name"], "validate");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test`
Expected: FAIL to compile — `write_json` is undefined.

- [ ] **Step 3: Write the implementation**

Add to `src/main.rs`, with `use serde::Serialize;` added to the imports:

```rust
#[derive(Serialize)]
struct JsonFunc<'a> {
    file: &'a str,
    #[serde(flatten)]
    func: &'a Func,
}

#[derive(Serialize)]
struct JsonWorst<'a> {
    file: &'a str,
    name: &'a str,
    line: u32,
    complexity: u32,
}

#[derive(Serialize)]
struct JsonSummary<'a> {
    functions: usize,
    files: usize,
    mean: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    worst: Option<JsonWorst<'a>>,
}

#[derive(Serialize)]
struct JsonReport<'a> {
    functions: Vec<JsonFunc<'a>>,
    summary: JsonSummary<'a>,
}

fn write_json<W: Write>(w: &mut W, rows: &[Row], summary: &Summary) -> io::Result<()> {
    let report = JsonReport {
        functions: rows
            .iter()
            .map(|r| JsonFunc { file: &r.file, func: &r.func })
            .collect(),
        summary: JsonSummary {
            functions: summary.funcs,
            files: summary.files,
            mean: summary.mean,
            worst: summary.worst.as_ref().map(|r| JsonWorst {
                file: &r.file,
                name: &r.func.name,
                line: r.func.line,
                complexity: r.func.complexity,
            }),
        },
    };
    serde_json::to_writer(&mut *w, &report).map_err(io::Error::other)?;
    writeln!(w)
}
```

`#[serde(flatten)]` needs no extra feature — it is part of `serde_derive`, already enabled.

Branch on it in `run`, replacing the `write_table` call:

```rust
    let wrote = if cfg.json {
        write_json(&mut w, &rows, &summary)
    } else {
        write_table(&mut w, &rows, &summary, cfg.header, cfg.explain)
    };
    if wrote.is_err() {
        return ExitCode::from(2);
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test`
Expected: all passed.

- [ ] **Step 5: Add an integration check**

Add to `tests/cli.rs`:

```rust
#[test]
fn json_output_parses() {
    let (stdout, _, code) = rustcx(&["--json", "fixtures/simple.rs"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert!(v["functions"].as_array().unwrap().len() >= 3);
}
```

`serde_json` must be available to the integration test. Add to `Cargo.toml`:

```toml
[dev-dependencies]
serde_json = "1.0"
```

Run: `cargo test`
Expected: all passed.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml src/main.rs tests/cli.rs
git commit -m "feat: --json output"
```

---

### Task 13: Fixture sweep

A broad set of hand-checked files exercising Rust syntax that a token-level tool would trip on, asserted end to end. This is where a regression in any rule surfaces.

**Files:**
- Create: `fixtures/nesting.rs`, `fixtures/patterns.rs`, `fixtures/syntax.rs`
- Create: `tests/fixtures.rs`

**Interfaces:**
- Consumes: `rustcx::analyze::{analyze, Options}` as a library.

- [ ] **Step 1: Write the fixtures**

Create `fixtures/nesting.rs`:

```rust
// Hand-checked expected scores are asserted in tests/fixtures.rs.
pub struct Engine {
    limit: u8,
}

impl Engine {
    // 1 + if + && = 3
    pub fn allows(&self, n: u8, open: bool) -> bool {
        if n < self.limit && open {
            return true;
        }
        false
    }

    // 1 + for = 2. The closure is a separate row.
    pub fn tally(&self, xs: &[u8]) -> usize {
        let mut n = 0;
        for _x in xs {
            n += 1;
        }
        xs.iter().filter(|x| **x > self.limit || **x == 0).count() + n
    }
}

pub trait Policy {
    fn name(&self) -> &str;
    // 1 + if = 2
    fn check(&self, ok: bool) -> bool {
        if ok {
            true
        } else {
            false
        }
    }
}

// 1 + if = 2, and the nested fn is its own row at 1 + if = 2.
pub fn outer(a: bool) -> u8 {
    fn inner(b: bool) -> u8 {
        if b {
            1
        } else {
            0
        }
    }
    if a {
        inner(a)
    } else {
        0
    }
}
```

Create `fixtures/patterns.rs`:

```rust
pub enum Token {
    Word(String),
    Num(i64),
    Open,
    Close,
    End,
}

// 1 base
// + arm Word (1) + guard (1) + the `&&` inside the guard (1)
// + arm Num (1)
// + arm Open|Close (1 arm + 1 alternative)
// + arm End (1)
// + catch-all (0)
// = 8
pub fn describe(t: Token, strict: bool) -> &'static str {
    match t {
        Token::Word(w) if strict && !w.is_empty() => "word",
        Token::Num(_) => "num",
        Token::Open | Token::Close => "bracket",
        Token::End => "end",
        _ => "other",
    }
}

// 1 + let-else + if let + alternative = 4
pub fn unwrap_pair(a: Option<Token>, b: Option<u8>) -> u8 {
    let Some(_x) = b else {
        return 0;
    };
    if let Some(Token::Open | Token::Close) = a {
        return 1;
    }
    2
}

// 1 base; `?` is free by default.
pub fn parse(s: &str) -> Result<i64, std::num::ParseIntError> {
    let n: i64 = s.trim().parse()?;
    let m: i64 = s.trim().parse()?;
    Ok(n + m)
}
```

Create `fixtures/syntax.rs`:

```rust
//! Syntax that trips token-level scanners: raw strings, nested block comments,
//! lifetimes against char literals, generics against shift operators.

/* a block comment /* nested inside another */ still a comment */

pub struct Holder<'a> {
    pub text: &'a str,
}

impl<'a> Holder<'a> {
    // 1 + if = 2. The raw string contains `if` and `&&` as text, not code.
    pub fn shout(&self) -> String {
        let banner = r#"if a && b { "quoted" }"#;
        if self.text.is_empty() {
            return banner.to_string();
        }
        self.text.to_uppercase()
    }
}

// 1 + if + && = 3. `'x'` is a char literal, `<u8>` is a generic, `>>` is a
// shift — none of which is a decision point.
pub fn ambiguous(n: u32, c: char) -> u32 {
    let v = Vec::<Vec<u8>>::new();
    if c == 'x' && v.is_empty() {
        return n >> 2;
    }
    n
}

// A raw identifier is still a name. `Ident::to_string` keeps the `r#`, so this
// function is reported as `r#match`.
pub fn r#match(a: bool) -> bool {
    a
}

macro_rules! ignored {
    ($x:expr) => {
        if $x {
            1
        } else {
            0
        }
    };
}

// 1 base. The branch inside the macro invocation is invisible to syn — a
// documented limitation, asserted here so it is a deliberate behavior.
pub fn uses_macro(a: bool) -> u8 {
    ignored!(a)
}
```

- [ ] **Step 2: Write the failing test**

Create `tests/fixtures.rs`:

```rust
use rustcx::analyze::{analyze, Options};
use std::collections::BTreeMap;

fn score_file(path: &str, opts: Options) -> BTreeMap<String, u32> {
    let src = std::fs::read_to_string(path).expect("fixture should exist");
    analyze(&src, opts)
        .expect("fixture should parse")
        .into_iter()
        .map(|f| (f.name, f.complexity))
        .collect()
}

fn expect(path: &str, pairs: &[(&str, u32)]) {
    let got = score_file(path, Options::default());
    for (name, want) in pairs {
        assert_eq!(
            got.get(*name),
            Some(want),
            "{path}: {name} should score {want}; full result: {got:?}"
        );
    }
}

#[test]
fn nesting_fixture() {
    expect(
        "fixtures/nesting.rs",
        &[
            ("Engine::allows", 3),
            ("Engine::tally", 2),
            ("filter", 2),
            ("Policy::check", 2),
            ("outer", 2),
            ("inner", 2),
        ],
    );
}

#[test]
fn patterns_fixture() {
    expect(
        "fixtures/patterns.rs",
        &[("describe", 8), ("unwrap_pair", 4), ("parse", 1)],
    );
}

#[test]
fn question_flag_changes_only_the_question_operator() {
    let with = score_file("fixtures/patterns.rs", Options { question: true, ..Options::default() });
    assert_eq!(with.get("parse"), Some(&3), "two ? operators");
    assert_eq!(with.get("describe"), Some(&8), "unchanged by --question");
}

#[test]
fn syntax_fixture_is_not_confused_by_lexical_traps() {
    expect(
        "fixtures/syntax.rs",
        &[("Holder::shout", 2), ("ambiguous", 3), ("r#match", 1), ("uses_macro", 1)],
    );
}

#[test]
fn nested_functions_report_their_depth() {
    let src = std::fs::read_to_string("fixtures/nesting.rs").unwrap();
    let funcs = analyze(&src, Options::default()).unwrap();
    let inner = funcs.iter().find(|f| f.name == "inner").unwrap();
    assert_eq!(inner.depth, 1);
}
```

- [ ] **Step 3: Run tests to verify they fail or reveal a real bug**

Run: `cargo test --test fixtures`
Expected: these may pass immediately, since the analyzer is complete. If any fail, the fixture comment and the implementation disagree — work out which is right by hand before changing either. That is the point of this task.

- [ ] **Step 4: Reconcile**

For any mismatch, count the decision points by hand against the spec's scoring table. Fix the analyzer if it is wrong; fix the fixture's comment and expectation if the hand count was wrong. Do not adjust a number just to make a test green.

- [ ] **Step 5: Run the full suite**

Run: `cargo test && cargo clippy -- -D warnings`
Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add fixtures/ tests/fixtures.rs src/analyze.rs
git commit -m "test: hand-checked fixtures for nesting, patterns, and lexical traps"
```

---

### Task 14: `--explain` integration and the CI gate end to end

The last behavioral gap: `--explain` has a unit test but no end-to-end check, and the `--max` workflow deserves one against a realistic file.

**Files:**
- Modify: `tests/cli.rs`

**Interfaces:**
- Consumes: everything built so far. No new production code is expected; if a test fails, fix the source.

- [ ] **Step 1: Write the tests**

Add to `tests/cli.rs`:

```rust
#[test]
fn explain_lists_branches_that_sum_to_the_score() {
    let (stdout, _, code) = rustcx(&["--explain", "--min", "2", "fixtures/patterns.rs"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("match arm"), "got:\n{stdout}");
    assert!(stdout.contains("guard"), "got:\n{stdout}");
    assert!(stdout.contains("pattern alternative"), "got:\n{stdout}");

    // describe scores 8, so it lists 7 decision points — the score is always
    // 1 + the number of explain lines.
    let after = stdout.split("describe").nth(1).unwrap();
    let listed = after
        .lines()
        .skip(1)
        .take_while(|l| l.starts_with("      "))
        .count();
    assert_eq!(listed, 7, "explain lines under describe:\n{after}");
}

#[test]
fn question_flag_reaches_the_cli() {
    let (plain, _, _) = rustcx(&["--json", "fixtures/patterns.rs"]);
    let (with, _, _) = rustcx(&["--json", "--question", "fixtures/patterns.rs"]);
    let p: serde_json::Value = serde_json::from_str(&plain).unwrap();
    let q: serde_json::Value = serde_json::from_str(&with).unwrap();
    let find = |v: &serde_json::Value| -> u64 {
        v["functions"].as_array().unwrap().iter()
            .find(|f| f["name"] == "parse").unwrap()["complexity"].as_u64().unwrap()
    };
    assert_eq!(find(&p), 1);
    assert_eq!(find(&q), 3);
}

#[test]
fn no_tests_flag_reaches_the_cli() {
    let dir = std::env::temp_dir().join("rustcx-notests");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("m.rs"),
        "pub fn real() {}\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {}\n}\n",
    )
    .unwrap();

    let (with, _, _) = rustcx(&[dir.to_str().unwrap()]);
    assert!(with.contains("2 functions"), "got:\n{with}");
    let (without, _, _) = rustcx(&["--no-tests", dir.to_str().unwrap()]);
    assert!(without.contains("1 function in"), "got:\n{without}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_ci_gate_reads_naturally() {
    // The documented CI invocation: clean under a generous threshold, failing
    // under a strict one, with the same output either way.
    let (_, _, clean) = rustcx(&["--max", "10", "fixtures"]);
    assert_eq!(clean, 0);
    let (_, _, breach) = rustcx(&["--max", "2", "fixtures"]);
    assert_eq!(breach, 1);
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo test --test cli`
Expected: PASS. If `explain_lists_branches_that_sum_to_the_score` fails on the line count, check that `write_table`'s explain lines start with exactly six spaces, per Task 9.

- [ ] **Step 3: Run everything**

Run: `cargo test && cargo clippy -- -D warnings && cargo build --release`
Expected: all green, and a binary at `target/release/rustcx`.

- [ ] **Step 4: Commit**

```bash
git add tests/cli.rs
git commit -m "test: end-to-end coverage for --explain, --question, --no-tests, and the CI gate"
```

---

### Task 15: README

**Files:**
- Create: `README.md`

**Interfaces:**
- Consumes: the finished binary, for the real output samples.

- [ ] **Step 1: Capture real output**

Run: `cargo run --release -- src/ | head -30`

Copy the actual output into the README rather than inventing a sample. Do the same for `cargo run --release -- --top 5 --explain src/`.

- [ ] **Step 2: Write the README**

Create `README.md` following the structure of `../jscx/README.md`, with these sections:

1. **Title and one-paragraph pitch** — cyclomatic complexity for Rust functions, single binary, plus the real `rustcx src/` output captured in step 1.
2. **Install** — `cargo install --path .`
3. **Usage** — the two invocation forms and the full options table, copied from the spec's CLI section.
4. **As a CI gate** — `rustcx --max 15 src/`, and the exit-code table (0 clean, 1 over `--max`, 2 unreadable or unparseable).
5. **Finding what to refactor first** — `rustcx --top 20 --explain src/`, with real output.
6. **Scoring** — the full decision-point table from the spec, then the three consequences worth knowing: nested functions and closures are scored independently; `?` is free by default and why (it is a conditional `return`, and `return` scores 0); a trailing catch-all `match` arm is free, like `default` in a `switch`.
7. **Comparison to jscx** — one short paragraph: the rules follow ESLint's `complexity` where the languages line up, so scores are directly comparable.
8. **How it works** — `syn` parse, a `Visit` walker with a stack of open functions, complexity derived as `1 + points.len()`. Mention the span-invalidation step, since it is surprising.
9. **Limitations** — all five from the spec, verbatim in substance: opaque macro bodies, a file that fails to parse yields no rows, branches outside a function are unscored, `#[cfg]`-disabled code is scored anyway (and that `--no-tests` only recognises the bare `#[cfg(test)]` form), and the blunt-instrument caveat.
10. **Development** — `cargo test`, `cargo run -- fixtures/ --explain`, `cargo clippy`, and a note that `fixtures/` holds hand-checked samples.

- [ ] **Step 3: Verify every command in the README actually runs**

Run each command block in the README and confirm the output matches what is documented.

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "docs: README with usage, scoring rules, and limitations"
```

---

## Done

At this point `rustcx` scores every function in a Rust tree, gates CI, explains its numbers, and emits JSON. The analyzer's public API (`analyze`, `Options`, `Func`) matches jscx's, so the out-of-scope work — `--lsp` and the Zed, JetBrains, and Neovim extensions — can be ported without touching `src/analyze.rs`.
