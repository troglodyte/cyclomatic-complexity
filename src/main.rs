use colored::Colorize;
use rustcx::analyze::{Func, Options};
use serde::Serialize;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const HELP: &str = "\
rustcx — cyclomatic complexity for Rust functions

USAGE:
    rustcx [OPTIONS] <PATH>...     files or directories (recursive)
    rustcx [OPTIONS] -             read from stdin

OPTIONS:
    -m, --min <N>      only report functions scoring N or higher (default 1)
    -M, --max <N>      exit 1 if any function scores above N — for CI
    -t, --top <N>      show only the N worst functions
    -s, --sort         sort by score, highest first
        --no-header    do not print the table header
    -j, --json         machine-readable output
    -e, --explain      list the decision points behind each score
        --question     count `?` as a branch (off by default)
        --no-logical   don't count `&&`, `||`
        --no-tests     skip #[test] functions and #[cfg(test)] modules
    -h, --help         print this help
    -V, --version      print version
";

#[derive(Debug)]
struct Config {
    paths: Vec<PathBuf>,
    stdin: bool,
    min: u32,
    max: Option<u32>,
    top: Option<usize>,
    sort: bool,
    header: bool,
    json: bool,
    explain: bool,
    opts: Options,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            paths: Vec::new(),
            stdin: false,
            min: 1,
            max: None,
            top: None,
            sort: false,
            header: true,
            json: false,
            explain: false,
            opts: Options::default(),
        }
    }
}

/// Parse argv (without the program name).
///
/// `Ok(None)` means help or version was printed and the caller should exit 0.
fn parse_args(args: &[String]) -> Result<Option<Config>, String> {
    let mut cfg = Config::default();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "-h" | "--help" => {
                print!("{HELP}");
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("rustcx {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "-m" | "--min" => cfg.min = take_number(args, &mut i, arg)?,
            "-M" | "--max" => cfg.max = Some(take_number(args, &mut i, arg)?),
            "-t" | "--top" => cfg.top = Some(take_number(args, &mut i, arg)? as usize),
            "-s" | "--sort" => cfg.sort = true,
            "--no-header" => cfg.header = false,
            "-j" | "--json" => cfg.json = true,
            "-e" | "--explain" => cfg.explain = true,
            "--question" => cfg.opts.question = true,
            "--no-logical" => cfg.opts.logical = false,
            "--no-tests" => cfg.opts.tests = false,
            "-" => cfg.stdin = true,
            _ => {
                if arg.starts_with('-') && arg.len() > 1 {
                    return Err(format!("unknown option: {arg}"));
                }
                cfg.paths.push(PathBuf::from(arg));
            }
        }
        i += 1;
    }
    if cfg.paths.is_empty() && !cfg.stdin {
        return Err("no paths given".to_string());
    }
    Ok(Some(cfg))
}

/// Consume the value following a flag and parse it as a number, advancing `i`.
fn take_number(args: &[String], i: &mut usize, flag: &str) -> Result<u32, String> {
    *i += 1;
    let raw = args.get(*i).ok_or_else(|| format!("{flag} needs a value"))?;
    raw.parse::<u32>()
        .map_err(|_| format!("{flag} needs a number, got: {raw}"))
}

#[derive(Clone, Debug)]
struct Row {
    file: String,
    func: Func,
}

#[derive(Debug)]
struct Summary {
    funcs: usize,
    files: usize,
    mean: f64,
    worst: Option<Row>,
}

fn summarize(rows: &[Row]) -> Summary {
    let mut files: Vec<&str> = rows.iter().map(|r| r.file.as_str()).collect();
    files.sort_unstable();
    files.dedup();
    let total: u32 = rows.iter().map(|r| r.func.complexity).sum();
    let mean = if rows.is_empty() {
        0.0
    } else {
        f64::from(total) / rows.len() as f64
    };
    let worst = rows.iter().max_by_key(|r| r.func.complexity).cloned();
    Summary { funcs: rows.len(), files: files.len(), mean, worst }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Column layout, copied from jscx so the two tools' output lines up:
/// score right-aligned in 6, line right-aligned in 7, two spaces, kind
/// left-aligned in 13, then two spaces of indent per nesting level.
fn write_table<W: Write>(
    w: &mut W,
    rows: &[Row],
    summary: &Summary,
    header: bool,
    explain: bool,
) -> io::Result<()> {
    let mut current = "";
    for row in rows {
        if row.file != current {
            if !current.is_empty() {
                writeln!(w)?;
            }
            writeln!(w, "{}", row.file.bold())?;
            if header {
                writeln!(w, "{:>6}{:>7}  {:<13}Name", "CC", "Line", "Kind")?;
            }
            current = &row.file;
        }
        let f = &row.func;
        writeln!(
            w,
            "{:>6}{:>7}  {:<13}{}{}",
            f.complexity,
            f.line,
            f.kind.label(),
            " ".repeat(2 * f.depth as usize),
            f.name
        )?;
        if explain {
            for p in &f.points {
                // Six spaces, then the line right-aligned in 7 so it sits under
                // the Line column.
                writeln!(w, "      {:>7}  {}", p.line, p.branch)?;
            }
        }
    }
    if !rows.is_empty() {
        writeln!(w)?;
    }
    write!(
        w,
        "{} function{} in {} file{}",
        summary.funcs,
        plural(summary.funcs),
        summary.files,
        plural(summary.files)
    )?;
    if let Some(worst) = &summary.worst {
        write!(
            w,
            " · mean {:.1} · worst {} ({} at {}:{})",
            summary.mean, worst.func.complexity, worst.func.name, worst.file, worst.func.line
        )?;
    }
    writeln!(w)
}


const SKIP_DIRS: &[&str] = &["target", ".git", ".hg", ".svn", ".jj"];

fn is_source(p: &Path) -> bool {
    p.extension().is_some_and(|e| e == "rs")
}

/// Recursively collect `.rs` files under `root`, skipping build and VCS dirs.
fn collect(root: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    let mut entries: Vec<PathBuf> = fs::read_dir(root)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            let skip = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| SKIP_DIRS.contains(&n));
            if !skip {
                collect(&path, out)?;
            }
        } else if is_source(&path) {
            out.push(path);
        }
    }
    Ok(())
}

/// Filter and order rows for display.
///
/// `--min` drops low scores, `--top` keeps the N worst, and `--sort` orders by
/// score descending. Without `--sort` the surviving rows are restored to source
/// order so the table still reads file by file.
fn select(rows: Vec<Row>, cfg: &Config) -> Vec<Row> {
    let mut kept: Vec<(usize, Row)> = rows
        .into_iter()
        .enumerate()
        .filter(|(_, r)| r.func.complexity >= cfg.min)
        .collect();

    if cfg.top.is_some() || cfg.sort {
        kept.sort_by_key(|(_, r)| std::cmp::Reverse(r.func.complexity));
    }
    if let Some(n) = cfg.top {
        kept.truncate(n);
    }
    if !cfg.sort {
        kept.sort_by_key(|(i, _)| *i);
    }
    kept.into_iter().map(|(_, r)| r).collect()
}

#[derive(Serialize)]
struct JsonFunc<'a> {
    file: &'a str,
    #[serde(flatten)]
    func: &'a Func,
}

#[derive(Serialize)]
struct JsonWorst<'a> {
    file: &'a str,
    name: &'a str,
    line: u32,
    complexity: u32,
}

#[derive(Serialize)]
struct JsonSummary<'a> {
    functions: usize,
    files: usize,
    mean: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    worst: Option<JsonWorst<'a>>,
}

#[derive(Serialize)]
struct JsonReport<'a> {
    functions: Vec<JsonFunc<'a>>,
    summary: JsonSummary<'a>,
}

fn write_json<W: Write>(w: &mut W, rows: &[Row], summary: &Summary) -> io::Result<()> {
    let report = JsonReport {
        functions: rows
            .iter()
            .map(|r| JsonFunc { file: &r.file, func: &r.func })
            .collect(),
        summary: JsonSummary {
            functions: summary.funcs,
            files: summary.files,
            mean: summary.mean,
            worst: summary.worst.as_ref().map(|r| JsonWorst {
                file: &r.file,
                name: &r.func.name,
                line: r.func.line,
                complexity: r.func.complexity,
            }),
        },
    };
    serde_json::to_writer(&mut *w, &report).map_err(io::Error::other)?;
    writeln!(w)
}

fn run(cfg: &Config) -> ExitCode {
    let mut rows: Vec<Row> = Vec::new();
    let mut trouble = false;

    if cfg.stdin {
        let mut src = String::new();
        if let Err(e) = io::Read::read_to_string(&mut io::stdin(), &mut src) {
            eprintln!("rustcx: <stdin>: {e}");
            return ExitCode::from(2);
        }
        trouble |= !score_into(&mut rows, "<stdin>", &src, cfg);
    }

    // Expand the paths given on the command line. A file named explicitly is
    // read whatever its extension; a directory is walked for `.rs`.
    let mut files: Vec<PathBuf> = Vec::new();
    for p in &cfg.paths {
        if p.is_dir() {
            if let Err(e) = collect(p, &mut files) {
                eprintln!("rustcx: {}: {e}", p.display());
                trouble = true;
            }
        } else {
            files.push(p.clone());
        }
    }

    for path in &files {
        match fs::read_to_string(path) {
            Ok(src) => {
                let name = path.display().to_string();
                trouble |= !score_into(&mut rows, &name, &src, cfg);
            }
            Err(e) => {
                eprintln!("rustcx: {}: {e}", path.display());
                trouble = true;
            }
        }
    }

    let rows = select(rows, cfg);
    let summary = summarize(&rows);
    let stdout = io::stdout();
    let mut w = stdout.lock();
    let wrote = if cfg.json {
        write_json(&mut w, &rows, &summary)
    } else {
        write_table(&mut w, &rows, &summary, cfg.header, cfg.explain)
    };
    if wrote.is_err() {
        return ExitCode::from(2);
    }

    if trouble {
        return ExitCode::from(2);
    }
    if let Some(max) = cfg.max {
        if rows.iter().any(|r| r.func.complexity > max) {
            return ExitCode::from(1);
        }
    }
    ExitCode::SUCCESS
}

/// Analyze one source and append its rows. Returns false on a parse error.
///
/// This function owns the span-lifetime invariant: `analyze` resolves every
/// span to a line before returning, so the thread-local span table — which
/// otherwise retains the full text of every file ever parsed — is cleared here.
/// Nothing borrowed from the AST may outlive this call.
fn score_into(rows: &mut Vec<Row>, name: &str, src: &str, cfg: &Config) -> bool {
    let result = rustcx::analyze::analyze(src, cfg.opts);
    proc_macro2::extra::invalidate_current_thread_spans();
    match result {
        Ok(funcs) => {
            for func in funcs {
                rows.push(Row { file: name.to_string(), func });
            }
            true
        }
        Err(e) => {
            eprintln!("rustcx: {}:{}: parse error: {}", name, e.line, e.message);
            false
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cfg = match parse_args(&args) {
        Ok(Some(cfg)) => cfg,
        Ok(None) => return ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("rustcx: {msg}");
            return ExitCode::from(2);
        }
    };
    run(&cfg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustcx::analyze::{FnKind, Point};

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn parse(list: &[&str]) -> Config {
        parse_args(&args(list)).unwrap().unwrap()
    }

    #[test]
    fn defaults_match_the_spec() {
        let c = parse(&["src/"]);
        assert_eq!(c.paths, vec![PathBuf::from("src/")]);
        assert_eq!(c.min, 1);
        assert_eq!(c.max, None);
        assert_eq!(c.top, None);
        assert!(!c.sort);
        assert!(c.header);
        assert!(!c.json);
        assert!(!c.explain);
        assert!(c.opts.logical, "&& and || count by default");
        assert!(!c.opts.question, "? does not count by default");
        assert!(c.opts.tests, "test code is scored by default");
        assert!(!c.stdin);
    }

    #[test]
    fn scoring_flags_flip_their_options() {
        assert!(!parse(&["--no-logical", "x.rs"]).opts.logical);
        assert!(parse(&["--question", "x.rs"]).opts.question);
        assert!(!parse(&["--no-tests", "x.rs"]).opts.tests);
    }

    #[test]
    fn numeric_flags_accept_short_and_long_forms() {
        assert_eq!(parse(&["-m", "3", "x.rs"]).min, 3);
        assert_eq!(parse(&["--min", "3", "x.rs"]).min, 3);
        assert_eq!(parse(&["-M", "15", "x.rs"]).max, Some(15));
        assert_eq!(parse(&["-t", "20", "x.rs"]).top, Some(20));
    }

    #[test]
    fn boolean_flags_are_recognised() {
        assert!(parse(&["-s", "x.rs"]).sort);
        assert!(parse(&["-j", "x.rs"]).json);
        assert!(parse(&["-e", "x.rs"]).explain);
        assert!(!parse(&["--no-header", "x.rs"]).header);
    }

    #[test]
    fn a_lone_dash_means_stdin() {
        let c = parse(&["-"]);
        assert!(c.stdin);
        assert!(c.paths.is_empty());
    }

    #[test]
    fn help_and_version_return_none() {
        assert!(parse_args(&args(&["--help"])).unwrap().is_none());
        assert!(parse_args(&args(&["-V"])).unwrap().is_none());
    }

    #[test]
    fn errors_are_reported_not_panicked() {
        assert!(parse_args(&args(&[])).is_err(), "no paths is an error");
        assert!(parse_args(&args(&["--nope", "x.rs"])).is_err());
        assert!(parse_args(&args(&["-m", "x.rs"])).is_err(), "-m needs a number");
        assert!(parse_args(&args(&["-m"])).is_err(), "-m needs a value at all");
    }

    fn func(name: &str, kind: FnKind, line: u32, cc: u32, depth: u32) -> Func {
        Func {
            name: name.to_string(),
            kind,
            line,
            complexity: cc,
            depth,
            points: Vec::new(),
        }
    }

    fn sample() -> Vec<Row> {
        vec![
            Row { file: "src/a.rs".into(), func: func("validate", FnKind::Fn, 12, 14, 0) },
            Row { file: "src/a.rs".into(), func: func("map", FnKind::Closure, 61, 3, 1) },
            Row { file: "src/b.rs".into(), func: func("S::go", FnKind::Method, 4, 2, 0) },
        ]
    }

    fn render(rows: &[Row], header: bool, explain: bool) -> String {
        colored::control::set_override(false);
        let s = summarize(rows);
        let mut out = Vec::new();
        write_table(&mut out, rows, &s, header, explain).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn table_groups_by_file_and_indents_by_depth() {
        let expected = "\
src/a.rs
    CC   Line  Kind         Name
    14     12  fn           validate
     3     61  closure        map

src/b.rs
    CC   Line  Kind         Name
     2      4  method       S::go

3 functions in 2 files · mean 6.3 · worst 14 (validate at src/a.rs:12)
";
        assert_eq!(render(&sample(), true, false), expected);
    }

    #[test]
    fn no_header_drops_the_column_row() {
        let out = render(&sample(), false, false);
        assert!(!out.contains("CC   Line"));
        assert!(out.contains("    14     12  fn           validate"));
    }

    #[test]
    fn explain_lists_the_decision_points() {
        let mut rows = sample();
        rows[0].func.points = vec![
            Point { line: 13, branch: "if" },
            Point { line: 14, branch: "&&" },
        ];
        let out = render(&rows, true, true);
        assert!(out.contains("           13  if\n"), "got:\n{out}");
        assert!(out.contains("           14  &&\n"), "got:\n{out}");
    }

    #[test]
    fn summary_is_singular_for_one_function() {
        let rows = vec![Row { file: "a.rs".into(), func: func("f", FnKind::Fn, 1, 2, 0) }];
        let out = render(&rows, true, false);
        assert!(out.contains("1 function in 1 file · mean 2.0 · worst 2 (f at a.rs:1)"), "got:\n{out}");
    }

    #[test]
    fn empty_input_reports_nothing_found() {
        let out = render(&[], true, false);
        assert_eq!(out, "0 functions in 0 files\n");
    }

    fn selected(cfg: Config, rows: Vec<Row>) -> Vec<(String, u32)> {
        select(rows, &cfg).into_iter().map(|r| (r.func.name, r.func.complexity)).collect()
    }

    fn spread() -> Vec<Row> {
        vec![
            Row { file: "a.rs".into(), func: func("low", FnKind::Fn, 1, 1, 0) },
            Row { file: "a.rs".into(), func: func("high", FnKind::Fn, 2, 9, 0) },
            Row { file: "b.rs".into(), func: func("mid", FnKind::Fn, 3, 5, 0) },
        ]
    }

    #[test]
    fn min_filters_below_the_threshold() {
        let cfg = Config { min: 5, ..Config::default() };
        assert_eq!(selected(cfg, spread()), [("high".to_string(), 9), ("mid".to_string(), 5)]);
    }

    #[test]
    fn min_of_one_keeps_everything_in_source_order() {
        let cfg = Config::default();
        let names: Vec<String> = selected(cfg, spread()).into_iter().map(|r| r.0).collect();
        assert_eq!(names, ["low", "high", "mid"]);
    }

    #[test]
    fn sort_orders_by_score_descending() {
        let cfg = Config { sort: true, ..Config::default() };
        let names: Vec<String> = selected(cfg, spread()).into_iter().map(|r| r.0).collect();
        assert_eq!(names, ["high", "mid", "low"]);
    }

    #[test]
    fn top_keeps_the_n_worst_and_restores_source_order() {
        // --top picks the worst 2, but without --sort they print in source
        // order so the table still reads file by file.
        let cfg = Config { top: Some(2), ..Config::default() };
        let names: Vec<String> = selected(cfg, spread()).into_iter().map(|r| r.0).collect();
        assert_eq!(names, ["high", "mid"]);
    }

    #[test]
    fn top_with_sort_stays_sorted() {
        let cfg = Config { top: Some(2), sort: true, ..Config::default() };
        let names: Vec<String> = selected(cfg, spread()).into_iter().map(|r| r.0).collect();
        assert_eq!(names, ["high", "mid"]);
    }

    #[test]
    fn json_carries_every_row_and_the_summary() {
        let mut rows = sample();
        rows[0].func.points = vec![Point { line: 13, branch: "if" }];
        let mut out = Vec::new();
        let s = summarize(&rows);
        write_json(&mut out, &rows, &s).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();

        assert_eq!(v["functions"].as_array().unwrap().len(), 3);
        assert_eq!(v["functions"][0]["file"], "src/a.rs");
        assert_eq!(v["functions"][0]["name"], "validate");
        assert_eq!(v["functions"][0]["kind"], "fn");
        assert_eq!(v["functions"][0]["complexity"], 14);
        assert_eq!(v["functions"][0]["line"], 12);
        assert_eq!(v["functions"][0]["depth"], 0);
        assert_eq!(v["functions"][0]["points"][0]["branch"], "if");
        assert_eq!(v["functions"][1]["kind"], "closure");

        assert_eq!(v["summary"]["functions"], 3);
        assert_eq!(v["summary"]["files"], 2);
        assert_eq!(v["summary"]["worst"]["name"], "validate");
    }
}
