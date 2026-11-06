//! The entry point every binary calls: parse, resolve config, score, report.
//!
//! Exit codes: 0 clean, 1 a function scored above `--max`, 2 an unreadable
//! path, a parse failure, or a config or argument error.

use crate::cli::{parse_args, Cli};
use crate::config::FileConfig;
use crate::language::{Lang, Registry};
use crate::report::{self, Filter, Row, Style, Summary};
use crate::walk::{self, Scope};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The file name rows read from stdin are reported under.
const STDIN_NAME: &str = "<stdin>";

/// Run a binary whose languages are `reg`: read `std::env::args`, then either
/// serve `--lsp` or score the given paths.
pub fn run(program: &'static str, reg: Registry) -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cli = match parse_args(program, &args, &reg) {
        Ok(Some(cli)) => cli,
        Ok(None) => return ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("{program}: {msg}");
            return ExitCode::from(2);
        }
    };
    if cli.lsp {
        return serve_lsp(program, &reg);
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let config = match resolve(&cli, &cwd, &reg) {
        Ok(config) => config,
        Err(msg) => {
            eprintln!("{program}: {msg}");
            return ExitCode::from(2);
        }
    };
    let code = execute(
        &cli,
        &reg,
        &config,
        &mut io::stdin().lock(),
        &mut io::stdout().lock(),
        &mut io::stderr().lock(),
    );
    ExitCode::from(code)
}

/// Run an alias binary: `lang` is the only language and its alias the program name.
pub fn run_as(lang: Lang) -> ExitCode {
    let program = lang
        .alias()
        .expect("an alias binary's language declares its alias");
    run(program, Registry::alias(lang))
}

/// Serve the language server on stdio. Only a broken stdio ends it early;
/// that is reported on stderr, since stdout is the protocol.
fn serve_lsp(program: &'static str, reg: &Registry) -> ExitCode {
    match crate::lsp::serve(program, reg) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{program}: --lsp: {e}");
            ExitCode::from(2)
        }
    }
}

/// Layer a config file under the flags. Pure, so it is testable without a
/// filesystem; `resolve` supplies the file.
fn apply(cli: &Cli, file: FileConfig) -> FileConfig {
    cli.flags.clone().or(file)
}

/// Find a `cmplx.toml` (or `rustcx.toml`) by walking up from `start`, and layer
/// it under the flags. An unreadable or unparseable file is an error, not a
/// shrug: a typo'd key that silently does nothing is the failure this guards against.
fn resolve(cli: &Cli, start: &Path, reg: &Registry) -> Result<FileConfig, String> {
    let file = match FileConfig::discover(start, reg) {
        Ok(Some((_, file))) => file,
        Ok(None) => FileConfig::default(),
        Err(e) => return Err(e.to_string()),
    };
    Ok(apply(cli, file))
}

/// Score stdin and the paths, print the report, and return the exit code.
/// Warnings go to `err`; a failed path or parse does not stop the run.
fn execute<R: Read, W: Write, E: Write>(
    cli: &Cli,
    reg: &Registry,
    config: &FileConfig,
    stdin: &mut R,
    out: &mut W,
    err: &mut E,
) -> u8 {
    let mut warn = |msg: String| {
        // Nowhere left to report a failure to write a warning.
        let _ = writeln!(err, "{}: {msg}", cli.program);
    };
    let mut rows: Vec<Row> = Vec::new();
    let mut ok = true;
    if cli.stdin {
        let mut bytes = Vec::new();
        if let Err(e) = stdin.read_to_end(&mut bytes) {
            warn(format!("{STDIN_NAME}: {e}"));
            return 2;
        }
        let src = String::from_utf8_lossy(&bytes);
        ok &= score_stdin(&mut rows, cli, config, &src, &mut warn);
    }
    ok &= score_files(&mut rows, cli, reg, config, &mut warn);

    let filter = Filter {
        min: cli.min,
        top: cli.top,
        sort: cli.sort,
    };
    let rows = report::select(rows, &filter);
    let summary = report::summarize(&rows, config.max);
    match render(out, &rows, &summary, cli) {
        // A closed pipe (`cmplx . | head`) is the reader's choice, not a failure.
        Err(e) if e.kind() != io::ErrorKind::BrokenPipe => {
            warn(e.to_string());
            2
        }
        _ => exit_code(summary.over, ok),
    }
}

/// Score stdin as the one active language (the CLI guarantees there is one),
/// lexed by `--ext` or the language's stdin extension.
fn score_stdin(
    rows: &mut Vec<Row>,
    cli: &Cli,
    config: &FileConfig,
    src: &str,
    warn: &mut impl FnMut(String),
) -> bool {
    let lang = cli.active[0];
    let ext = cli.ext.as_deref().unwrap_or(lang.stdin_ext());
    let path = PathBuf::from(format!("{STDIN_NAME}.{ext}"));
    score_into(rows, STDIN_NAME, &path, lang, src, config, warn)
}

/// Score every file named or found under the command-line paths. Returns false
/// if any path could not be used, read or parsed.
fn score_files(
    rows: &mut Vec<Row>,
    cli: &Cli,
    reg: &Registry,
    config: &FileConfig,
    warn: &mut impl FnMut(String),
) -> bool {
    let scope = Scope {
        reg,
        active: &cli.active,
        explicit_lang: cli.explicit_lang,
        config,
    };
    let expanded = walk::expand(&cli.paths, &scope);
    let mut ok = expanded.problems.is_empty();
    expanded.problems.into_iter().for_each(&mut *warn);
    for source in &expanded.sources {
        let name = source.path.display().to_string();
        match fs::read(&source.path) {
            Ok(bytes) => {
                let src = String::from_utf8_lossy(&bytes);
                ok &= score_into(rows, &name, &source.path, source.lang, &src, config, warn);
            }
            Err(e) => {
                warn(format!("{name}: {e}"));
                ok = false;
            }
        }
    }
    ok
}

/// Analyze one source and append its rows. Returns false on a parse error,
/// which is reported as a warning naming the file.
fn score_into(
    rows: &mut Vec<Row>,
    name: &str,
    path: &Path,
    lang: Lang,
    src: &str,
    config: &FileConfig,
    warn: &mut impl FnMut(String),
) -> bool {
    match lang.analyze(src, path, &config.options_for(lang)) {
        Ok(funcs) => {
            rows.extend(funcs.into_iter().map(|func| Row {
                file: name.to_string(),
                lang,
                func,
            }));
            true
        }
        Err(e) => {
            warn(format!("{name}:{}: parse error: {}", e.line, e.message));
            false
        }
    }
}

/// Write the report in whichever format `cli` selects.
fn render<W: Write>(w: &mut W, rows: &[Row], summary: &Summary, cli: &Cli) -> io::Result<()> {
    if cli.json {
        report::write_json(w, rows, summary)
    } else {
        let style = Style {
            header: !cli.no_header,
            explain: cli.explain,
            flat: cli.sort,
        };
        report::write_table(w, rows, summary, style)
    }?;
    w.flush()
}

/// 2 if anything failed, 1 if a function scored above `--max`, 0 otherwise.
fn exit_code(over: usize, ok: bool) -> u8 {
    match (ok, over) {
        (false, _) => 2,
        (true, 0) => 0,
        (true, _) => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{parse_args, Cli};
    use crate::config::FileConfig;
    use crate::language::{LangOptions, Language, OptionSpec, Registry};
    use crate::model::{Func, Kind, ParseError};
    use std::fs;
    use std::path::{Path, PathBuf};

    const TESTS: OptionSpec = OptionSpec {
        key: "tests",
        on_flag: None,
        off_flag: Some("--no-tests"),
        default: Some(true),
        help: "",
    };
    static OPTIONS: [OptionSpec; 1] = [TESTS];

    /// Each source line `name score` is one function; a line `!` is a parse
    /// error on that line. The analysis path's extension is recorded as the
    /// name of a function `ext:<extension>` so stdin handling is observable.
    struct FakeLang {
        id: &'static str,
        extensions: &'static [&'static str],
    }
    impl Language for FakeLang {
        fn id(&self) -> &'static str {
            self.id
        }
        fn display_name(&self) -> &'static str {
            "Fake"
        }
        fn alias(&self) -> Option<&'static str> {
            Some("fakecx")
        }
        fn extensions(&self) -> &'static [&'static str] {
            self.extensions
        }
        fn stdin_ext(&self) -> &'static str {
            self.extensions[0]
        }
        fn options(&self) -> &'static [OptionSpec] {
            &OPTIONS
        }
        fn skip(&self, _: &Path, _: &LangOptions) -> bool {
            false
        }
        fn analyze(
            &self,
            src: &str,
            path: &Path,
            _: &LangOptions,
        ) -> Result<Vec<Func>, ParseError> {
            let mut funcs = Vec::new();
            for (i, line) in src.lines().enumerate() {
                let line_no = i as u32 + 1;
                if line == "!" {
                    return Err(ParseError {
                        line: line_no,
                        message: "bad".into(),
                    });
                }
                if line == "ext?" {
                    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                    funcs.push(fake_func(&format!("ext:{ext}"), line_no, 1));
                    continue;
                }
                let (name, cc) = line.split_once(' ').expect("`name score`");
                funcs.push(fake_func(name, line_no, cc.parse().unwrap()));
            }
            Ok(funcs)
        }
    }
    static JS: FakeLang = FakeLang {
        id: "js",
        extensions: &["js", "ts"],
    };
    static RUST: FakeLang = FakeLang {
        id: "rust",
        extensions: &["rs"],
    };

    fn fake_func(name: &str, line: u32, complexity: u32) -> Func {
        Func {
            name: name.to_string(),
            kind: Kind {
                id: "fn",
                label: "fn",
            },
            line,
            complexity,
            depth: 0,
            points: Vec::new(),
        }
    }

    fn both() -> Registry {
        Registry::all(&[&JS, &RUST])
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn parse(reg: &Registry, list: &[&str]) -> Cli {
        parse_args("cmplx", &args(list), reg).unwrap().unwrap()
    }

    fn temp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("cmplx-run-{}-{}-{}", tag, std::process::id(), n));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    struct Outcome {
        code: u8,
        stdout: String,
        stderr: String,
    }

    fn execute_with(reg: &Registry, list: &[&str], stdin: &str) -> Outcome {
        execute_bytes(reg, list, stdin.as_bytes())
    }

    fn execute_bytes(reg: &Registry, list: &[&str], mut stdin: &[u8]) -> Outcome {
        colored::control::set_override(false);
        let cli = parse(reg, list);
        let config = apply(&cli, FileConfig::default());
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = execute(
            &cli,
            reg,
            &config,
            &mut stdin,
            &mut out,
            &mut err,
        );
        Outcome {
            code,
            stdout: String::from_utf8(out).unwrap(),
            stderr: String::from_utf8(err).unwrap(),
        }
    }

    fn file(dir: &Path, name: &str, text: &str) -> String {
        let path = dir.join(name);
        fs::write(&path, text).unwrap();
        path.display().to_string()
    }

    #[test]
    fn a_flag_beats_the_file_and_the_file_beats_the_default() {
        let reg = both();
        let mut file = FileConfig::default();
        file.max = Some(9);
        file.set_option(&RUST, "tests", true);
        let merged = apply(&parse(&reg, &["--no-tests", "x.rs"]), file);
        assert_eq!(
            merged.options_for(&RUST).get("tests"),
            Some(false),
            "the flag wins"
        );
        assert_eq!(merged.max, Some(9), "an unset flag takes the file's value");

        let mut file = FileConfig::default();
        file.max = Some(9);
        let merged = apply(&parse(&reg, &["-M", "4", "x.rs"]), file);
        assert_eq!(merged.max, Some(4), "--max beats the file's max");

        let merged = apply(&parse(&reg, &["x.rs"]), FileConfig::default());
        assert_eq!(
            merged.options_for(&RUST).get("tests"),
            None,
            "left to the language default"
        );
        assert_eq!(merged.max, None);
    }

    #[test]
    fn resolve_layers_the_discovered_cmplx_toml_under_the_flags() {
        let reg = both();
        let root = temp_dir("resolve");
        let nested = root.join("deep");
        fs::create_dir_all(&nested).unwrap();
        fs::write(root.join("cmplx.toml"), "max = 7\n[rust]\ntests = false\n").unwrap();
        let config = resolve(&parse(&reg, &["x.rs"]), &nested, &reg).unwrap();
        assert_eq!(config.max, Some(7));
        assert_eq!(config.options_for(&RUST).get("tests"), Some(false));
    }

    #[test]
    fn resolve_reports_a_broken_config_file() {
        let reg = both();
        let root = temp_dir("broken");
        fs::write(root.join("cmplx.toml"), "treshold = 3\n").unwrap();
        let err = resolve(&parse(&reg, &["x.rs"]), &root, &reg).unwrap_err();
        assert!(
            err.contains("cmplx.toml") && err.contains("treshold"),
            "got: {err}"
        );
    }

    #[test]
    fn exit_code_is_two_on_failure_one_over_the_limit_else_zero() {
        assert_eq!(exit_code(0, false), 2, "a failure outranks the limit");
        assert_eq!(exit_code(3, false), 2);
        assert_eq!(exit_code(3, true), 1);
        assert_eq!(exit_code(0, true), 0);
    }

    #[test]
    fn a_clean_run_prints_the_table_and_exits_zero() {
        let dir = temp_dir("clean");
        let path = file(&dir, "a.rs", "f 2\ng 1\n");
        let out = execute_with(&both(), &[&path], "");
        assert_eq!(out.code, 0, "{}", out.stderr);
        assert!(
            out.stdout.contains("     2      1  fn           f\n"),
            "got:\n{}",
            out.stdout
        );
        assert!(out
            .stdout
            .contains("2 functions in 1 file · mean 1.5 · worst 2 (f at "));
        assert_eq!(out.stderr, "");
    }

    #[test]
    fn exceeding_max_exits_one_and_says_so() {
        let dir = temp_dir("max");
        let path = file(&dir, "a.rs", "f 5\ng 1\n");
        let out = execute_with(&both(), &["--max", "3", &path], "");
        assert_eq!(out.code, 1);
        assert!(
            out.stdout.ends_with("1 function above the limit of 3\n"),
            "got:\n{}",
            out.stdout
        );
    }

    #[test]
    fn a_parse_failure_warns_names_the_file_and_the_rest_is_still_scored() {
        let dir = temp_dir("parse");
        let bad = file(&dir, "bad.rs", "f 1\n!\n");
        let good = file(&dir, "good.rs", "g 1\n");
        let out = execute_with(&both(), &[&bad, &good], "");
        assert_eq!(out.code, 2);
        assert_eq!(out.stderr, format!("cmplx: {bad}:2: parse error: bad\n"));
        assert!(out.stdout.contains("g\n"), "got:\n{}", out.stdout);
    }

    #[test]
    fn an_unreadable_path_exits_two_and_names_the_path() {
        let dir = temp_dir("missing");
        let missing = dir.join("nope.rs").display().to_string();
        let out = execute_with(&both(), &[&missing], "");
        assert_eq!(out.code, 2);
        assert!(
            out.stderr.starts_with(&format!("cmplx: {missing}: ")),
            "got: {}",
            out.stderr
        );
        assert!(out.stdout.ends_with("0 functions in 0 files\n"));
    }

    #[test]
    fn a_named_file_no_language_claims_exits_two() {
        let dir = temp_dir("unclaimed");
        let path = file(&dir, "notes.txt", "f 1\n");
        let out = execute_with(&both(), &[&path], "");
        assert_eq!(out.code, 2);
        assert_eq!(
            out.stderr,
            format!("cmplx: no language for {path}; pass --js, --rust\n")
        );
    }

    #[test]
    fn an_alias_scores_a_named_file_as_its_language() {
        let dir = temp_dir("alias");
        let path = file(&dir, "notes.txt", "f 1\n");
        let out = execute_with(&Registry::alias(&RUST), &[&path], "");
        assert_eq!(out.code, 0, "{}", out.stderr);
        assert!(out.stdout.starts_with(&format!("{path}\n")));
    }

    #[test]
    fn stdin_is_scored_as_the_one_language_with_its_stdin_extension() {
        let out = execute_with(&both(), &["--js", "-", "--json"], "ext?\n");
        assert_eq!(out.code, 0, "{}", out.stderr);
        let v: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
        assert_eq!(v["functions"][0]["file"], "<stdin>");
        assert_eq!(v["functions"][0]["language"], "js");
        assert_eq!(v["functions"][0]["name"], "ext:js");
    }

    #[test]
    fn stdin_that_is_not_utf8_is_read_lossily_like_a_file() {
        let out = execute_bytes(&both(), &["--js", "-"], b"caf\xff 2\ng 1\n");
        assert_eq!(out.code, 0, "{}", out.stderr);
        assert!(out.stdout.contains("caf\u{fffd}"), "got:\n{}", out.stdout);
        assert!(out.stdout.contains("2 functions"), "got:\n{}", out.stdout);
    }

    #[test]
    fn ext_overrides_the_stdin_extension() {
        let out = execute_with(
            &Registry::alias(&JS),
            &["--ext", "ts", "-", "--json"],
            "ext?\n",
        );
        let v: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
        assert_eq!(v["functions"][0]["name"], "ext:ts");
    }

    #[test]
    fn json_output_reports_the_limit() {
        let dir = temp_dir("json");
        let path = file(&dir, "a.rs", "f 5\n");
        let out = execute_with(&both(), &["-j", "-M", "4", &path], "");
        assert_eq!(out.code, 1);
        let v: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
        assert_eq!(v["summary"]["limit"], 4);
        assert_eq!(v["summary"]["overLimit"], 1);
    }
}
