//! Argument parsing and `--help` for every cmplx binary.
//!
//! Shared flags are fixed here. Language flags (`--js`, `--rust`, …) come from
//! the registry's language ids and exist only when the registry is complete (an
//! alias has its language fixed). Language *option* flags (`--no-tests`, …) come
//! from each language's [`OptionSpec`]s and are checked against the active
//! languages once every argument has been read, so their order is irrelevant.

use crate::config::FileConfig;
use crate::language::{Lang, Registry};
use std::fmt;
use std::path::PathBuf;

/// What the command line asked for.
pub struct Cli {
    /// `"cmplx"` or an alias name, for `--help` and `-V`.
    pub program: &'static str,
    pub paths: Vec<PathBuf>,
    pub stdin: bool,
    /// `--ext`: lower-cased, without a leading dot.
    pub ext: Option<String>,
    pub min: u32,
    /// `--max`; mirrors `flags.max`. Also recorded in `flags.max` so that `flags.or(file)` resolves
    /// it exactly as rustcx merged its CLI layer over `rustcx.toml`.
    pub max: Option<u32>,
    pub top: Option<usize>,
    pub sort: bool,
    pub json: bool,
    pub explain: bool,
    pub no_header: bool,
    pub lsp: bool,
    /// Explicit language flags, else every registered language.
    pub active: Vec<Lang>,
    /// True when an alias or a language flag chose the languages.
    pub explicit_lang: bool,
    /// The CLI layer of the config: only what the flags set.
    pub flags: FileConfig,
}

impl fmt::Debug for Cli {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Cli")
            .field("program", &self.program)
            .field("paths", &self.paths)
            .field("stdin", &self.stdin)
            .field("active", &language_ids(&self.active))
            .field("flags", &self.flags)
            .finish_non_exhaustive()
    }
}

/// A flag that consumes a value, as `--flag value` or `--flag=value`.
#[derive(Clone, Copy)]
enum ValueFlag {
    Min,
    Max,
    Top,
    Ext,
}

const VALUE_FLAGS: &[(&str, &str, ValueFlag)] = &[
    ("-m", "--min", ValueFlag::Min),
    ("-M", "--max", ValueFlag::Max),
    ("-t", "--top", ValueFlag::Top),
    ("", "--ext", ValueFlag::Ext),
];

/// Everything gathered before the active languages are known.
struct Parser<'a> {
    cli: Cli,
    reg: &'a Registry,
    /// Languages named by `--<id>` flags.
    selected: Vec<Lang>,
    /// Option flags in command-line order, checked against the active set at the end.
    option_flags: Vec<String>,
    /// An output option was given (`--min`, `--max`, `--top`, `--sort`,
    /// `--json`, `--explain`, `--no-header` or `--ext`), which `--lsp` refuses.
    output_option: bool,
}

enum Step {
    Next,
    Stop,
}

impl Parser<'_> {
    fn arg(
        &mut self,
        args: &[String],
        i: &mut usize,
        flags_done: &mut bool,
    ) -> Result<Step, String> {
        let arg = args[*i].as_str();
        if *flags_done || arg == "-" || !arg.starts_with('-') {
            self.positional(arg);
            return Ok(Step::Next);
        }
        let (flag, inline) = split_inline(arg);
        if let Some((_, _, kind)) = VALUE_FLAGS
            .iter()
            .find(|(s, l, _)| flag == *s || flag == *l)
        {
            let value = take_value(args, i, flag, inline)?;
            self.output_option = true;
            return self.value(*kind, flag, &value).map(|()| Step::Next);
        }
        if matches!(
            arg,
            "-s" | "--sort" | "-j" | "--json" | "-e" | "--explain" | "--no-header"
        ) {
            self.output_option = true;
        }
        match arg {
            "--" => *flags_done = true,
            "-h" | "--help" => {
                print!("{}", help(self.cli.program, self.reg));
                return Ok(Step::Stop);
            }
            "-V" | "--version" => {
                println!("{} {}", self.cli.program, env!("CARGO_PKG_VERSION"));
                return Ok(Step::Stop);
            }
            "-s" | "--sort" => self.cli.sort = true,
            "-j" | "--json" => self.cli.json = true,
            "-e" | "--explain" => self.cli.explain = true,
            "--no-header" => self.cli.no_header = true,
            "--lsp" => self.cli.lsp = true,
            _ => self.language_or_option(arg)?,
        }
        Ok(Step::Next)
    }

    fn positional(&mut self, arg: &str) {
        if arg == "-" {
            self.cli.stdin = true;
        } else {
            self.cli.paths.push(PathBuf::from(arg));
        }
    }

    fn value(&mut self, kind: ValueFlag, flag: &str, raw: &str) -> Result<(), String> {
        match kind {
            ValueFlag::Min => self.cli.min = number(flag, raw)?,
            ValueFlag::Max => {
                let max = number(flag, raw)?;
                self.cli.max = Some(max);
                self.cli.flags.max = Some(max);
            }
            ValueFlag::Top => self.cli.top = Some(number(flag, raw)? as usize),
            ValueFlag::Ext => {
                let ext = raw.trim_start_matches('.');
                if ext.is_empty() {
                    return Err(format!("{flag} needs an extension, got: {raw:?}"));
                }
                self.cli.ext = Some(ext.to_ascii_lowercase());
            }
        }
        Ok(())
    }

    /// `--<id>` selects a language (complete registries only); any other `--flag`
    /// is held back as a possible language option. A bare `-x` is a typo, not a
    /// filename — reporting it beats scanning for a file named `-x`.
    fn language_or_option(&mut self, arg: &str) -> Result<(), String> {
        if !arg.starts_with("--") {
            return Err(format!("unknown option: {arg}"));
        }
        let chosen = self
            .reg
            .langs()
            .iter()
            .copied()
            .find(|lang| self.reg.is_complete() && arg.strip_prefix("--") == Some(lang.id()));
        match chosen {
            Some(lang) => self.selected.push(lang),
            None => self.option_flags.push(arg.to_string()),
        }
        Ok(())
    }

    fn finish(mut self) -> Result<Cli, String> {
        let reg = self.reg;
        self.cli.explicit_lang = !reg.is_complete() || !self.selected.is_empty();
        self.cli.active = if self.selected.is_empty() {
            reg.langs().to_vec()
        } else {
            let chosen = |lang: &&Lang| self.selected.iter().any(|s| s.id() == lang.id());
            reg.langs().iter().filter(chosen).copied().collect()
        };
        self.apply_option_flags()?;
        let lsp_misused = self.output_option || self.cli.stdin || !self.cli.paths.is_empty();
        if self.cli.lsp && lsp_misused {
            return Err("--lsp takes no paths or output options".to_string());
        }
        let mut cli = self.cli;
        if cli.stdin {
            check_stdin_language(&cli, reg)?;
        }
        if cli.paths.is_empty() && !cli.stdin && !cli.lsp {
            cli.paths.push(PathBuf::from("."));
        }
        Ok(cli)
    }

    /// Set the options the held-back flags name. A flag no active language
    /// declares is an error, and so is a flag whose option an earlier flag set
    /// the other way (`--jsx --no-jsx`).
    fn apply_option_flags(&mut self) -> Result<(), String> {
        // (language id, option key, value, the flag that set it)
        let mut set: Vec<(&str, &str, bool, &str)> = Vec::new();
        for flag in &self.option_flags {
            let mut declared = false;
            for lang in &self.cli.active {
                for spec in lang.options() {
                    let value = match (spec.on_flag, spec.off_flag) {
                        (Some(on), _) if on == flag => true,
                        (_, Some(off)) if off == flag => false,
                        _ => continue, // this option has no such flag
                    };
                    let earlier = set.iter().find(|(id, key, was, _)| {
                        *id == lang.id() && *key == spec.key && *was != value
                    });
                    if let Some((_, _, _, first)) = earlier {
                        return Err(format!("{first} and {flag} contradict each other"));
                    }
                    set.push((lang.id(), spec.key, value, flag));
                    self.cli.flags.set_option(*lang, spec.key, value);
                    declared = true;
                }
            }
            if !declared {
                let names = language_ids(&self.cli.active).join(", ");
                return Err(format!("{flag} is not an option for {names}"));
            }
        }
        Ok(())
    }
}

fn check_stdin_language(cli: &Cli, reg: &Registry) -> Result<(), String> {
    if !cli.explicit_lang {
        return Err(format!(
            "reading stdin needs a language: pass one of {}",
            reg.langs()
                .iter()
                .map(|lang| format!("--{}", lang.id()))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if cli.active.len() != 1 {
        return Err(format!(
            "reading stdin needs exactly one language, got: {}",
            language_ids(&cli.active).join(", ")
        ));
    }
    Ok(())
}

fn language_ids(langs: &[Lang]) -> Vec<&'static str> {
    langs.iter().map(|lang| lang.id()).collect()
}

/// Split `--flag=value`; only long flags take the inline form.
fn split_inline(arg: &str) -> (&str, Option<&str>) {
    match arg.split_once('=') {
        Some((flag, value)) if flag.starts_with("--") => (flag, Some(value)),
        _ => (arg, None),
    }
}

/// The value for `flag`, inline or the next argument (advancing `i`).
fn take_value(
    args: &[String],
    i: &mut usize,
    flag: &str,
    inline: Option<&str>,
) -> Result<String, String> {
    if let Some(value) = inline {
        return Ok(value.to_string());
    }
    *i += 1;
    args.get(*i)
        .cloned()
        .ok_or_else(|| format!("{flag} needs a value"))
}

fn number(flag: &str, raw: &str) -> Result<u32, String> {
    raw.parse::<u32>()
        .map_err(|_| format!("{flag} needs a number, got: {raw}"))
}

/// Parse argv (without the program name).
///
/// `Ok(None)` means help or version was printed and the caller should exit 0.
pub fn parse_args(
    program: &'static str,
    args: &[String],
    reg: &Registry,
) -> Result<Option<Cli>, String> {
    let cli = Cli {
        program,
        paths: Vec::new(),
        stdin: false,
        ext: None,
        min: 1,
        max: None,
        top: None,
        sort: false,
        json: false,
        explain: false,
        no_header: false,
        lsp: false,
        active: Vec::new(),
        explicit_lang: false,
        flags: FileConfig::default(),
    };
    let mut parser = Parser {
        cli,
        reg,
        selected: Vec::new(),
        option_flags: Vec::new(),
        output_option: false,
    };
    let (mut i, mut flags_done) = (0, false);
    while i < args.len() {
        if let Step::Stop = parser.arg(args, &mut i, &mut flags_done)? {
            return Ok(None);
        }
        i += 1;
    }
    parser.finish().map(Some)
}

/// The `--help` text: shared flags, then each language's options under its name.
pub fn help(program: &'static str, reg: &Registry) -> String {
    let mut out = format!("{program} — cyclomatic complexity\n\nUSAGE:\n");
    if reg.is_complete() {
        let flags: Vec<String> = reg.langs().iter().map(|l| l.id().to_string()).collect();
        out.push_str(&format!(
            "    {program} [OPTIONS] [--{}] <PATH>...\n",
            flags.join("|--")
        ));
        out.push_str(&format!(
            "    {program} [OPTIONS] --<LANG> -     read from stdin\n"
        ));
    } else {
        out.push_str(&format!("    {program} [OPTIONS] <PATH>...\n"));
        out.push_str(&format!("    {program} [OPTIONS] -     read from stdin\n"));
    }
    out.push_str(SHARED_HELP);
    if reg.is_complete() {
        out.push_str("\nLANGUAGES:\n");
        for lang in reg.langs() {
            out.push_str(&format!(
                "    {:<22}{}\n",
                format!("--{}", lang.id()),
                lang.display_name()
            ));
        }
    }
    for lang in reg.langs() {
        out.push_str(&format!("\n{} OPTIONS:\n", lang.display_name()));
        for spec in lang.options() {
            let flags: Vec<&str> = [spec.on_flag, spec.off_flag]
                .into_iter()
                .flatten()
                .collect();
            out.push_str(&format!("    {:<22}{}\n", flags.join(", "), spec.help));
        }
    }
    out
}

const SHARED_HELP: &str = "
OPTIONS:
    -m, --min <N>         only report functions scoring N or higher (default 1)
    -M, --max <N>         exit 1 if any function scores above N — for CI
    -t, --top <N>         show only the N worst functions
    -s, --sort            sort by score, highest first
        --no-header       do not print the table header
    -j, --json            machine-readable output
    -e, --explain         list the decision points behind each score
        --ext <EXT>       extension to assume for stdin
        --lsp             run as a Language Server (stdio)
    -h, --help            print this help
    -V, --version         print version
";
