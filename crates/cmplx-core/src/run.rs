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

/// Largest file, or stdin, that is read and scored. Anything larger is
/// reported as a problem instead, so one huge input cannot exhaust memory.
pub const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;

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
        Ok((config, warnings)) => {
            warnings.iter().for_each(|w| eprintln!("{program}: {w}"));
            config
        }
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
/// shrug: a typo'd key that silently does nothing is the failure this guards
/// against. Keys an alias had to ignore come back as warnings to print.
fn resolve(cli: &Cli, start: &Path, reg: &Registry) -> Result<(FileConfig, Vec<String>), String> {
    let (file, warnings) = match FileConfig::discover(start, reg) {
        Ok(Some((path, file))) => {
            let warnings = file.ignored_warnings(&path);
            (file, warnings)
        }
        Ok(None) => (FileConfig::default(), Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    Ok((apply(cli, file), warnings))
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
    execute_with_cap(cli, reg, config, stdin, out, err, MAX_FILE_BYTES)
}

/// [`execute`] with sources capped at `max_bytes` rather than [`MAX_FILE_BYTES`].
fn execute_with_cap<R: Read, W: Write, E: Write>(
    cli: &Cli,
    reg: &Registry,
    config: &FileConfig,
    stdin: &mut R,
    out: &mut W,
    err: &mut E,
    max_bytes: u64,
) -> u8 {
    let mut warn = |msg: String| {
        // Nowhere left to report a failure to write a warning.
        let _ = writeln!(err, "{}: {msg}", cli.program);
    };
    let mut rows: Vec<Row> = Vec::new();
    let mut ok = true;
    if cli.stdin {
        match read_capped(stdin, None, max_bytes) {
            Ok(bytes) => {
                let src = String::from_utf8_lossy(&bytes);
                ok &= score_stdin(&mut rows, cli, config, &src, &mut warn);
            }
            Err(ReadFailure::TooLarge) => {
                warn(too_large(STDIN_NAME, max_bytes));
                ok = false;
            }
            Err(ReadFailure::Io(e)) => {
                warn(format!("{STDIN_NAME}: {e}"));
                return 2;
            }
        }
    }
    ok &= score_files(&mut rows, cli, reg, config, max_bytes, &mut warn);

    let filter = Filter {
        min: cli.min,
        top: cli.top,
        sort: cli.sort,
    };
    // The gate sees every scored function, not just the ones --min/--top show.
    let over = report::over_limit(&rows, config.max);
    let rows = report::select(rows, &filter);
    let summary = Summary {
        over,
        ..report::summarize(&rows, config.max)
    };
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
    max_bytes: u64,
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
        match read_file_capped(&source.path, max_bytes) {
            Ok(bytes) => {
                let src = String::from_utf8_lossy(&bytes);
                ok &= score_into(rows, &name, &source.path, source.lang, &src, config, warn);
            }
            Err(ReadFailure::TooLarge) => {
                warn(too_large(&name, max_bytes));
                ok = false;
            }
            Err(ReadFailure::Io(e)) => {
                warn(format!("{name}: {e}"));
                ok = false;
            }
        }
    }
    ok
}

/// Why a source was not read.
enum ReadFailure {
    /// It holds more than the cap.
    TooLarge,
    Io(io::Error),
}

/// Read a file of at most `max_bytes`, checking its size before reading.
fn read_file_capped(path: &Path, max_bytes: u64) -> Result<Vec<u8>, ReadFailure> {
    let file = fs::File::open(path).map_err(ReadFailure::Io)?;
    let len = file.metadata().map_err(ReadFailure::Io)?.len();
    read_capped(file, Some(len), max_bytes)
}

/// Read all of `reader` if it holds at most `max_bytes`. `known_len`, when
/// given, refuses an oversized source without reading it; the read itself is
/// bounded too, so a source that grows while being read cannot exceed the cap.
fn read_capped(
    reader: impl Read,
    known_len: Option<u64>,
    max_bytes: u64,
) -> Result<Vec<u8>, ReadFailure> {
    if known_len.is_some_and(|len| len > max_bytes) {
        return Err(ReadFailure::TooLarge);
    }
    let mut bytes = Vec::with_capacity(known_len.unwrap_or(0) as usize);
    reader
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(ReadFailure::Io)?;
    if bytes.len() as u64 > max_bytes {
        return Err(ReadFailure::TooLarge);
    }
    Ok(bytes)
}

/// The warning for a source over the cap.
fn too_large(name: &str, max_bytes: u64) -> String {
    format!("{name}: larger than {}; not scored", size_label(max_bytes))
}

/// `10 MB` for a whole number of mebibytes, else the byte count.
pub(crate) fn size_label(bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    if bytes > 0 && bytes.is_multiple_of(MIB) {
        format!("{} MB", bytes / MIB)
    } else {
        format!("{bytes} bytes")
    }
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

    fn execute_bytes(reg: &Registry, list: &[&str], stdin: &[u8]) -> Outcome {
        execute_capped(reg, list, stdin, MAX_FILE_BYTES)
    }

    /// `execute`, with files and stdin capped at `max_bytes`.
    fn execute_capped(reg: &Registry, list: &[&str], mut stdin: &[u8], max_bytes: u64) -> Outcome {
        colored::control::set_override(false);
        let cli = parse(reg, list);
        let config = apply(&cli, FileConfig::default());
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = execute_with_cap(
            &cli,
            reg,
            &config,
            &mut stdin,
            &mut out,
            &mut err,
            max_bytes,
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
        let (config, warnings) = resolve(&parse(&reg, &["x.rs"]), &nested, &reg).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
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
    fn the_max_gate_counts_rows_that_min_and_top_hide() {
        let dir = temp_dir("max-unfiltered");
        let path = file(&dir, "a.rs", "f 5\ng 4\nh 1\n");
        let out = execute_with(&both(), &["--min", "30", "--max", "3", &path], "");
        assert_eq!(out.code, 1, "{}", out.stdout);
        assert!(
            out.stdout.ends_with("0 functions in 0 files\n2 functions above the limit of 3\n"),
            "got:\n{}",
            out.stdout
        );

        let out = execute_with(&both(), &["--top", "1", "-j", "--max", "3", &path], "");
        assert_eq!(out.code, 1);
        let v: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
        assert_eq!(v["summary"]["functions"], 1, "the summary describes the shown rows");
        assert_eq!(v["summary"]["overLimit"], 2, "the gate counts every row");
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
    fn a_directory_past_the_depth_cap_warns_the_rest_is_scored_and_exits_two() {
        let dir = temp_dir("too-deep");
        let deepest = (0..=crate::walk::MAX_DIR_DEPTH).fold(dir.clone(), |d, _| d.join("d"));
        fs::create_dir_all(&deepest).unwrap();
        file(&deepest, "deep.rs", "hidden 1\n");
        file(&dir, "top.rs", "shown 1\n");
        let out = execute_with(&both(), &[&dir.display().to_string()], "");
        assert_eq!(out.code, 2);
        assert_eq!(
            out.stderr,
            format!(
                "cmplx: {}: directory nesting deeper than 64; not scanned\n",
                deepest.display()
            )
        );
        assert!(out.stdout.contains("shown"), "got:\n{}", out.stdout);
        assert!(!out.stdout.contains("hidden"), "got:\n{}", out.stdout);
    }

    #[test]
    fn the_file_size_cap_is_ten_mebibytes() {
        assert_eq!(MAX_FILE_BYTES, 10 * 1024 * 1024);
    }

    #[test]
    fn a_file_at_the_size_cap_is_scored() {
        let dir = temp_dir("at-cap");
        let text = "f 1\n";
        let path = file(&dir, "a.rs", text);
        let out = execute_capped(&both(), &[&path], b"", text.len() as u64);
        assert_eq!(out.code, 0, "{}", out.stderr);
        assert!(out.stdout.contains("1 function"), "got:\n{}", out.stdout);
    }

    #[test]
    fn a_file_over_the_size_cap_is_a_problem_and_the_rest_is_still_scored() {
        let dir = temp_dir("over-cap");
        let big = file(&dir, "big.rs", "big 1\n");
        let small = file(&dir, "small.rs", "f 1\n");
        let out = execute_capped(&both(), &[&big, &small], b"", 5);
        assert_eq!(out.code, 2);
        assert_eq!(out.stderr, format!("cmplx: {big}: larger than 5 bytes; not scored\n"));
        assert!(out.stdout.contains("1 function"), "got:\n{}", out.stdout);
        assert!(!out.stdout.contains("big"), "got:\n{}", out.stdout);
    }

    #[test]
    fn stdin_at_the_size_cap_is_scored_and_over_it_is_a_problem() {
        let at = execute_capped(&both(), &["--js", "-"], b"f 1\n", 4);
        assert_eq!(at.code, 0, "{}", at.stderr);

        let over = execute_capped(&both(), &["--js", "-"], b"f 1\n", 3);
        assert_eq!(over.code, 2);
        assert_eq!(over.stderr, "cmplx: <stdin>: larger than 3 bytes; not scored\n");
    }

    #[test]
    fn a_source_that_outgrows_its_reported_size_is_still_capped() {
        let read = read_capped(&b"0123456789"[..], Some(2), 5);
        assert!(matches!(read, Err(ReadFailure::TooLarge)));
        assert_eq!(read_capped(&b"01234"[..], Some(2), 5).ok(), Some(b"01234".to_vec()));
    }

    #[test]
    fn a_size_reads_as_whole_megabytes_when_it_is_one() {
        assert_eq!(size_label(MAX_FILE_BYTES), "10 MB");
        assert_eq!(size_label(5), "5 bytes");
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
