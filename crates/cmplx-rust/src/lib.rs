//! Rust support for cmplx: rustcx's syn-based analyzer behind `Language`.

pub mod analyze;
mod nesting;

use cmplx_core::language::{LangOptions, Language, OptionSpec};
use cmplx_core::model::{Func, Kind, ParseError, ParseErrorKind, Point};
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
    fn first_line_deeper_than(&self, src: &str, _path: &Path, _opts: &LangOptions, limit: usize) -> Option<u32> {
        nesting::first_line_deeper_than(src, limit)
    }
    fn analyze(&self, src: &str, _path: &Path, opts: &LangOptions) -> Result<Vec<Func>, ParseError> {
        let options = analyze::Options {
            logical: opts.flag(&LOGICAL).unwrap_or(true),
            question: opts.flag(&QUESTION).unwrap_or(false),
            tests: opts.flag(&TESTS).unwrap_or(true),
        };
        let result = on_parse_stack(|| {
            let result = analyze::analyze(src, options).map_err(|e| ParseError {
                line: e.line,
                message: e.message,
                kind: ParseErrorKind::Syntax,
            });
            // Every span is already a line number, so the thread-local span
            // table (which otherwise keeps each parsed file's text alive) can
            // be dropped, on whichever thread parsed. Callers of
            // `Language::analyze` cannot reach proc-macro2 themselves.
            proc_macro2::extra::invalidate_current_thread_spans();
            result
        });
        result.map(|funcs| funcs.into_iter().map(core_func).collect())
    }
}

/// Stack for the thread `syn` parses on, so that core's 100-level nesting cap
/// fits with a wide margin whatever thread calls `analyze` (see the
/// measurements in `nesting`). Only the pages a parse touches are committed.
const PARSE_STACK_BYTES: usize = 16 << 20;

/// Run `parse` on a thread with a [`PARSE_STACK_BYTES`] stack and wait for it.
/// If no thread can be started it runs on the caller's: the cap still holds
/// there, with the smaller margin of the caller's stack. A panic is passed on
/// unchanged.
fn on_parse_stack<T: Send>(parse: impl Fn() -> T + Sync) -> T {
    std::thread::scope(|scope| {
        let spawned = std::thread::Builder::new()
            .name("cmplx-rust-parse".into())
            .stack_size(PARSE_STACK_BYTES)
            .spawn_scoped(scope, &parse);
        match spawned {
            Ok(handle) => handle.join().unwrap_or_else(|p| std::panic::resume_unwind(p)),
            Err(_) => parse(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmplx_core::language::{analyze_bounded, MAX_NESTING};

    /// Score as core does: refused past the nesting cap, else analyzed.
    fn analyze_with(src: &str, opts: &LangOptions) -> Result<Vec<Func>, ParseError> {
        analyze_bounded(&RUST, src, Path::new("a.rs"), opts)
    }

    fn deeper_than_cap(src: &str) -> Option<u32> {
        RUST.first_line_deeper_than(src, Path::new("a.rs"), &LangOptions::default(), MAX_NESTING)
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
        assert_eq!(err.kind, ParseErrorKind::Syntax);
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

    /// A file whose deepest delimiter nesting is `depth`: `fn f() {` is one
    /// level and each `(` inside the expression adds one.
    fn nested_parens(depth: usize) -> String {
        let inner = depth - 1;
        format!("fn f() {{ let _ = {}1{}; }}", "(".repeat(inner), ")".repeat(inner))
    }

    /// Run `f` on a thread with Rust's default 2 MB spawned-thread stack.
    fn on_small_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(f)
            .unwrap()
            .join()
            .unwrap()
    }

    #[test]
    fn nesting_exactly_at_the_cap_scores_normally_on_a_2mb_stack() {
        let funcs = on_small_stack(|| analyze_with(&nested_parens(MAX_NESTING), &LangOptions::default()));
        assert_eq!(funcs.unwrap().len(), 1);
    }

    /// Closures use the most stack per delimiter level measured (see
    /// `nesting`): far too much for 100 levels on a 2 MB stack in a debug
    /// build, which is why the parse gets its own thread.
    #[test]
    fn the_costliest_nesting_measured_at_the_cap_scores_normally_on_a_2mb_stack() {
        let levels = MAX_NESTING - 1;
        let src = format!("fn f() {{ {}1{} }}", "let _ = || { ".repeat(levels), " };".repeat(levels));
        assert_eq!(deeper_than_cap(&src), None);
        assert!(deeper_than_cap(&format!("({src})")).is_some(), "the input is at the cap");
        let funcs = on_small_stack(move || analyze_with(&src, &LangOptions::default()));
        assert_eq!(funcs.unwrap().len(), levels + 1);
    }

    #[test]
    fn nesting_one_past_the_cap_is_refused_at_the_line_it_first_exceeds() {
        let src = format!("\n\n{}", nested_parens(MAX_NESTING + 1));
        assert_eq!(deeper_than_cap(&src), Some(3));
        let err = analyze_with(&src, &LangOptions::default()).unwrap_err();
        assert_eq!(err.line, 3);
        assert_eq!(err.kind, ParseErrorKind::TooDeep { limit: MAX_NESTING });
    }

    #[test]
    fn nesting_100k_deep_is_refused_quickly_without_overflowing_a_2mb_stack() {
        let started = std::time::Instant::now();
        let err = on_small_stack(|| analyze_with(&nested_parens(100_000), &LangOptions::default()));
        assert_eq!(err.unwrap_err().line, 1);
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
    }

    #[test]
    fn delimiters_inside_literals_and_comments_do_not_count() {
        let deep = "(".repeat(MAX_NESTING * 2);
        let src = format!(
            "fn f() {{\n\
             // {deep}\n\
             /* {deep} /* nested */ {deep} */\n\
             let _ = \"{deep}\";\n\
             let _ = r#\"{deep}\"#;\n\
             let _ = b\"{deep}\";\n\
             let _ = ['(', '[', '{{'];\n\
             }}"
        );
        let funcs = analyze_with(&src, &LangOptions::default());
        assert_eq!(funcs.map(|f| f.len()), Ok(1));
    }

    /// Token-shape chains are no longer refused before parsing: only bracket
    /// depth is. Each source sits one past the cap a removed heuristic had,
    /// and well below where `syn` overflowed on the 16 MB parse thread.
    #[test]
    fn chains_past_the_removed_heuristic_caps_are_scored() {
        let shapes: [(&str, String); 6] = [
            ("prefix operators", format!("type T = {}u8;", "&".repeat(101))),
            ("generics", format!("type T = {}u8{};", "Vec<".repeat(71), ">".repeat(71))),
            ("arrows", format!("type T = {}u8;", "fn() -> ".repeat(91))),
            ("closures", format!("fn f() {{ {}x; }}", "|a| ".repeat(201))),
            ("operators", format!("fn f() {{ a{}; }}", " + a".repeat(6_001))),
            ("else if", format!("fn f() {{ if a {{}} {} }}", "else if a {} ".repeat(6_001))),
        ];
        for (name, src) in shapes {
            let result = on_small_stack(move || analyze_with(&src, &LangOptions::default()));
            assert!(result.is_ok(), "{name}: {:?}", result.map(|f| f.len()));
        }
    }

    /// Ordinary code that is long but not deep must still be scored.
    #[test]
    fn long_but_shallow_files_are_not_refused() {
        let derives = "#[derive(Debug, Clone)]\nstruct S { a: Vec<u8>, b: Option<&'static str> }\n".repeat(3_000);
        let comparisons = format!("fn f(a: u8, b: u8) {{ {} }}", "if a < b { g(); } ".repeat(3_000));
        let guards = format!(
            "fn f(x: u8, y: u8) -> u8 {{ match x {{ {} _ => 0 }} }}",
            "1 if x < y => 1, ".repeat(3_000)
        );
        let arms = format!("fn f(x: u8) {{ match x {{ {} _ => {{}} }} }}", "1 => {} ".repeat(30_000));
        let items = "fn f() -> impl Fn() -> u8 { || 0 }\n".repeat(3_000);
        let statements = format!("fn f() {{ {} }}", "let a = &mut *b..c; return; ".repeat(3_000));
        for src in [derives, comparisons, guards, arms, items, statements] {
            let result = analyze_with(&src, &LangOptions::default());
            assert!(result.is_ok(), "{:?}: {}", result.map(|f| f.len()), &src[..80]);
        }
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
