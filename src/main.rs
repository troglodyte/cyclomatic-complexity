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
        --lsp          run as a Language Server (stdio)
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
    lsp: bool,
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
            lsp: false,
            opts: Options::default(),
        }
    }
}

/// What an option does when it appears on the command line.
enum Action {
    /// A flag with no value.
    Set(fn(&mut Config)),
    /// A flag that consumes the next argument as a number.
    Num(fn(&mut Config, u32)),
    /// Prints something and ends the run successfully.
    Terminal(fn()),
}

/// Every option, in help order: short form, long form, what it does. Either
/// form may be `""` for an option that has only the other one.
///
/// This is a table rather than a `match` so that adding an option is adding a
/// row. It also keeps `parse_args` flat: the arms here are data, so the parser
/// dispatches on three kinds of option instead of on thirteen names.
const FLAGS: &[(&str, &str, Action)] = &[
    ("-h", "--help", Action::Terminal(|| print!("{HELP}"))),
    ("-V", "--version", Action::Terminal(|| println!("rustcx {}", env!("CARGO_PKG_VERSION")))),
    ("-m", "--min", Action::Num(|c, n| c.min = n)),
    ("-M", "--max", Action::Num(|c, n| c.max = Some(n))),
    ("-t", "--top", Action::Num(|c, n| c.top = Some(n as usize))),
    ("-s", "--sort", Action::Set(|c| c.sort = true)),
    ("", "--no-header", Action::Set(|c| c.header = false)),
    ("-j", "--json", Action::Set(|c| c.json = true)),
    ("-e", "--explain", Action::Set(|c| c.explain = true)),
    ("", "--question", Action::Set(|c| c.opts.question = true)),
    ("", "--no-logical", Action::Set(|c| c.opts.logical = false)),
    ("", "--no-tests", Action::Set(|c| c.opts.tests = false)),
    ("", "--lsp", Action::Set(|c| c.lsp = true)),
    ("-", "", Action::Set(|c| c.stdin = true)),
];

/// Find the option `arg` names, by either form. An empty form never matches.
fn lookup(arg: &str) -> Option<&'static Action> {
    FLAGS
        .iter()
        .find(|(short, long, _)| matches_form(short, arg) || matches_form(long, arg))
        .map(|(_, _, action)| action)
}

fn matches_form(form: &str, arg: &str) -> bool {
    !form.is_empty() && form == arg
}

/// A bare argument is a path. Anything else starting with `-` is a typo, not a
/// filename — reporting it beats silently scanning for a file named `--min`.
fn push_path(cfg: &mut Config, arg: &str) -> Result<(), String> {
    if arg.starts_with('-') && arg.len() > 1 {
        return Err(format!("unknown option: {arg}"));
    }
    cfg.paths.push(PathBuf::from(arg));
    Ok(())
}

/// Parse argv (without the program name).
///
/// `Ok(None)` means help or version was printed and the caller should exit 0.
fn parse_args(args: &[String]) -> Result<Option<Config>, String> {
    let mut cfg = Config::default();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match lookup(arg) {
            Some(Action::Terminal(print)) => {
                print();
                return Ok(None);
            }
            Some(Action::Set(apply)) => apply(&mut cfg),
            Some(Action::Num(apply)) => {
                // take_number advances `i` past the value.
                let n = take_number(args, &mut i, arg)?;
                apply(&mut cfg, n);
            }
            None => push_path(&mut cfg, arg)?,
        }
        i += 1;
    }
    if cfg.paths.is_empty() && !cfg.stdin && !cfg.lsp {
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

/// Which display options the table writer needs.
#[derive(Clone, Copy)]
struct Style {
    header: bool,
    explain: bool,
    /// One flat table with a Location column, instead of a section per file.
    flat: bool,
}

/// Column layout, copied from jscx so the two tools' output lines up: score
/// right-aligned in 6, line right-aligned in 7, two spaces, kind left-aligned
/// in 13, then two spaces of indent per nesting level.
///
/// `--sort` orders rows by score across files, which makes grouping by file
/// meaningless — a file's header would reappear every time its rows resurface.
/// Sorted output goes out flat instead, every row carrying its own
/// `file:line`, which is what jscx does.
fn write_table<W: Write>(
    w: &mut W,
    rows: &[Row],
    summary: &Summary,
    style: Style,
) -> io::Result<()> {
    if style.flat {
        write_flat(w, rows, style)?;
    } else {
        write_grouped(w, rows, style)?;
    }
    if !rows.is_empty() {
        writeln!(w)?;
    }
    write_summary(w, summary)
}

/// A section per file, rows in source order and indented by nesting depth.
fn write_grouped<W: Write>(w: &mut W, rows: &[Row], style: Style) -> io::Result<()> {
    let mut current = "";
    for row in rows {
        if row.file != current {
            if !current.is_empty() {
                writeln!(w)?;
            }
            writeln!(w, "{}", row.file.bold())?;
            if style.header {
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
        // 6 for the score, 7 for the line: points sit under the Line column.
        write_points(w, f, style.explain, 13)?;
    }
    Ok(())
}

/// One table, no file sections. The Location column is widened to the longest
/// `file:line` so the Kind and Name columns still line up. Depth is not shown:
/// a sorted row rarely sits next to the function it nests in.
fn write_flat<W: Write>(w: &mut W, rows: &[Row], style: Style) -> io::Result<()> {
    let locs: Vec<String> = rows
        .iter()
        .map(|r| format!("{}:{}", r.file, r.func.line))
        .collect();
    let width = locs
        .iter()
        .map(String::len)
        .max()
        .unwrap_or(0)
        .max("Location".len());

    // No rows, no header: an empty flat table is just the summary, the way an
    // empty grouped one is.
    if style.header && !rows.is_empty() {
        writeln!(w, "{:>6}  {:<width$}  {:<13}Name", "CC", "Location", "Kind", width = width)?;
    }
    for (row, loc) in rows.iter().zip(&locs) {
        let f = &row.func;
        // Pad by hand: `loc` is colored, and a width spec would count the
        // escape bytes and shift the columns right.
        let pad = " ".repeat(width - loc.len());
        writeln!(
            w,
            "{:>6}  {}{}  {:<13}{}",
            f.complexity,
            loc.bold(),
            pad,
            f.kind.label(),
            f.name
        )?;
        // Right-aligned on the Location column's far edge, so a point's line
        // lands under the location it belongs to and its branch under Kind.
        write_points(w, f, style.explain, 8 + width)?;
    }
    Ok(())
}

/// The decision points behind one score, each right-aligned in `width` so it
/// falls under whichever column carries the function's own line number.
fn write_points<W: Write>(w: &mut W, f: &Func, explain: bool, width: usize) -> io::Result<()> {
    if !explain {
        return Ok(());
    }
    for p in &f.points {
        writeln!(w, "{:>width$}  {}", p.line, p.branch, width = width)?;
    }
    Ok(())
}

fn write_summary<W: Write>(w: &mut W, summary: &Summary) -> io::Result<()> {
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
    let mut ok = true;

    if cfg.stdin {
        match read_stdin() {
            // `&=`, not `&&`: score_into must run for its side effect.
            Ok(src) => ok &= score_into(&mut rows, "<stdin>", &src, cfg),
            Err(e) => {
                eprintln!("rustcx: <stdin>: {e}");
                return ExitCode::from(2);
            }
        }
    }
    ok &= score_files(&mut rows, cfg);

    let rows = select(rows, cfg);
    let summary = summarize(&rows);
    if render(&rows, &summary, cfg).is_err() {
        return ExitCode::from(2);
    }
    exit_code(&rows, cfg, ok)
}

fn read_stdin() -> io::Result<String> {
    let mut src = String::new();
    io::Read::read_to_string(&mut io::stdin(), &mut src)?;
    Ok(src)
}

/// Expand the paths given on the command line into files to score. A file named
/// explicitly is taken whatever its extension; a directory is walked for `.rs`.
///
/// Returns false if a directory could not be read.
fn expand_paths(paths: &[PathBuf], files: &mut Vec<PathBuf>) -> bool {
    let mut ok = true;
    for p in paths {
        if p.is_dir() {
            if let Err(e) = collect(p, files) {
                eprintln!("rustcx: {}: {e}", p.display());
                ok = false;
            }
        } else {
            files.push(p.clone());
        }
    }
    ok
}

/// Score every file named or discovered under the command-line paths, appending
/// their rows. Returns false if any path could not be read or parsed.
fn score_files(rows: &mut Vec<Row>, cfg: &Config) -> bool {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut ok = expand_paths(&cfg.paths, &mut files);
    for path in &files {
        match fs::read_to_string(path) {
            Ok(src) => ok &= score_into(rows, &path.display().to_string(), &src, cfg),
            Err(e) => {
                eprintln!("rustcx: {}: {e}", path.display());
                ok = false;
            }
        }
    }
    ok
}

/// Write the report to stdout in whichever format `cfg` selects.
fn render(rows: &[Row], summary: &Summary, cfg: &Config) -> io::Result<()> {
    let stdout = io::stdout();
    let mut w = stdout.lock();
    if cfg.json {
        write_json(&mut w, rows, summary)
    } else {
        let style = Style { header: cfg.header, explain: cfg.explain, flat: cfg.sort };
        write_table(&mut w, rows, summary, style)
    }
}

/// 2 if any path failed, 1 if `--max` was exceeded, 0 otherwise.
fn exit_code(rows: &[Row], cfg: &Config, ok: bool) -> ExitCode {
    if !ok {
        return ExitCode::from(2);
    }
    let over = cfg.max.is_some_and(|max| rows.iter().any(|r| r.func.complexity > max));
    if over {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
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
    if cfg.lsp {
        if let Err(e) = rustcx::lsp::run() {
            eprintln!("rustcx: LSP error: {e}");
            return ExitCode::from(2);
        }
        return ExitCode::SUCCESS;
    }
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
    fn the_lsp_flag_needs_no_paths() {
        let c = parse(&["--lsp"]);
        assert!(c.lsp);
        assert!(c.paths.is_empty(), "a server reads no paths");
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
        write_table(&mut out, rows, &s, Style { header, explain, flat: false }).unwrap();
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

    fn render_flat(rows: &[Row], header: bool, explain: bool) -> String {
        colored::control::set_override(false);
        let s = summarize(rows);
        let mut out = Vec::new();
        write_table(&mut out, rows, &s, Style { header, explain, flat: true }).unwrap();
        String::from_utf8(out).unwrap()
    }

    /// Rows sorted by score interleave the files they came from. Grouping by
    /// file would reprint a header every time a file resurfaced, so sorted
    /// output is one flat table keyed by location.
    fn interleaved() -> Vec<Row> {
        vec![
            Row { file: "src/a.rs".into(), func: func("validate", FnKind::Fn, 12, 14, 0) },
            Row { file: "src/b.rs".into(), func: func("S::go", FnKind::Method, 4, 9, 0) },
            Row { file: "src/a.rs".into(), func: func("map", FnKind::Closure, 61, 3, 1) },
        ]
    }

    #[test]
    fn sorted_output_is_one_flat_table() {
        let out = render_flat(&interleaved(), true, false);
        assert_eq!(out.matches("Location").count(), 1, "got:\n{out}");
        // The bare filename headers of the grouped layout must not appear.
        assert!(!out.contains("\nsrc/a.rs\n"), "got:\n{out}");
        assert!(!out.contains("\nsrc/b.rs\n"), "got:\n{out}");
    }

    #[test]
    fn flat_columns_align_on_the_longest_location() {
        let out = render_flat(&interleaved(), true, false);
        let kind_col = |line: &str| line.find("fn ").or(line.find("method")).or(line.find("closure"));
        let cols: Vec<Option<usize>> = out.lines().skip(1).take(3).map(kind_col).collect();
        assert!(cols.windows(2).all(|w| w[0] == w[1]), "kind column ragged: {cols:?}\n{out}");
    }

    #[test]
    fn flat_layout_drops_the_depth_indent() {
        // `map` is depth 1; sorted rows rarely sit next to what they nest in.
        let out = render_flat(&interleaved(), true, false);
        assert!(out.contains("closure      map\n"), "got:\n{out}");
    }

    #[test]
    fn no_header_drops_the_flat_column_row() {
        let out = render_flat(&interleaved(), false, false);
        assert!(!out.contains("Location"), "got:\n{out}");
    }

    #[test]
    fn empty_flat_table_is_just_the_summary() {
        assert_eq!(render_flat(&[], true, false), "0 functions in 0 files\n");
    }

    #[test]
    fn flat_explain_lines_sit_against_the_location_column() {
        let mut rows = interleaved();
        rows[0].func.points = vec![Point { line: 13, branch: "if" }];
        let out = render_flat(&rows, true, true);
        // Location column is 11 wide ("src/a.rs:12"), so points right-align at 19.
        assert!(out.contains("\n                 13  if\n"), "got:\n{out}");
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
