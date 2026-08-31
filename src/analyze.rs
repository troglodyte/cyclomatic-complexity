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
    #[allow(dead_code)]
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
