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
    help: "count &&, || and ?? (and their assignment forms)",
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
    help: "lex JSX (default: on, except for .ts/.mts/.cts)",
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
    fn analyze(&self, src: &str, path: &Path, opts: &LangOptions) -> Result<Vec<Func>, ParseError> {
        let jsx = opts
            .flag(&JSX)
            .unwrap_or_else(|| jsx_default(&extension_of(path)));
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

    fn analyze_as(src: &str, name: &str, opts: &LangOptions) -> Vec<Func> {
        JS.analyze(src, Path::new(name), opts).unwrap()
    }

    fn opts_with(key: &'static str, value: bool) -> LangOptions {
        let mut opts = LangOptions::default();
        opts.set(key, value);
        opts
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
}
