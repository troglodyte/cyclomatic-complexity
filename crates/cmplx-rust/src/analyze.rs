//! Cyclomatic complexity for Rust functions.
//!
//! Every function starts at 1 and gains a point per decision point. Complexity
//! is always derived as `1 + points.len()` so a score and its `--explain`
//! listing can never disagree.

use proc_macro2::Span;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
#[derive(Clone, Debug)]
pub struct Point {
    pub line: u32,
    pub branch: &'static str,
}

#[derive(Clone, Debug)]
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
/// afterwards. The `Language` wrapper in `lib.rs` owns that call.
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
    /// Enclosing `impl` or `trait` type names, for qualifying method names.
    type_stack: Vec<String>,
    /// Name to give the next closure visited. Set by the parent expression
    /// immediately before descending into the closure, consumed by
    /// `visit_expr_closure`.
    closure_name: Option<String>,
}

impl Walker {
    fn new(opts: Options) -> Self {
        Walker {
            opts,
            funcs: Vec::new(),
            stack: Vec::new(),
            type_stack: Vec::new(),
            closure_name: None,
        }
    }

    /// `Type::method` when inside an impl or trait, otherwise the bare name.
    /// Module paths are deliberately not prefixed — the file path is already
    /// printed above the table.
    fn qualified(&self, ident: &syn::Ident) -> String {
        match self.type_stack.last() {
            Some(ty) => format!("{}::{}", ty, ident),
            None => ident.to_string(),
        }
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
        if !self.opts.tests && has_test_attr(&node.attrs) {
            return;
        }
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

    fn visit_expr_try(&mut self, node: &'ast syn::ExprTry) {
        // `?` is a conditional early return. `return` scores 0 here, matching
        // ESLint's complexity rule, so `?` follows it and is opt-in.
        if self.opts.question {
            self.point(node.question_token.spans[0], "?");
        }
        visit::visit_expr_try(self, node);
    }

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
        if !self.opts.tests && has_test_attr(&node.attrs) {
            return;
        }
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
        let name = self.closure_name.take().unwrap_or_else(|| "closure".to_string());
        self.open(name, FnKind::Closure, node.inputs_begin.spans[0]);
        visit::visit_expr_closure(self, node);
        self.close();
    }

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

    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        if !self.opts.tests && is_cfg_test(&node.attrs) {
            return;
        }
        visit::visit_item_mod(self, node);
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

    fn rows(src: &str) -> Vec<(String, &'static str, u32, u32)> {
        analyze(src, Options::default())
            .unwrap()
            .into_iter()
            .map(|f| (f.name, f.kind.label(), f.complexity, f.depth))
            .collect()
    }

    fn names(src: &str) -> Vec<String> {
        analyze(src, Options::default()).unwrap().into_iter().map(|f| f.name).collect()
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
        assert_eq!(names, ["a", "_c", "b"]);
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
}
