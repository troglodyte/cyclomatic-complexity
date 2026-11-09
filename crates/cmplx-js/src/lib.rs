//! JavaScript/TypeScript support for cmplx: jscx's lexer and analyzer behind `Language`.

pub mod analyze;
pub mod lexer;

use cmplx_core::language::{LangOptions, Language, OptionSpec};
use cmplx_core::model::{Func, Kind, ParseError, Point};
use std::path::Path;

const LOGICAL: OptionSpec = OptionSpec {
    key: "logical",
    on_flag: None,
    off_flag: Some("--no-logical"),
    default: Some(true),
    help: "do not count &&, || and ?? (or their assignment forms)",
};
const OPTIONAL_CHAIN: OptionSpec = OptionSpec {
    key: "optional-chain",
    on_flag: Some("--optional-chain"),
    off_flag: None,
    default: Some(false),
    help: "count ?. as a branch",
};
const JSX: OptionSpec = OptionSpec {
    key: "jsx",
    on_flag: Some("--jsx"),
    off_flag: Some("--no-jsx"),
    default: None,
    help: "force JSX lexing on or off (default: on, except for .ts/.mts/.cts)",
};
const INCLUDE_DTS: OptionSpec = OptionSpec {
    key: "include-dts",
    on_flag: Some("--include-dts"),
    off_flag: None,
    default: Some(false),
    help: "also scan .d.ts/.d.mts/.d.cts declaration files",
};

static OPTIONS: [OptionSpec; 4] = [LOGICAL, OPTIONAL_CHAIN, JSX, INCLUDE_DTS];

pub struct Js;
pub static JS: Js = Js;

fn kind_of(kind: analyze::FnKind) -> Kind {
    let name = kind.as_str();
    Kind { id: name, label: name }
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

fn extension_of(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// JSX lexing is on everywhere except `.ts`/`.mts`/`.cts`, where `<` is always
/// a comparison or a type argument.
fn jsx_default(extension: &str) -> bool {
    !matches!(extension, "ts" | "mts" | "cts")
}

/// Whether to lex `path` as JSX: the `jsx` option, else by extension.
fn jsx_for(path: &Path, opts: &LangOptions) -> bool {
    opts.flag(&JSX)
        .unwrap_or_else(|| jsx_default(&extension_of(path)))
}

impl Language for Js {
    fn id(&self) -> &'static str {
        "js"
    }
    fn display_name(&self) -> &'static str {
        "JavaScript/TypeScript"
    }
    fn alias(&self) -> Option<&'static str> {
        Some("jscx")
    }
    fn extensions(&self) -> &'static [&'static str] {
        &["js", "mjs", "cjs", "jsx", "ts", "tsx", "mts", "cts"]
    }
    fn stdin_ext(&self) -> &'static str {
        "tsx"
    }
    fn options(&self) -> &'static [OptionSpec] {
        &OPTIONS
    }
    fn skip(&self, path: &Path, opts: &LangOptions) -> bool {
        if opts.flag(&INCLUDE_DTS).unwrap_or(false) {
            return false;
        }
        path.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| {
                name.ends_with(".d.ts") || name.ends_with(".d.mts") || name.ends_with(".d.cts")
            })
    }
    /// Brackets counted on the lexer's tokens, so strings, template text,
    /// regexes, comments and JSX text are already out of the way. A closer
    /// with no opener is ignored. Template `${` substitutions are not tokens
    /// and do not count; brackets inside them do.
    fn first_line_deeper_than(&self, src: &str, path: &Path, opts: &LangOptions, limit: usize) -> Option<u32> {
        let mut depth = 0usize;
        for token in lexer::lex(src, jsx_for(path, opts)) {
            if token.kind != lexer::Kind::Punct {
                continue;
            }
            match token.text {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" | "}" => depth = depth.saturating_sub(1),
                _ => continue,
            }
            if depth > limit {
                return Some(token.line);
            }
        }
        None
    }
    fn analyze(&self, src: &str, path: &Path, opts: &LangOptions) -> Result<Vec<Func>, ParseError> {
        let jsx = jsx_for(path, opts);
        let options = analyze::Options {
            logical: opts.flag(&LOGICAL).unwrap_or(true),
            optional_chain: opts.flag(&OPTIONAL_CHAIN).unwrap_or(false),
        };
        Ok(analyze::analyze(src, jsx, options)
            .into_iter()
            .map(core_func)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmplx_core::language::{analyze_bounded, MAX_NESTING};
    use cmplx_core::model::ParseErrorKind;

    fn analyze_as(src: &str, name: &str, opts: &LangOptions) -> Vec<Func> {
        JS.analyze(src, Path::new(name), opts).unwrap()
    }

    fn opts_with(key: &'static str, value: bool) -> LangOptions {
        let mut opts = LangOptions::default();
        opts.set(key, value);
        opts
    }

    #[test]
    fn each_option_help_says_what_its_listed_flags_do() {
        let help: Vec<(Option<&str>, Option<&str>, &str)> = JS
            .options()
            .iter()
            .map(|o| (o.on_flag, o.off_flag, o.help))
            .collect();
        assert_eq!(
            help,
            [
                (None, Some("--no-logical"), "do not count &&, || and ?? (or their assignment forms)"),
                (Some("--optional-chain"), None, "count ?. as a branch"),
                (
                    Some("--jsx"),
                    Some("--no-jsx"),
                    "force JSX lexing on or off (default: on, except for .ts/.mts/.cts)"
                ),
                (Some("--include-dts"), None, "also scan .d.ts/.d.mts/.d.cts declaration files"),
            ]
        );
    }

    #[test]
    fn ts_files_lex_without_jsx() {
        let funcs = analyze_as("const f = <T,>(x: T) => x < 1 ? x : x;", "a.ts", &LangOptions::default());
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].kind.id, "arrow");
        assert_eq!(funcs[0].complexity, 2);
    }

    // Legacy values, recorded from:
    //   echo 'const f = () => <p>a && b</p>;' | cargo run -q -p jscx -- --ext ts --json -        => complexity 2 (`&&`)
    //   echo 'const f = () => <p>a && b</p>;' | cargo run -q -p jscx -- --jsx --ext ts --json -  => complexity 1
    // (The generic-arrow source above prints complexity 2 either way, so it cannot pin the override.)
    #[test]
    fn jsx_flag_overrides_the_extension() {
        let src = "const f = () => <p>a && b</p>;";
        let plain = analyze_as(src, "a.ts", &LangOptions::default());
        let forced = analyze_as(src, "a.ts", &opts_with("jsx", true));
        assert_eq!((plain[0].complexity, forced[0].complexity), (2, 1));
    }

    #[test]
    fn jsx_can_be_forced_off_for_a_jsx_extension() {
        let src = "const f = () => <p>a && b</p>;";
        let default = analyze_as(src, "a.tsx", &LangOptions::default());
        let off = analyze_as(src, "a.tsx", &opts_with("jsx", false));
        assert_eq!((default[0].complexity, off[0].complexity), (1, 2));
    }

    #[test]
    fn dts_is_skipped_unless_included() {
        assert!(JS.skip(Path::new("x.d.ts"), &LangOptions::default()));
        assert!(!JS.skip(Path::new("x.d.ts"), &opts_with("include-dts", true)));
        assert!(!JS.skip(Path::new("x.ts"), &LangOptions::default()));
    }

    #[test]
    fn an_uppercase_declaration_suffix_is_not_skipped_as_in_jscx() {
        // jscx matched `.d.ts` case-sensitively while lowercasing the extension,
        // so `X.D.TS` was scored as TypeScript; keep that.
        assert!(!JS.skip(Path::new("X.D.TS"), &LangOptions::default()));
        assert!(!JS.skip(Path::new("x.D.ts"), &LangOptions::default()));
    }

    #[test]
    fn optional_chain_counts_only_when_asked() {
        let src = "function f(a){ return a?.b }";
        let off = analyze_as(src, "a.js", &LangOptions::default());
        let on = analyze_as(src, "a.js", &opts_with("optional-chain", true));
        assert_eq!((off[0].complexity, on[0].complexity), (1, 2));
    }

    #[test]
    fn logical_false_drops_the_and_operator_point() {
        let src = "function f(a, b){ return a && b }";
        let default = analyze_as(src, "a.js", &LangOptions::default());
        let off = analyze_as(src, "a.js", &opts_with("logical", false));
        assert_eq!((default[0].complexity, off[0].complexity), (2, 1));
    }

    fn deeper_than_cap(src: &str, name: &str) -> Option<u32> {
        JS.first_line_deeper_than(src, Path::new(name), &LangOptions::default(), MAX_NESTING)
    }

    /// `function f() {` is one level and each `(` inside adds one.
    fn nested_parens(depth: usize) -> String {
        let inner = depth - 1;
        format!("function f() {{ x = {}1{}; }}", "(".repeat(inner), ")".repeat(inner))
    }

    #[test]
    fn nesting_exactly_at_the_cap_is_analyzed() {
        let src = nested_parens(MAX_NESTING);
        assert_eq!(deeper_than_cap(&src, "a.js"), None);
        let funcs = analyze_bounded(&JS, &src, Path::new("a.js"), &LangOptions::default());
        assert_eq!(funcs.map(|f| f.len()), Ok(1));
    }

    #[test]
    fn nesting_one_past_the_cap_is_refused_at_the_line_it_first_exceeds() {
        let src = format!("\n\n{}", nested_parens(MAX_NESTING + 1));
        assert_eq!(deeper_than_cap(&src, "a.js"), Some(3));
        let err = analyze_bounded(&JS, &src, Path::new("a.js"), &LangOptions::default()).unwrap_err();
        assert_eq!((err.line, err.kind), (3, ParseErrorKind::TooDeep { limit: MAX_NESTING }));
    }

    #[test]
    fn all_three_brackets_count() {
        let open = "([{".repeat(MAX_NESTING / 3 + 1);
        let close: String = open
            .chars()
            .rev()
            .map(|c| match c {
                '(' => ')',
                '[' => ']',
                _ => '}',
            })
            .collect();
        assert_eq!(deeper_than_cap(&format!("x = {open}{close};"), "a.js"), Some(1));
        assert_eq!(deeper_than_cap("x = ([{}]);", "a.js"), None);
    }

    #[test]
    fn nesting_100k_deep_is_refused_quickly_without_overflowing_a_2mb_stack() {
        let started = std::time::Instant::now();
        let err = std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(|| analyze_bounded(&JS, &nested_parens(100_000), Path::new("a.js"), &LangOptions::default()))
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(err.map_err(|e| e.line), Err(1));
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
    }

    #[test]
    fn brackets_in_strings_comments_templates_and_regexes_do_not_count() {
        let deep = "([{".repeat(MAX_NESTING);
        // A `[` outside a class would open one, so the plain regex has none.
        let groups = "({".repeat(MAX_NESTING);
        let src = format!(
            "function f() {{\n\
             // {deep}\n\
             /* {deep} */\n\
             a = '{deep}';\n\
             b = \"{deep}\";\n\
             c = `{deep}`;\n\
             d = /{groups}/;\n\
             e = /[{deep}]/g;\n\
             }}"
        );
        assert_eq!(deeper_than_cap(&src, "a.js"), None);
    }

    #[test]
    fn brackets_inside_a_template_substitution_count() {
        let src = format!("x = `${{{}1{}}}`;", "(".repeat(MAX_NESTING + 1), ")".repeat(MAX_NESTING + 1));
        assert_eq!(deeper_than_cap(&src, "a.js"), Some(1));
    }

    #[test]
    fn jsx_text_brackets_do_not_count_but_jsx_expression_braces_do() {
        let text = format!("const a = <p>{}</p>;", "(".repeat(MAX_NESTING * 2));
        assert_eq!(deeper_than_cap(&text, "a.jsx"), None);
        let exprs = format!("const a = <p>{}x{}</p>;", "{(".repeat(MAX_NESTING), ")}".repeat(MAX_NESTING));
        assert_eq!(deeper_than_cap(&exprs, "a.jsx"), Some(1));
    }

    /// Analyze `src` on a thread with Rust's default 2 MB spawned-thread stack.
    fn analyze_on_small_stack(src: String) -> Vec<Func> {
        std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(move || analyze_as(&src, "a.js", &LangOptions::default()))
            .unwrap()
            .join()
            .unwrap()
    }

    // The lexer tracks nesting in a `Vec` of frames and the analyzer matches
    // brackets with a `Vec` stack; neither recurses, so depth needs no cap.
    #[test]
    fn nesting_100k_parens_deep_does_not_overflow_a_2mb_stack() {
        let src = format!("function f() {{ x = {}1{}; }}", "(".repeat(100_000), ")".repeat(100_000));
        let funcs = analyze_on_small_stack(src);
        assert_eq!(funcs.len(), 1);
    }

    #[test]
    fn nesting_100k_braces_deep_does_not_overflow_a_2mb_stack() {
        let src = format!("function f() {}{} if (a) {{}} {}{}", "{", "{".repeat(100_000), "}".repeat(100_000), "}");
        let funcs = analyze_on_small_stack(src);
        assert_eq!(funcs.len(), 1);
        assert_eq!(funcs[0].complexity, 2);
    }

    #[test]
    fn nesting_100k_template_literals_deep_does_not_overflow_a_2mb_stack() {
        let src = format!("function f() {{ x = {}1{}; }}", "`${".repeat(100_000), "}`".repeat(100_000));
        let funcs = analyze_on_small_stack(src);
        assert_eq!(funcs.len(), 1);
    }
}
