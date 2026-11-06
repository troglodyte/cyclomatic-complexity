# cmplx — design

One tool for cyclomatic complexity across languages. It replaces two sibling
repos, [`jscx`](https://github.com/troglodyte/jscx) (JavaScript/TypeScript) and
[`rustcx`](https://github.com/troglodyte/rustcx) (Rust), which grew the same CLI,
LSP, and editor extensions twice and have started to drift. It is built so that
adding a language (Python and Go are next) means adding one crate, not another
CLI, LSP, and four editor extensions.

Repo: `troglodyte/cyclomatic-complexity` (`~/code/cyclomatic-complexity`).

## Why

- **Drift.** rustcx has a config file (`rustcx.toml`), hover, a VS Code
  extension, `--no-tests`, and 130 tests including CLI/LSP integration tests.
  jscx has none of those. Every future feature would be built twice.
- **More languages are coming.** Python and Go next. Two copies of everything
  is tolerable; five is not.
- **All three surfaces are in daily use** and matter equally:
  1. the Claude Code `PostToolUse` hook (`complexity.sh` in the `trog-skills`
     plugin) that calls `jscx --json` / `rustcx --json` after every edit;
  2. editor diagnostics via `--lsp` (Neovim, Zed, JetBrains, VS Code);
  3. CI gates via `--max`.

## Goals

1. One binary, `cmplx`, that scores every supported language, choosing the
   language by file extension. CLI, JSON, `--explain`, `--max` gate, and
   `--lsp` all work across languages.
2. Language aliases: `jscx` ≡ `cmplx --js`, `rustcx` ≡ `cmplx --rust`. Adding
   another (`gocx`, `pycx`) is a small, repeatable step.
3. Adding a language touches only a new crate plus its registration. The core
   (walk, CLI, output, config, LSP) does not change.
4. Each language scores like its ecosystem's standard tool, verified by tests
   built from that tool's documented examples:

   | Language | Must match | Parser |
   | --- | --- | --- |
   | JS/TS/JSX | ESLint `complexity` rule (as jscx does today) | jscx's hand-written lexer, kept as-is |
   | Rust | rustcx's current rules (ESLint-style; `?` off by default) | `syn`, kept as-is |
   | Python | `mccabe` (what flake8 / Ruff `C901` report) | tree-sitter (`tree-sitter-python`) |
   | Go | `gocyclo` (what golangci-lint reports) | tree-sitter (`tree-sitter-go`) |

5. Today's behaviour of `jscx` and `rustcx` keeps working for existing callers
   (see [Compatibility](#compatibility)).

## Non-goals

- Rewriting the JS or Rust parsers onto tree-sitter. They are tested and
  trusted; the `Language` trait hides the parser, so either can move later.
- Cognitive complexity, maintainability index, or other metrics.
- Updating `complexity.sh` in `trog-skills` — a follow-up in that repo
  (switch to `cmplx`, add `.py`/`.go`).
- Publishing to crates.io / Homebrew. Names are reserved-free as of
  2026-11-06 (`cmplx`, `cmplx-*`), but publishing is a later decision.

## Anti-goals (failure even if it "works")

- A change in any existing JS or Rust score. The current test suites (jscx 35,
  rustcx 130) must pass unchanged in meaning, with only paths/imports adjusted.
- A change in `jscx --json` or `rustcx --json` that breaks a consumer reading
  `.functions[] | .name, .line, .complexity` or `.summary`.
- A language whose scores quietly disagree with its reference tool.
- jscx losing its "no C compiler needed" build: the `jscx` binary must not
  link tree-sitter or `syn`.
- Core code containing `match language { … }` — a new language must not need
  edits to the core.

## Constraints

- Rust 2021, Cargo workspace. No `clap` (both tools hand-roll argument
  parsing; keep it, extended to accept flags declared by languages).
- Release profile as today: `lto = true`, `codegen-units = 1`,
  `panic = "abort"`, `opt-level = 3`.
- Exit codes unchanged: `0` clean, `1` something exceeded `--max`, `2` a path
  could not be read (or config error, as rustcx does today).
- The JetBrains Gradle wrapper (`gradle-wrapper.jar`) stays committed;
  `extensions/jetbrains/.intellijPlatform/`, `.kotlin/`, `.idea/`, `*.iml`,
  `out/`, `local.properties` are git-ignored.

## Decided

### Names

- Binary `cmplx`; crates `cmplx-core`, `cmplx-js`, `cmplx-rust`,
  `cmplx-python`, `cmplx-go`; config file `cmplx.toml`.
- `cx` and `cyclo` were rejected (collisions / user preference).

### History

Start from rustcx: its 30 commits become the base history of the new repo.
jscx's single commit is merged in (unrelated-histories merge or `git subtree
add` into its new crate path) so its blame is kept. The old repos are not
modified by this project.

### Workspace layout

```
Cargo.toml                 [workspace]
crates/
  cmplx-core/              model, Language trait, registry, walk, CLI parsing,
                           table/JSON/--explain output, cmplx.toml config, LSP
  cmplx-js/                jscx's lexer.rs + analyze.rs
  cmplx-rust/              rustcx's analyze.rs
  cmplx-python/            tree-sitter-python walker   (phase 2)
  cmplx-go/                tree-sitter-go walker       (phase 3)
  cmplx/                   the `cmplx` binary: fn main() { cmplx_core::run(&ALL_LANGUAGES) }
  aliases/
    jscx/                  package `jscx`:   fn main() { cmplx_core::run_as(&cmplx_js::JS) }
    rustcx/                package `rustcx`: fn main() { cmplx_core::run_as(&cmplx_rust::RUST) }
extensions/
  nvim/  zed/  jetbrains/  vscode/      one per editor, all languages
docs/
```

Start the core from rustcx's `main.rs`, `lsp.rs`, `config.rs` (the more
complete versions); port jscx-only CLI behaviour into it.

### The `Language` trait

Shape, not final signatures:

```rust
pub trait Language: Sync {
    fn id(&self) -> &'static str;              // "js" — also the --js flag and [js] config table
    fn alias(&self) -> Option<&'static str>;   // Some("jscx")
    fn extensions(&self) -> &'static [&'static str];
    fn skip(&self, path: &Path) -> bool;       // e.g. .d.ts unless --include-dts
    fn options(&self) -> &'static [OptionSpec]; // language-specific flags/config keys
    fn analyze(&self, src: &str, path_hint: &Path, opts: &LangOptions)
        -> Result<Vec<Func>, ParseError>;
}
```

- `Func`, `Point`, `ParseError` live in core and are shared. `kind` is
  supplied by the language as two strings: a JSON id and a table label. Today
  these differ for rustcx (`trait_fn` in JSON, `trait fn` in the table), and
  both are kept exactly as they are now.
- `Point.branch` is the language's own term (`elif`, `except`, `select case`,
  `?.`), not mapped to JS words.
- `OptionSpec` describes one language option once — CLI flag (`--optional-chain`,
  `--no-tests`, `--jsx`/`--no-jsx`, `--include-dts`), its `cmplx.toml` key, its
  default — and core derives parsing, `--help`, config validation, and LSP
  `initializationOptions` from it. Common options (`--no-logical`) are declared
  by each language that supports them.
- Registration is a static slice in the `cmplx` binary crate. Adding a
  language: new crate, one line in the slice, optional alias `[[bin]]`.

### CLI

```
cmplx [OPTIONS] [--js|--rust|--python|--go …] <PATH>... | -
```

- No language flag: walk all registered languages' extensions; each file goes
  to the language that claims its extension.
- One or more language flags: only those languages' files; required (exactly
  one) for stdin, replacing jscx's `--ext` — except `jscx --ext <EXT>` keeps
  working because it chooses JS vs TS/JSX lexing.
- Shared flags unchanged: `--min`, `--max`, `--top`, `--sort`, `--json`,
  `--explain`, `--no-header`, `--lsp`, `-h`, `-V`.
- A language's own flags are accepted only when that language is active
  (alias, flag, or no flag = all); `--help` lists them grouped by language.
- Directory skips as today (`node_modules`, `dist`, `build`, `coverage`,
  `.next`, `target`, VCS dirs…) plus `__pycache__`, `.venv`, `venv`, `vendor`.

- A file named explicitly on the command line that no active language claims:
  under an alias or a single language flag, it is scored as that language
  (rustcx's behaviour today). Under bare `cmplx` it is reported on stderr
  ("no language for <path>; pass --js, --rust, …") and counts as an
  unreadable path (exit 2). *(Builder-check call.)*

### Aliases

`run_as(lang)` = `run` with exactly that language active and that alias as
the program name in `--help`/`-V`. The user's words: an alias is "just a
shortcut to `cmplx --rust`". So alias output **is** `cmplx` output. There are
no per-binary output profiles, and today's two formats converge on one (see
[Output](#output)).

Each alias is its own tiny package (`crates/aliases/jscx`,
`crates/aliases/rustcx`) that depends only on `cmplx-core` and its language
crate. `cargo install --path crates/aliases/jscx` therefore compiles no C and
no `syn`, using the default command. A `[[bin]]` inside the `cmplx` crate
would not give that guarantee, because Cargo builds every dependency of a
package. Adding `gocx` means adding one small package with a three-line
`main.rs`. *(Builder-check call.)*

### Config: `cmplx.toml`

rustcx's layering, unchanged in principle: built-in defaults < `cmplx.toml`
found by walking up from the file/root < LSP `initializationOptions` < CLI
flags. Unknown keys are an error (as rustcx today). Shape:

```toml
max = 15          # CLI --max default
threshold = 11    # LSP: warn at N, error at 2N
logical = true    # default for every language that supports it

[js]
optional-chain = false
jsx = true

[rust]
question = false
tests = true
```

A per-language table may override a shared key (`[go] logical = false`).
`rustcx.toml` is still read, as a fallback when no `cmplx.toml` is found, and
only for Rust.

### Output

There is one output path for every binary. Today's two formats converge, and
that is accepted (aliases are shortcuts, not frozen copies). *(Builder-check
calls, below.)*

- **Table:** per-file blocks with the rows as they are today, using rustcx's
  one-line summary (`17 functions in 4 files · mean 2.4 · worst …`). When
  `--max` is set and exceeded, it is followed by jscx's
  `N functions above the limit of M` line. jscx's `==> Running jscx <==`
  banner is dropped. A run that mixes languages gets no extra column, because
  the file path already shows the language.
- **JSON:** a single line from serde, with `mean` as a plain number. Today's
  jscx output is pretty-printed with `mean` to 3 decimals. That is a
  formatting change only, which jq doesn't notice. One shape for all binaries,
  a superset of both current shapes:

  ```json
  {
    "summary": { "functions": 1, "files": 1, "mean": 3.0,
                 "limit": null, "overLimit": 0,
                 "worst": { "file": "…", "name": "f", "line": 1, "complexity": 3 } },
    "functions": [
      { "file": "…", "language": "js", "name": "f", "kind": "function",
        "line": 1, "depth": 0, "complexity": 3,
        "points": [ { "line": 1, "branch": "if" } ] }
    ]
  }
  ```
  Today jscx emits `limit`/`overLimit` but not `worst`; rustcx emits `worst`
  but not `limit`/`overLimit`. Adding fields is safe for jq consumers; removing
  or renaming is not.

### LSP

One server (`cmplx --lsp`) serving every registered language; alias servers
(`jscx --lsp`, `rustcx --lsp`) serve only theirs, so existing editor configs
keep working. Features are rustcx's: diagnostics on open/change (warn ≥
threshold, error ≥ 2× threshold, default threshold 11), hover with the
`--explain` breakdown, config from `cmplx.toml` + `initializationOptions`.
Parse failures: as rustcx today (no diagnostics for the file, a log message).

**`initializationOptions`** take the same shape as `cmplx.toml`: shared keys
at the top level and nested per-language tables (`{ "threshold": 15, "rust":
{ "tests": false } }`). For existing editor configs, a flat key that belongs
to exactly one active language (`{ "threshold": 6, "tests": false }` sent to
`rustcx --lsp`) is accepted as that language's key. An unknown key is logged
and ignored on its own. It no longer throws away all the options, which is
what rustcx's `unwrap_or_default` does today. *(Builder-check call.)*

The server applies `Language::skip` (for example `.d.ts` unless
`include-dts`), so extensions no longer need their own client-side `.d.ts`
filters.

### Editor extensions

One extension per editor — Neovim, Zed, JetBrains, VS Code — each launching
`cmplx --lsp` for every supported language, starting from rustcx's
extensions (VS Code exists only there). Every extension gets a new identity
named `cmplx`: the Zed id, the Neovim module and LSP config name
(`require('cmplx')`, `vim.lsp.config("cmplx")`), the JetBrains plugin id and
package (`dev.cmplx`) and the VS Code extension id. If `cmplx` isn't on PATH,
the extension does not fall back to `jscx`/`rustcx`. Instead it shows the
"binary not found" message it already has. The old extensions are not touched
and keep working against the alias servers. *(Builder-check call.)*

### Phases

1. **Combine:** workspace, core, JS, Rust, `cmplx` + `jscx` + `rustcx`
   binaries, `cmplx.toml`, merged extensions, README. Done when both old test
   suites pass in the new layout and the compatibility checks below pass.
2. **Python** (`cmplx-python`, `--python`, alias `pycx`).
3. **Go** (`cmplx-go`, `--go`, alias `gocx`).

Each phase is its own plan and its own PR.

## Compatibility

Before moving any code, capture golden output from the currently installed
`jscx` and `rustcx` over their own fixtures/test files: `--json`, `--explain`,
`--top`, exit codes, and an LSP diagnostics exchange. The goldens are compared
**by meaning, not by bytes**:
- JSON is compared parsed. The new output must contain every old field with
  the same value; the only allowed differences are the added fields.
- Tables are compared by their rows (score, line, kind, name, nesting). The
  banner and summary lines are expected to change.
- Exit codes and LSP diagnostics (range, severity, message) must be identical.

A test feeds the new `jscx --json` through the exact jq filter that
`complexity.sh` uses (`trog-skills` `hooks/complexity.sh`) and asserts the
results are identical.

## Scoring references for phases 2–3

- **Python / mccabe:** the reference is PyCQA `mccabe` (its own test suite and
  README examples). Rules to confirm against it rather than assume: how
  `elif`, loop `else`, `try`/`except`/`else`/`finally`, `with`, boolean
  operators, comprehensions, `match`/`case`, lambdas, and nested
  functions/classes are counted and attributed. Where Ruff `C901` and `mccabe`
  differ, `mccabe` wins.
- **Go / gocyclo:** the reference is `github.com/fzipp/gocyclo`: 1 + `if`,
  `for`, each non-default `case` / `comm` clause, `&&`, `||`. Confirm function
  literal attribution and method naming (`(T).M`) against it.

## Left to the builder

- Exact trait signatures, module splits inside `cmplx-core`, error types.
- How the hand-rolled argument parser absorbs language-declared options.
- tree-sitter crate versions; whether grammar crates sit behind Cargo
  features on the `cmplx` binary (default: all on).
- Fixture layout and test harness for reference-tool parity.
- README and per-extension README content.
