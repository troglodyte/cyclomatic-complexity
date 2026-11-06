//! CLI parsing, exercised with fake languages (the core knows no real one).

use cmplx_core::cli::{help, parse_args, Cli};
use cmplx_core::language::{Lang, LangOptions, Language, OptionSpec, Registry};
use cmplx_core::model::{Func, ParseError};
use std::path::{Path, PathBuf};

const LOGICAL: OptionSpec = OptionSpec {
    key: "logical",
    on_flag: None,
    off_flag: Some("--no-logical"),
    default: Some(true),
    help: "do not count && and ||",
};

struct FakeJs;
impl Language for FakeJs {
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
        &["js", "ts"]
    }
    fn stdin_ext(&self) -> &'static str {
        "tsx"
    }
    fn options(&self) -> &'static [OptionSpec] {
        const OPTS: &[OptionSpec] = &[
            OptionSpec {
                key: "optional-chain",
                on_flag: Some("--optional-chain"),
                off_flag: None,
                default: Some(false),
                help: "count ?. as a branch",
            },
            LOGICAL,
            OptionSpec {
                key: "jsx",
                on_flag: Some("--jsx"),
                off_flag: Some("--no-jsx"),
                default: None,
                help: "force JSX lexing",
            },
            OptionSpec {
                key: "include-dts",
                on_flag: Some("--include-dts"),
                off_flag: None,
                default: Some(false),
                help: "do not skip .d.ts files",
            },
        ];
        OPTS
    }
    fn skip(&self, _: &Path, _: &LangOptions) -> bool {
        false
    }
    fn analyze(&self, _: &str, _: &Path, _: &LangOptions) -> Result<Vec<Func>, ParseError> {
        Ok(vec![])
    }
}

struct FakeRust;
impl Language for FakeRust {
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
        const OPTS: &[OptionSpec] = &[
            OptionSpec {
                key: "question",
                on_flag: Some("--question"),
                off_flag: None,
                default: Some(false),
                help: "count ? as a branch",
            },
            LOGICAL,
            OptionSpec {
                key: "tests",
                on_flag: None,
                off_flag: Some("--no-tests"),
                default: Some(true),
                help: "skip test code",
            },
        ];
        OPTS
    }
    fn skip(&self, _: &Path, _: &LangOptions) -> bool {
        false
    }
    fn analyze(&self, _: &str, _: &Path, _: &LangOptions) -> Result<Vec<Func>, ParseError> {
        Ok(vec![])
    }
}

static JS: FakeJs = FakeJs;
static RUST: FakeRust = FakeRust;

fn every() -> Registry {
    Registry::all(&[&JS, &RUST])
}
fn js_alias() -> Registry {
    Registry::alias(&JS)
}
fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}
fn cmplx(list: &[&str]) -> Cli {
    parse_args("cmplx", &args(list), &every()).unwrap().unwrap()
}
fn jscx(list: &[&str]) -> Cli {
    parse_args("jscx", &args(list), &js_alias())
        .unwrap()
        .unwrap()
}
fn cmplx_err(list: &[&str]) -> String {
    parse_args("cmplx", &args(list), &every()).unwrap_err()
}
fn ids(cli: &Cli) -> Vec<&'static str> {
    cli.active.iter().map(|l| l.id()).collect()
}
fn opt(cli: &Cli, lang: Lang, key: &str) -> Option<bool> {
    cli.flags.options_for(lang).get(key)
}

#[test]
fn defaults_match_the_spec() {
    let c = cmplx(&["src/"]);
    assert_eq!(c.program, "cmplx");
    assert_eq!(c.paths, vec![PathBuf::from("src/")]);
    assert_eq!((c.min, c.max, c.top), (1, None, None));
    assert!(!c.sort && !c.json && !c.explain && !c.no_header && !c.lsp && !c.stdin);
    assert_eq!(c.ext, None);
    assert_eq!(c.flags, cmplx_core::config::FileConfig::default());
}

#[test]
fn language_flags_restrict_the_active_set() {
    let c = cmplx(&["--js", "src"]);
    assert_eq!(ids(&c), ["js"]);
    assert!(c.explicit_lang);
}

#[test]
fn no_language_flag_means_every_language() {
    let c = cmplx(&["src"]);
    assert_eq!(ids(&c), ["js", "rust"]);
    assert!(!c.explicit_lang);
}

#[test]
fn an_alias_fixes_its_language() {
    let c = jscx(&["src"]);
    assert_eq!(ids(&c), ["js"]);
    assert!(c.explicit_lang);
}

#[test]
fn a_flag_of_an_inactive_language_is_rejected() {
    let alias = parse_args("jscx", &args(&["--no-tests", "src"]), &js_alias());
    assert_eq!(alias.unwrap_err(), "--no-tests is not an option for js");
    assert_eq!(
        cmplx_err(&["--js", "--no-tests", "src"]),
        "--no-tests is not an option for js"
    );
}

#[test]
fn option_order_does_not_matter() {
    let c = cmplx(&["--no-tests", "--rust", "src"]);
    assert_eq!(opt(&c, &RUST, "tests"), Some(false));
}

#[test]
fn a_flag_no_active_language_declares_names_every_active_language() {
    assert_eq!(
        cmplx_err(&["--x", "src"]),
        "--x is not an option for js, rust"
    );
}

#[test]
fn a_flag_declared_by_some_active_languages_applies_to_those() {
    let c = cmplx(&["--no-tests", "src"]);
    assert_eq!(opt(&c, &RUST, "tests"), Some(false));
    assert_eq!(opt(&c, &JS, "tests"), None);
}

#[test]
fn a_shared_option_flag_reaches_every_active_language() {
    let c = cmplx(&["--no-logical", "src"]);
    assert_eq!(opt(&c, &JS, "logical"), Some(false));
    assert_eq!(opt(&c, &RUST, "logical"), Some(false));
}

#[test]
fn absent_flags_leave_options_unset() {
    let c = cmplx(&["src"]);
    assert_eq!(opt(&c, &RUST, "tests"), None);
}

#[test]
fn on_and_off_flags_set_their_values() {
    let c = jscx(&["--jsx", "--optional-chain", "--include-dts", "x"]);
    assert_eq!(opt(&c, &JS, "jsx"), Some(true));
    assert_eq!(opt(&c, &JS, "optional-chain"), Some(true));
    assert_eq!(opt(&c, &JS, "include-dts"), Some(true));
    assert_eq!(
        jscx(&["--no-jsx", "x"]).flags.options_for(&JS).get("jsx"),
        Some(false)
    );
}

#[test]
fn the_last_of_conflicting_flags_wins() {
    let c = jscx(&["--jsx", "--no-jsx", "x"]);
    assert_eq!(opt(&c, &JS, "jsx"), Some(false));
}

#[test]
fn stdin_without_a_language_is_an_error() {
    assert_eq!(
        cmplx_err(&["-"]),
        "reading stdin needs a language: pass one of --js, --rust"
    );
}

#[test]
fn stdin_with_a_language_or_alias_is_fine() {
    assert!(cmplx(&["--rust", "-"]).stdin);
    assert!(jscx(&["-"]).stdin);
}

#[test]
fn stdin_with_several_language_flags_is_an_error() {
    assert!(cmplx_err(&["--js", "--rust", "-"]).contains("exactly one"));
}

#[test]
fn aliases_do_not_offer_language_flags() {
    let err = parse_args("jscx", &args(&["--rust", "x"]), &js_alias()).unwrap_err();
    assert!(err.contains("--rust"), "{err}");
}

#[test]
fn ext_is_normalised_and_accepted_with_or_without_stdin() {
    assert_eq!(jscx(&["--ext", ".TS", "-"]).ext.as_deref(), Some("ts"));
    assert_eq!(jscx(&["--ext=tsx", "x"]).ext.as_deref(), Some("tsx"));
}

#[test]
fn value_flags_accept_equals_form() {
    assert_eq!(cmplx(&["--min=3", "x"]).min, 3);
    assert_eq!(cmplx(&["--top=2", "x"]).top, Some(2));
}

#[test]
fn double_dash_ends_flags() {
    let c = cmplx(&["--", "--weird"]);
    assert_eq!(c.paths, vec![PathBuf::from("--weird")]);
}

#[test]
fn max_is_recorded_for_the_file_layer_merge() {
    let c = cmplx(&["-M", "15", "x"]);
    assert_eq!(c.max, Some(15));
    assert_eq!(c.flags.max, Some(15));
    assert_eq!(cmplx(&["x"]).flags.max, None);
}

#[test]
fn the_lsp_flag_needs_no_paths() {
    let c = cmplx(&["--lsp"]);
    assert!(c.lsp && c.paths.is_empty());
}

#[test]
fn numeric_flags_accept_short_and_long_forms() {
    assert_eq!(cmplx(&["-m", "3", "x"]).min, 3);
    assert_eq!(cmplx(&["--min", "3", "x"]).min, 3);
    assert_eq!(cmplx(&["-M", "15", "x"]).max, Some(15));
    assert_eq!(cmplx(&["-t", "20", "x"]).top, Some(20));
}

#[test]
fn boolean_flags_are_recognised() {
    assert!(cmplx(&["-s", "x"]).sort);
    assert!(cmplx(&["-j", "x"]).json);
    assert!(cmplx(&["-e", "x"]).explain);
    assert!(cmplx(&["--no-header", "x"]).no_header);
}

#[test]
fn a_lone_dash_means_stdin() {
    let c = cmplx(&["--js", "-"]);
    assert!(c.stdin && c.paths.is_empty());
}

#[test]
fn help_and_version_return_none() {
    for flag in ["--help", "-h", "-V", "--version"] {
        assert!(parse_args("cmplx", &args(&[flag]), &every())
            .unwrap()
            .is_none());
    }
}

#[test]
fn errors_are_reported_not_panicked() {
    assert!(parse_args("cmplx", &args(&["-x", "a"]), &every()).is_err());
    assert!(parse_args("cmplx", &args(&["-m", "x.rs"]), &every()).is_err());
    assert!(parse_args("cmplx", &args(&["-m"]), &every()).is_err());
}

#[test]
fn help_lists_each_languages_options_under_its_name() {
    let text = help("cmplx", &every());
    let js = text.find("JavaScript/TypeScript").expect("js heading");
    assert!(text[js..].contains("--optional-chain"));
    let rust = text.find("Rust").expect("rust heading");
    assert!(text[rust..].contains("--no-tests"));
    assert!(text.contains("--js") && text.contains("--rust"));
    assert!(!help("jscx", &js_alias()).contains("--no-tests"));
}

#[test]
fn an_alias_help_names_the_alias_and_offers_no_language_flags() {
    let text = help("jscx", &js_alias());
    assert!(text.contains("jscx [OPTIONS]"));
    assert!(!text.contains("--js "));
}

#[test]
fn no_paths_defaults_to_the_current_directory() {
    assert_eq!(cmplx(&[]).paths, vec![PathBuf::from(".")]);
    assert_eq!(jscx(&["--min", "2"]).paths, vec![PathBuf::from(".")]);
}

#[test]
fn stdin_and_lsp_do_not_get_a_default_path() {
    assert!(cmplx(&["--js", "-"]).paths.is_empty());
    assert!(cmplx(&["--lsp"]).paths.is_empty());
}
