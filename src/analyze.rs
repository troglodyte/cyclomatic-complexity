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
}

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
        // methods are not discovered until Task 5.
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
}
