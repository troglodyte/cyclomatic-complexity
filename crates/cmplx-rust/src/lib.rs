//! Rust support for cmplx: rustcx's syn-based analyzer behind `Language`.

pub mod analyze;

use cmplx_core::language::{LangOptions, Language, OptionSpec};
use cmplx_core::model::{Func, Kind, ParseError, Point};
use std::path::Path;

const KIND_FN: Kind = Kind { id: "fn", label: "fn" };
const KIND_METHOD: Kind = Kind { id: "method", label: "method" };
const KIND_TRAIT_FN: Kind = Kind { id: "trait_fn", label: "trait fn" };
const KIND_CLOSURE: Kind = Kind { id: "closure", label: "closure" };

const LOGICAL: OptionSpec = OptionSpec {
    key: "logical",
    on_flag: None,
    off_flag: Some("--no-logical"),
    default: Some(true),
    help: "do not count && and ||",
};
const QUESTION: OptionSpec = OptionSpec {
    key: "question",
    on_flag: Some("--question"),
    off_flag: None,
    default: Some(false),
    help: "count the ? operator",
};
const TESTS: OptionSpec = OptionSpec {
    key: "tests",
    on_flag: None,
    off_flag: Some("--no-tests"),
    default: Some(true),
    help: "do not score #[test] functions or #[cfg(test)] modules",
};

static OPTIONS: [OptionSpec; 3] = [LOGICAL, QUESTION, TESTS];

pub struct Rust;
pub static RUST: Rust = Rust;

fn kind_of(kind: analyze::FnKind) -> Kind {
    match kind {
        analyze::FnKind::Fn => KIND_FN,
        analyze::FnKind::Method => KIND_METHOD,
        analyze::FnKind::TraitFn => KIND_TRAIT_FN,
        analyze::FnKind::Closure => KIND_CLOSURE,
    }
}

fn core_func(func: analyze::Func) -> Func {
    Func {
        name: func.name,
        kind: kind_of(func.kind),
        line: func.line,
        complexity: func.complexity,
        depth: func.depth,
        points: func
            .points
            .into_iter()
            .map(|p| Point { line: p.line, branch: p.branch })
            .collect(),
    }
}

impl Language for Rust {
    fn id(&self) -> &'static str {
        "rust"
    }
    fn display_name(&self) -> &'static str {
        "Rust"
    }
    fn alias(&self) -> Option<&'static str> {
        Some("rustcx")
    }
    fn extensions(&self) -> &'static [&'static str] {
        &["rs"]
    }
    fn stdin_ext(&self) -> &'static str {
        "rs"
    }
    fn options(&self) -> &'static [OptionSpec] {
        &OPTIONS
    }
    fn skip(&self, _path: &Path, _opts: &LangOptions) -> bool {
        false
    }
    fn analyze(&self, src: &str, _path: &Path, opts: &LangOptions) -> Result<Vec<Func>, ParseError> {
        let options = analyze::Options {
            logical: opts.flag(&LOGICAL).unwrap_or(true),
            question: opts.flag(&QUESTION).unwrap_or(false),
            tests: opts.flag(&TESTS).unwrap_or(true),
        };
        let result = analyze::analyze(src, options);
        // Every span is already a line number, so the thread-local span table
        // (which otherwise keeps each parsed file's text alive) can be dropped.
        // Callers of `Language::analyze` cannot reach proc-macro2 themselves.
        proc_macro2::extra::invalidate_current_thread_spans();
        result
            .map(|funcs| funcs.into_iter().map(core_func).collect())
            .map_err(|e| ParseError { line: e.line, message: e.message })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analyze_with(src: &str, opts: &LangOptions) -> Result<Vec<Func>, ParseError> {
        RUST.analyze(src, Path::new("a.rs"), opts)
    }

    #[test]
    fn each_option_help_says_what_its_listed_flags_do() {
        let help: Vec<(Option<&str>, Option<&str>, &str)> = RUST
            .options()
            .iter()
            .map(|o| (o.on_flag, o.off_flag, o.help))
            .collect();
        assert_eq!(
            help,
            [
                (None, Some("--no-logical"), "do not count && and ||"),
                (Some("--question"), None, "count the ? operator"),
                (None, Some("--no-tests"), "do not score #[test] functions or #[cfg(test)] modules"),
            ]
        );
    }

    #[test]
    fn trait_fn_keeps_its_two_spellings() {
        let funcs = analyze_with("trait T { fn f(&self) { if true {} } }", &LangOptions::default()).unwrap();
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].kind, Kind { id: "trait_fn", label: "trait fn" });
        assert_eq!(funcs[0].complexity, 2);
    }

    #[test]
    fn question_option_is_honoured() {
        let src = "fn f() -> Option<()> { None?; Some(()) }";
        let off = analyze_with(src, &LangOptions::default()).unwrap();
        let mut on_opts = LangOptions::default();
        on_opts.set("question", true);
        let on = analyze_with(src, &on_opts).unwrap();
        assert_eq!((off[0].complexity, on[0].complexity), (1, 2));
    }

    #[test]
    fn a_parse_error_carries_its_line() {
        let err = analyze_with("fn f( {", &LangOptions::default()).unwrap_err();
        assert_eq!(err.line, 1);
    }

    #[test]
    fn logical_false_drops_the_and_operator_point() {
        let src = "fn f(a: bool, b: bool) { if a && b {} }";
        let mut off = LangOptions::default();
        off.set("logical", false);
        let default_score = analyze_with(src, &LangOptions::default()).unwrap()[0].complexity;
        let off_score = analyze_with(src, &off).unwrap()[0].complexity;
        assert_eq!((default_score, off_score), (3, 2));
    }

    // proc-macro2's span table is private, so invalidation cannot be observed
    // directly. This guards what it must not break: repeated analyses on one
    // thread (the LSP case) stay correct, including a later direct `analyze`.
    #[test]
    fn repeated_analyses_on_one_thread_stay_correct() {
        let src = "\n\nfn first() {}\nfn second() { if true {} }";
        let baseline = analyze_with(src, &LangOptions::default()).unwrap();
        for _ in 0..5 {
            assert_eq!(analyze_with(src, &LangOptions::default()).unwrap(), baseline);
        }
        let direct = analyze::analyze(src, analyze::Options::default()).unwrap();
        assert_eq!(direct[0].line, 3);
        assert_eq!(baseline[0].line, 3);
    }
}
