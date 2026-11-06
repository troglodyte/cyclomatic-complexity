use jscx::analyze::{analyze as analyze_src, Func, Options};
use colored::*;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const HELP: &str = "\
jscx -- cyclomatic complexity for JavaScript and TypeScript

USAGE:
    jscx [OPTIONS] <PATH>...
    jscx [OPTIONS] -            read source from stdin

OPTIONS:
    -m, --min <N>        only report functions scoring N or higher [default: 1]
    -M, --max <N>        exit with status 1 if any function scores above N
    -s, --sort           sort all results by score, highest first
    -j, --json           emit JSON instead of a table
    -e, --explain        list the decision points behind each score
    -t, --top <N>        show only the N worst functions (implies --sort)
        --no-header      do not print the table header
        --lsp            run as a Language Server
        --optional-chain count `?.` as a branch (off by default, as in ESLint)
        --no-logical     do not count `&&`, `||`, `??`
        --jsx            force JSX lexing on (default: on except for .ts/.mts/.cts)
        --no-jsx         force JSX lexing off
        --include-dts    do not skip .d.ts files
        --ext <EXT>      extension to assume for stdin [default: tsx]
    -h, --help           print this help
    -V, --version        print version

SCORING
    Starts at 1 and adds one for each `if`, loop, `case`, `catch`, ternary, and
    each `&&` / `||` / `??`. This matches ESLint's `complexity` rule, so scores
    are comparable. Nested functions are scored independently.

EXIT STATUS
    0  success
    1  a function exceeded --max
    2  a path could not be read
";

const SOURCE_EXTS: &[&str] = &["js", "mjs", "cjs", "jsx", "ts", "tsx", "mts", "cts"];

const SKIP_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    ".hg",
    ".svn",
    "dist",
    "build",
    "out",
    "coverage",
    ".next",
    ".nuxt",
    ".svelte-kit",
    "vendor",
    "target",
    ".venv",
    "__pycache__",
    ".cache",
    ".turbo",
    ".output",
];

struct Config {
    paths: Vec<PathBuf>,
    min: u32,
    max: Option<u32>,
    sort: bool,
    json: bool,
    explain: bool,
    top: Option<usize>,
    no_header: bool,
    lsp: bool,
    jsx_override: Option<bool>,
    include_dts: bool,
    stdin_ext: String,
    use_stdin: bool,
    opts: Options,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            paths: Vec::new(),
            min: 1,
            max: None,
            sort: false,
            json: false,
            explain: false,
            top: None,
            no_header: false,
            lsp: false,
            jsx_override: None,
            include_dts: false,
            stdin_ext: "tsx".to_string(),
            use_stdin: false,
            opts: Options::default(),
        }
    }
}

/// A scored function together with the file it came from.
struct Row {
    file: String,
    func: Func,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cfg = match parse_args(&args) {
        Ok(Some(cfg)) => cfg,
        Ok(None) => return ExitCode::SUCCESS, // --help / --version
        Err(msg) => {
            eprintln!("jscx: {msg}\n\nTry `jscx --help`.");
            return ExitCode::from(2);
        }
    };

    if cfg.lsp {
        if let Err(e) = jscx::lsp::run() {
            eprintln!("jscx: LSP error: {e}");
            return ExitCode::from(2);
        }
        return ExitCode::SUCCESS;
    }

    let mut rows: Vec<Row> = Vec::new();
    let mut files_scanned = 0usize;
    let mut had_error = false;

    if cfg.use_stdin {
        let mut src = String::new();
        if io::stdin().read_to_string(&mut src).is_err() {
            eprintln!("jscx: could not read stdin");
            return ExitCode::from(2);
        }
        let jsx = cfg
            .jsx_override
            .unwrap_or_else(|| jsx_default(&cfg.stdin_ext));
        files_scanned += 1;
        for f in analyze_src(&src, jsx, cfg.opts) {
            rows.push(Row {
                file: "<stdin>".to_string(),
                func: f,
            });
        }
    } else {
        let mut files: Vec<PathBuf> = Vec::new();
        for p in &cfg.paths {
            if let Err(e) = collect(p, &cfg, &mut files) {
                eprintln!("jscx: {}: {e}", p.display());
                had_error = true;
            }
        }
        for path in files {
            let bytes = match fs::read(&path) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("jscx: {}: {e}", path.display());
                    had_error = true;
                    continue;
                }
            };
            let src = String::from_utf8_lossy(&bytes);
            let ext = ext_of(&path);
            let jsx = cfg.jsx_override.unwrap_or_else(|| jsx_default(&ext));
            files_scanned += 1;
            let name = path.display().to_string();
            for f in analyze_src(&src, jsx, cfg.opts) {
                rows.push(Row {
                    file: name.clone(),
                    func: f,
                });
            }
        }
    }

    let total = rows.len();
    let mean = if total == 0 {
        0.0
    } else {
        rows.iter().map(|r| r.func.complexity as f64).sum::<f64>() / total as f64
    };
    let worst_line = rows.iter().max_by_key(|r| r.func.complexity).map(|r| {
        (
            r.func.name.clone(),
            r.func.complexity,
            r.file.clone(),
            r.func.line,
        )
    });
    let over = match cfg.max {
        Some(m) => rows.iter().filter(|r| r.func.complexity > m).count(),
        None => 0,
    };
    let summary = Summary {
        total,
        files: files_scanned,
        mean,
        over,
        max: cfg.max,
    };

    // Filter and order for display.
    rows.retain(|r| r.func.complexity >= cfg.min);
    if cfg.sort || cfg.top.is_some() {
        rows.sort_by(|a, b| {
            b.func
                .complexity
                .cmp(&a.func.complexity)
                .then(a.file.cmp(&b.file))
                .then(a.func.line.cmp(&b.func.line))
        });
    } else {
        rows.sort_by(|a, b| {
            a.file
                .cmp(&b.file)
                .then(a.func.line.cmp(&b.func.line))
                .then(a.func.depth.cmp(&b.func.depth))
        });
    }
    if let Some(n) = cfg.top {
        rows.truncate(n);
    }

    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    let flat = cfg.sort || cfg.top.is_some();
    let res = if cfg.json {
        write_json(&mut out, &rows, &summary)
    } else {
        write_table(
            &mut out,
            &rows,
            &summary,
            worst_line,
            flat,
            cfg.explain,
            cfg.no_header,
        )
    };
    if let Err(e) = res {
        if e.kind() != io::ErrorKind::BrokenPipe {
            eprintln!("jscx: {e}");
            return ExitCode::from(2);
        }
    }
    let _ = out.flush();

    if had_error {
        return ExitCode::from(2);
    }
    if over > 0 {
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

struct Summary {
    total: usize,
    files: usize,
    mean: f64,
    over: usize,
    max: Option<u32>,
}

// ------------------------------------------------------------------ arguments

fn parse_args(args: &[String]) -> Result<Option<Config>, String> {
    let mut cfg = Config::default();
    let mut i = 0;
    let mut no_more_flags = false;

    while i < args.len() {
        let a = args[i].as_str();

        if no_more_flags || (!a.starts_with('-') && a != "-") {
            cfg.paths.push(PathBuf::from(a));
            i += 1;
            continue;
        }

        // Split `--flag=value`.
        let (flag, inline) = match a.find('=') {
            Some(p) if a.starts_with("--") => (&a[..p], Some(a[p + 1..].to_string())),
            _ => (a, None),
        };

        match flag {
            "-" => {
                cfg.use_stdin = true;
            }
            "--" => no_more_flags = true,
            "-h" | "--help" => {
                print!("{HELP}");
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("jscx {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "-m" | "--min" => {
                let v = take_value(args, &mut i, &inline, "--min")?;
                cfg.min = v.parse().map_err(|_| format!("invalid --min: {v}"))?;
            }
            "-M" | "--max" => {
                let v = take_value(args, &mut i, &inline, "--max")?;
                cfg.max = Some(v.parse().map_err(|_| format!("invalid --max: {v}"))?);
            }
            "-t" | "--top" => {
                let v = take_value(args, &mut i, &inline, "--top")?;
                cfg.top = Some(v.parse().map_err(|_| format!("invalid --top: {v}"))?);
            }
            "--ext" => {
                let v = take_value(args, &mut i, &inline, "--ext")?;
                cfg.stdin_ext = v.trim_start_matches('.').to_ascii_lowercase();
            }
            "-s" | "--sort" => cfg.sort = true,
            "-j" | "--json" => cfg.json = true,
            "-e" | "--explain" => cfg.explain = true,
            "--no-header" => cfg.no_header = true,
            "--lsp" => cfg.lsp = true,
            "--optional-chain" => cfg.opts.optional_chain = true,
            "--no-logical" => cfg.opts.logical = false,
            "--jsx" => cfg.jsx_override = Some(true),
            "--no-jsx" => cfg.jsx_override = Some(false),
            "--include-dts" => cfg.include_dts = true,
            other => return Err(format!("unknown option `{other}`")),
        }
        i += 1;
    }

    if cfg.paths.is_empty() && !cfg.use_stdin {
        cfg.paths.push(PathBuf::from("."));
    }
    Ok(Some(cfg))
}

/// Read the value for a flag, either from `--flag=value` or the next argument.
fn take_value(
    args: &[String],
    i: &mut usize,
    inline: &Option<String>,
    name: &str,
) -> Result<String, String> {
    if let Some(v) = inline {
        return Ok(v.clone());
    }
    *i += 1;
    args.get(*i)
        .cloned()
        .ok_or_else(|| format!("{name} requires a value"))
}

// ----------------------------------------------------------------- file walk

fn ext_of(p: &Path) -> String {
    p.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// JSX lexing is on everywhere except `.ts`/`.mts`/`.cts`, where `<` is always
/// a comparison or a type argument.
fn jsx_default(ext: &str) -> bool {
    !matches!(ext, "ts" | "mts" | "cts")
}

fn is_source(p: &Path, include_dts: bool) -> bool {
    if !SOURCE_EXTS.contains(&ext_of(p).as_str()) {
        return false;
    }
    if !include_dts {
        if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
            if name.ends_with(".d.ts") || name.ends_with(".d.mts") || name.ends_with(".d.cts") {
                return false;
            }
        }
    }
    true
}

fn collect(root: &Path, cfg: &Config, out: &mut Vec<PathBuf>) -> io::Result<()> {
    let meta = fs::metadata(root)?;
    if meta.is_file() {
        out.push(root.to_path_buf());
        return Ok(());
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("jscx: {}: {e}", dir.display());
                continue;
            }
        };
        let mut found: Vec<PathBuf> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if file_type.is_dir() {
                let dir_name = entry.file_name().to_string_lossy().into_owned();
                if SKIP_DIRS.contains(&dir_name.as_str()) {
                    continue;
                }
                stack.push(path);
            } else if file_type.is_file() && is_source(&path, cfg.include_dts) {
                found.push(path);
            }
        }
        found.sort();
        out.append(&mut found);
    }
    out.sort();
    out.dedup();
    Ok(())
}

// -------------------------------------------------------------------- output

fn write_table<W: Write>(
    w: &mut W,
    rows: &[Row],
    s: &Summary,
    worst: Option<(String, u32, String, u32)>,
    flat: bool,
    explain: bool,
    no_header: bool,
) -> io::Result<()> {
    writeln!(w, "{}", "==> Running jscx <==".yellow())?;
    if rows.is_empty() {
        writeln!(w, "no functions matched")?;
    } else if flat {
        if !no_header {
            writeln!(
                w,
                "{}",
                format!("{:>5}  {:<14}  {:<12} {}", "CC", "Location", "Kind", "Name").blue()
            )?;
        }
        for r in rows {
            writeln!(
                w,
                "{:>5}  {}:{}  {} {}",
                r.func.complexity,
                r.file.cyan(),
                r.func.line,
                r.func.kind.as_str(),
                r.func.name
            )?;
            if explain {
                write_points(w, r)?;
            }
        }
    } else {
        let mut current = "";
        for r in rows {
            if r.file != current {
                if !current.is_empty() {
                    writeln!(w)?;
                }
                writeln!(w, "{}", r.file.cyan().bold())?;
                if !no_header {
                    writeln!(
                        w,
                        "{}",
                        format!("  {:>4}  {:>5}  {:<12} {}", "CC", "Line", "Kind", "Name").blue()
                    )?;
                }
                current = r.file.as_str();
            }
            let indent = "  ".repeat(r.func.depth as usize);
            writeln!(
                w,
                "  {:>4}  {:>5}  {:<12} {}{}",
                r.func.complexity,
                r.func.line,
                r.func.kind.as_str(),
                indent,
                r.func.name
            )?;
            if explain {
                write_points(w, r)?;
            }
        }
    }

    writeln!(w)?;
    write!(
        w,
        "{} function{} in {} file{}",
        s.total,
        plural(s.total),
        s.files,
        plural(s.files)
    )?;
    if s.total > 0 {
        write!(w, "\n\t \u{b7} mean {:.1}", s.mean)?;
        if let Some((name, cc, file, line)) = worst {
            write!(w, "\n\t \u{b7} worst {cc} ({name} at {file}:{line})")?;
        }
    }
    writeln!(w)?;
    if let Some(m) = s.max {
        if s.over > 0 {
            writeln!(
                w,
                "{} function{} above the limit of {}",
                s.over,
                plural(s.over),
                m
            )?;
        } else {
            writeln!(w, "all functions within the limit of {m}")?;
        }
    }
    Ok(())
}

fn write_points<W: Write>(w: &mut W, r: &Row) -> io::Result<()> {
    for p in &r.func.points {
        writeln!(w, "          line {:<5} {}", p.line, p.branch)?;
    }
    Ok(())
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

fn write_json<W: Write>(w: &mut W, rows: &[Row], s: &Summary) -> io::Result<()> {
    writeln!(w, "{{")?;
    writeln!(w, "  \"summary\": {{")?;
    writeln!(w, "    \"functions\": {},", s.total)?;
    writeln!(w, "    \"files\": {},", s.files)?;
    writeln!(w, "    \"mean\": {:.3},", s.mean)?;
    match s.max {
        Some(m) => writeln!(w, "    \"limit\": {m},")?,
        None => writeln!(w, "    \"limit\": null,")?,
    }
    writeln!(w, "    \"overLimit\": {}", s.over)?;
    writeln!(w, "  }},")?;
    writeln!(w, "  \"functions\": [")?;
    for (i, r) in rows.iter().enumerate() {
        let comma = if i + 1 == rows.len() { "" } else { "," };
        write!(w, "    {{")?;
        write!(w, "\"file\": \"{}\", ", esc(&r.file))?;
        write!(w, "\"name\": \"{}\", ", esc(&r.func.name))?;
        write!(w, "\"kind\": \"{}\", ", r.func.kind.as_str())?;
        write!(w, "\"line\": {}, ", r.func.line)?;
        write!(w, "\"depth\": {}, ", r.func.depth)?;
        write!(w, "\"complexity\": {}, ", r.func.complexity)?;
        write!(w, "\"points\": [")?;
        for (k, p) in r.func.points.iter().enumerate() {
            if k > 0 {
                write!(w, ", ")?;
            }
            write!(
                w,
                "{{\"line\": {}, \"branch\": \"{}\"}}",
                p.line,
                esc(p.branch)
            )?;
        }
        writeln!(w, "]}}{comma}")?;
    }
    writeln!(w, "  ]")?;
    writeln!(w, "}}")?;
    Ok(())
}

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}
