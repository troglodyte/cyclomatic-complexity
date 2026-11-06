# cmplx Phase 1 (Combine) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Merge jscx and rustcx into one Cargo workspace whose `cmplx` binary scores JS/TS and Rust through a language-agnostic core, with `jscx`/`rustcx` as thin alias binaries and one editor extension per editor.

**Architecture:** `cmplx-core` owns the model, the `Language` trait, config, CLI, output and LSP. It never names a concrete language. `cmplx-js` and `cmplx-rust` wrap the existing analyzers unchanged behind that trait. Binaries are separate packages: `cmplx` registers every language, and each alias registers one. Old code moves under `legacy/` first so both suites stay green while the core is built. `legacy/` is deleted last.

**Tech Stack:** Rust 2021, `syn` 3 (Rust analyzer), `serde`/`serde_json`, `toml`, `colored`. Extensions: Lua (Neovim), Rust→WASM (`zed_extension_api`), TypeScript (VS Code), Kotlin/Gradle (JetBrains).

**Spec:** `docs/superpowers/specs/2026-11-06-cmplx-design.md`. Read it before any task.

## Global Constraints

- No `clap`. Argument parsing is hand-rolled, as in both source tools.
- Release profile: `opt-level = 3`, `lto = true`, `codegen-units = 1`, `panic = "abort"`.
- Exit codes: `0` clean, `1` something exceeded `--max`, `2` a path could not be read, a file failed to parse, or a config/argument error.
- `cmplx-core` contains no reference to `js`, `rust`, `jscx`, `rustcx`, `syn` or any language crate. A test in Task 3 enforces this.
- `jscx` (package `crates/aliases/jscx`) must not depend, even indirectly, on `syn`, `proc-macro2`, `cc` or any `tree-sitter*` crate. A test in Task 8 enforces this.
- Existing JS and Rust scores must not change. The analyzer unit tests move with their analyzers, and their assertions are not edited.
- LSP default threshold `11`. Warn at `>= threshold`, error at `>= 2 × threshold`. Message `Complexity is {n} for {kind label} '{name}'`. `source` is the running program's name (`cmplx`, `jscx`, `rustcx`).
- Config file `cmplx.toml`. Unknown keys are an error in `cmplx`. `rustcx.toml` is a Rust-only fallback.
- Commit messages follow this repo's existing history (`feat: …`, `refactor(core): …`), with no JIRA reference. This is a personal repo with no ticket. Keep the `Co-Authored-By` trailer.
- `Cargo.lock` is committed. This is a binary workspace, and jscx's `.gitignore` entry for it is dropped.
- Keep the git-ignore entries from both repos: `.intellijPlatform/`, `.gradle/`, `.kotlin/`, `local.properties`, `.idea/`, `*.iml`, `out/`, `node_modules/`, `*.vsix`, `extensions/*/extension.wasm`, `extensions/*/grammars/`, `extensions/zed/target/`, `/target`, `/run/`, `/scripts-history/`. `gradle-wrapper.jar` is NOT ignored.

## Review Focus

1. **Mixed directory with one unparseable `.rs` file.** `cmplx dir/` still scores every other file, JS included, prints the parse warning on stderr and exits 2. Test: Task 9 `a_parse_failure_in_one_language_does_not_stop_the_others`.
2. **Option for a language that isn't active.** `jscx --no-tests src/` gets the error `--no-tests is not an option for js` and exit 2. It is not silently ignored. Test: Task 7 `a_flag_of_an_inactive_language_is_rejected`.
3. **A full `cmplx.toml` read by an alias.** A `[rust]` table must not make `jscx` fail. Aliases ignore tables and top-level keys that belong only to languages they don't link. `cmplx` rejects truly unknown ones. Test: Task 6 `an_alias_ignores_tables_for_languages_it_does_not_link`.
4. **`cmplx -` with no language flag.** Exits 2 with `reading stdin needs a language: pass one of --js, --rust`. It does not guess. Test: Task 7 `stdin_without_a_language_is_an_error`.
5. **LSP document no language claims** (for example a `.md` open in the editor). The server publishes empty diagnostics and keeps serving. Test: Task 10 `a_document_no_language_claims_gets_no_diagnostics`.

---

## File Structure (end of phase 1)

```
Cargo.toml                       [workspace] members = ["crates/*", "crates/aliases/*"]
crates/cmplx-core/src/
  lib.rs                         pub mod model, language, config, cli, walk, report, run, lsp
  model.rs                       Func, Point, Kind, ParseError
  language.rs                    Language trait, OptionSpec, LangOptions, Registry
  config.rs                      FileConfig (cmplx.toml, rustcx.toml fallback, initializationOptions), Watched
  cli.rs                         parse_args, help text
  walk.rs                        directory walk, file→language dispatch
  report.rs                      Row, Summary, table + JSON writers
  run.rs                         run(), run_as(), exit codes
  lsp.rs                         language server
crates/cmplx-js/src/             lib.rs (Js), lexer.rs, analyze.rs   ← from jscx
crates/cmplx-rust/src/           lib.rs (Rust), analyze.rs           ← from rustcx; fixtures/, tests/fixtures.rs
crates/cmplx/                    src/main.rs; tests/{cli,golden,hook_compat,lsp}.rs
crates/aliases/jscx/             src/main.rs
crates/aliases/rustcx/           src/main.rs
tests/golden/{jscx,rustcx}/      captured outputs from the pre-merge tools
tools/capture-goldens.sh
extensions/{nvim,zed,vscode,jetbrains}/
```

---

### Task 1: Import both histories into a temporary workspace

Run this on `main` in `~/code/cyclomatic-complexity`. The repo has no commits yet, so a worktree can't be made until this task creates one. Every later task runs in a worktree (superpowers:using-git-worktrees).

**Files:**
- Create: `Cargo.toml` (workspace), `.gitignore` (union, per Global Constraints)
- Move: rustcx root → `legacy/rustcx/`; jscx → `legacy/jscx/` (subtree)
- Add: `docs/superpowers/specs/2026-11-06-cmplx-design.md`, this plan

**Interfaces:**
- Produces: workspace members `legacy/rustcx` (package `rustcx`) and `legacy/jscx` (package `jscx`), both building and testing exactly as before.

- [ ] **Step 1: Move the uncommitted spec and plan aside, then pull rustcx's history**

```bash
cd ~/code/cyclomatic-complexity
mv docs /tmp/cmplx-docs-$$
git pull ~/code/rustcx main
git log --oneline | wc -l        # Expected: 30
```

- [ ] **Step 2: Move rustcx into `legacy/rustcx`, then add jscx as a subtree**

```bash
mkdir -p legacy/rustcx
git ls-files | cut -d/ -f1 | sort -u | while read -r top; do git mv "$top" legacy/rustcx/; done
git commit -m "refactor: move rustcx under legacy/ ahead of the workspace merge"
git subtree add --prefix=legacy/jscx ~/code/jscx main
```
Check `git ls-files | head` shows only `legacy/…`.

- [ ] **Step 3: Write the root `Cargo.toml` and `.gitignore`**

The `[workspace]` has `resolver = "2"`, `members = ["legacy/rustcx", "legacy/jscx"]` and the release profile from Global Constraints. Remove `[profile.release]` from both legacy manifests, because Cargo ignores profiles outside the workspace root and warns about them. Write the root `.gitignore` from the Global Constraints list and delete the two legacy `.gitignore` files. Restore the docs with `mv /tmp/cmplx-docs-$$ docs`.

- [ ] **Step 4: Verify both suites, unchanged**

Run: `cargo test --workspace 2>&1 | rg 'test result'`
Expected: every line `ok`. The total passed is jscx 35 plus rustcx's total (130, from `cargo test` in `~/code/rustcx` today).

- [ ] **Step 5: Commit, then push only after the user agrees**

```bash
git add Cargo.toml Cargo.lock .gitignore legacy docs
git commit -m "build: combine jscx and rustcx in one cargo workspace"
```

---

### Task 2: Capture golden outputs from the pre-merge tools

**Files:**
- Create: `tools/capture-goldens.sh`, `tests/golden/{jscx,rustcx}/*`

**Interfaces:**
- Produces: for each case `<name>`: `<name>.stdout`, `<name>.stderr`, `<name>.exit`. Task 9 reads them.

- [ ] **Step 1: Write `tools/capture-goldens.sh`**

The script runs `cargo build --release -p jscx -p rustcx` and uses `target/release/{jscx,rustcx}`, not `~/.cargo/bin`, which may be stale. It runs each case and writes the three files per case.

| Tool | Case | Command |
|---|---|---|
| rustcx | `json-<f>` | `rustcx --json legacy/rustcx/fixtures/<f>.rs` for each fixture |
| rustcx | `explain-<f>` | `rustcx --explain legacy/rustcx/fixtures/<f>.rs` |
| rustcx | `top` | `rustcx --top 5 legacy/rustcx/fixtures` |
| rustcx | `max` | `rustcx --max 3 legacy/rustcx/fixtures` |
| rustcx | `missing` | `rustcx does-not-exist.rs` |
| jscx | `json-<f>` | `jscx --json legacy/jscx/<f>` for `test.js`, `test.ts` |
| jscx | `explain-<f>` | `jscx --explain legacy/jscx/<f>` |
| jscx | `top` | `jscx --top 5 legacy/jscx` |
| jscx | `max` | `jscx --max 3 legacy/jscx` |
| jscx | `stdin-ts` | `jscx --json --ext ts - < legacy/jscx/test.ts` |
| jscx | `missing` | `jscx does-not-exist.js` |

Paths in the outputs must be repo-relative. Run the script from the repo root.

- [ ] **Step 2: Run it and inspect**

Run: `tools/capture-goldens.sh && ls tests/golden/*/ | wc -l`
Expected: a non-zero count. `cat tests/golden/rustcx/max.exit` prints `1`, and `cat tests/golden/jscx/missing.exit` prints `2`.

- [ ] **Step 3: Commit**

```bash
git add tools/capture-goldens.sh tests/golden
git commit -m "test: capture golden output from jscx and rustcx before the merge"
```

---

### Task 3: `cmplx-core` model and `Language` trait

**Files:**
- Create: `crates/cmplx-core/{Cargo.toml,src/lib.rs,src/model.rs,src/language.rs}`, `crates/cmplx-core/tests/independence.rs`
- Modify: root `Cargo.toml` members → `["crates/*", "crates/aliases/*", "legacy/*"]`

**Interfaces:**
- Produces (`cmplx_core::model`):
  ```rust
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub struct Kind { pub id: &'static str, pub label: &'static str }   // id → JSON, label → table/LSP
  #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
  pub struct Point { pub line: u32, pub branch: &'static str }
  #[derive(Clone, Debug, PartialEq, Eq)]
  pub struct Func { pub name: String, pub kind: Kind, pub line: u32, pub complexity: u32, pub depth: u32, pub points: Vec<Point> }
  #[derive(Clone, Debug, PartialEq, Eq)]
  pub struct ParseError { pub line: u32, pub message: String }
  ```
- Produces (`cmplx_core::language`):
  ```rust
  pub struct OptionSpec {
      pub key: &'static str,              // cmplx.toml key and initializationOptions key, e.g. "optional-chain"
      pub on_flag: Option<&'static str>,  // e.g. Some("--optional-chain")
      pub off_flag: Option<&'static str>, // e.g. Some("--no-logical")
      pub default: Option<bool>,          // None = the language decides per file (jsx)
      pub help: &'static str,
  }
  #[derive(Clone, Debug, Default, PartialEq, Eq)]
  pub struct LangOptions(BTreeMap<&'static str, bool>);
  impl LangOptions {
      pub fn get(&self, key: &str) -> Option<bool>;          // explicitly set value only
      pub fn flag(&self, spec: &OptionSpec) -> Option<bool>; // set value, else spec.default
      pub fn set(&mut self, key: &'static str, v: bool);
      pub fn or(self, lower: LangOptions) -> LangOptions;    // self wins
  }
  pub trait Language: Sync {
      fn id(&self) -> &'static str;                 // "js": also the --js flag and the [js] table
      fn display_name(&self) -> &'static str;       // "JavaScript/TypeScript", for --help
      fn alias(&self) -> Option<&'static str>;
      fn extensions(&self) -> &'static [&'static str];
      fn stdin_ext(&self) -> &'static str;          // used when `-` is read and --ext is absent
      fn options(&self) -> &'static [OptionSpec];
      fn skip(&self, path: &Path, opts: &LangOptions) -> bool;
      fn analyze(&self, src: &str, path: &Path, opts: &LangOptions) -> Result<Vec<Func>, ParseError>;
  }
  pub type Lang = &'static dyn Language;
  pub struct Registry { langs: Vec<Lang>, complete: bool }
  impl Registry {
      pub fn all(langs: &[Lang]) -> Registry;     // complete = true  (the cmplx binary)
      pub fn alias(lang: Lang) -> Registry;       // complete = false (alias binaries)
      pub fn langs(&self) -> &[Lang];
      pub fn by_id(&self, id: &str) -> Option<Lang>;
      pub fn for_path(&self, path: &Path, active: &[Lang]) -> Option<Lang>; // by extension, within active
      pub fn is_complete(&self) -> bool;
  }
  ```

- [ ] **Step 1: Write failing tests in `language.rs`, using a private `Fake` language defined in the test module**

  - `flag_falls_back_to_the_spec_default`: `LangOptions::default().flag(&spec_with_default(Some(true))) == Some(true)`
  - `or_prefers_the_upper_layer`: upper `{logical:false}` `.or(` lower `{logical:true, tests:false})` gives `get("logical") == Some(false)` and `get("tests") == Some(false)`
  - `for_path_picks_by_extension_within_active`: a Fake with `["fk"]` matches `a.fk`, and returns `None` for `a.fk` when the Fake isn't in `active`
  - `for_path_is_case_sensitive_on_extension`: `a.FK` → `None` (matches both tools today)

- [ ] **Step 2: Write `tests/independence.rs`**

The test reads every `src/**/*.rs` and `Cargo.toml` in the crate and asserts none contains `cmplx-js`, `cmplx_js`, `cmplx-rust`, `cmplx_rust`, `syn`, `"jscx"` or `"rustcx"`. The `jscx`/`rustcx` strings are allowed only in `config.rs`, for the `rustcx.toml` fallback, and the test whitelists that one file and string.

- [ ] **Step 3: Run the tests and see them fail**

Run: `cargo test -p cmplx-core`. Expected: compile errors / FAIL.

- [ ] **Step 4: Implement `model.rs` and `language.rs`**

Dependencies: `serde` (derive), `serde_json`, `toml`, `colored`.

- [ ] **Step 5: Run the tests and see them pass**

Run: `cargo test -p cmplx-core`. Expected: PASS.

- [ ] **Step 6: Commit**

`feat(core): add the language-agnostic model and Language trait`

---

### Task 4: `cmplx-rust`, rustcx's analyzer behind `Language`

**Files:**
- Move (`git mv`): `legacy/rustcx/src/analyze.rs` → `crates/cmplx-rust/src/analyze.rs`; `legacy/rustcx/fixtures/` → `crates/cmplx-rust/fixtures/`; `legacy/rustcx/tests/fixtures.rs` → `crates/cmplx-rust/tests/fixtures.rs`
- Create: `crates/cmplx-rust/{Cargo.toml,src/lib.rs}`
- Modify: `legacy/rustcx/src/lib.rs` → `pub use cmplx_rust::analyze;` (plus its existing `config`, `lsp`), and `legacy/rustcx/Cargo.toml` gains `cmplx-rust = { path = "../../crates/cmplx-rust" }`

**Interfaces:**
- Consumes: Task 3 `Language`, `Func`, `Kind`, `OptionSpec`, `LangOptions`
- Produces: `pub struct Rust; pub static RUST: Rust;` and `pub mod analyze` (unchanged API)
  - `id "rust"`, `display_name "Rust"`, `alias Some("rustcx")`, `extensions ["rs"]`, `stdin_ext "rs"`
  - options: `logical` (off `--no-logical`, default `Some(true)`), `question` (on `--question`, default `Some(false)`), `tests` (off `--no-tests`, default `Some(true)`)
  - kinds: `Fn {id:"fn",label:"fn"}`, `Method {"method","method"}`, `TraitFn {"trait_fn","trait fn"}`, `Closure {"closure","closure"}`
  - `skip` → always `false`

- [ ] **Step 1: Write failing tests in `crates/cmplx-rust/src/lib.rs`**

  - `trait_fn_keeps_its_two_spellings`: analyzing `trait T { fn f(&self) { if true {} } }` gives `kind == Kind{id:"trait_fn",label:"trait fn"}`, complexity 2
  - `question_option_is_honoured`: `fn f() -> Option<()> { None? ; Some(()) }` scores 1 by default and 2 with `question=true`
  - `a_parse_error_carries_its_line`: `"fn f( {"` → `Err(ParseError{line:1, ..})`

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p cmplx-rust`.

- [ ] **Step 3: Implement `impl Language for Rust`, mapping `analyze::Func` → `cmplx_core::Func`**

- [ ] **Step 4: Verify the new crate and legacy rustcx**

Run: `cargo test -p cmplx-rust -p rustcx 2>&1 | rg 'test result'`. Expected: all `ok`. The moved `analyze.rs` unit tests and `tests/fixtures.rs` pass without edits to their assertions.

- [ ] **Step 5: Commit**

`refactor(rust): move rustcx's analyzer into cmplx-rust behind Language`

---

### Task 5: `cmplx-js`, jscx's lexer and analyzer behind `Language`

**Files:**
- Move: `legacy/jscx/src/{lexer.rs,analyze.rs}` → `crates/cmplx-js/src/`
- Create: `crates/cmplx-js/{Cargo.toml,src/lib.rs}`, `crates/cmplx-js/fixtures/{test.js,test.ts}` (copied from `legacy/jscx/`)
- Modify: `legacy/jscx/src/lib.rs` re-exports `cmplx_js::{lexer, analyze}`, and its `Cargo.toml` gains `cmplx-js`

**Interfaces:**
- Produces: `pub struct Js; pub static JS: Js;`
  - `id "js"`, `display_name "JavaScript/TypeScript"`, `alias Some("jscx")`, `extensions ["js","mjs","cjs","jsx","ts","tsx","mts","cts"]`, `stdin_ext "tsx"`
  - options: `logical` (off `--no-logical`, `Some(true)`), `optional-chain` (on `--optional-chain`, `Some(false)`), `jsx` (on `--jsx`, off `--no-jsx`, `None`), `include-dts` (on `--include-dts`, `Some(false)`)
  - jsx when unset: `false` for `ts`/`mts`/`cts`, `true` otherwise (today's `jsx_default`)
  - `skip(path)`: the name ends with `.d.ts`/`.d.mts`/`.d.cts` and `include-dts` isn't set. Copy jscx's `is_source` rule exactly.
  - kind id = label = jscx's `FnKind::as_str()` (`function`, `arrow`, `method`, `constructor`, `getter`, `setter`)
  - `analyze` never returns `Err`
- Must depend on `cmplx-core` only. No `syn`, no `cc`.

- [ ] **Step 1: Write failing tests**

  - `ts_files_lex_without_jsx`: `"const f = <T,>(x: T) => x < 1 ? x : x;"` as `a.ts` gives one arrow, complexity 2
  - `jsx_flag_overrides_the_extension`: the same source as `a.ts` with `jsx=true` gives a different result from the test above. Assert whatever today's `jscx --jsx --ext ts -` prints, and record that value in the test.
  - `dts_is_skipped_unless_included`: `skip("x.d.ts", {})` is `true`; with `include-dts=true` it is `false`
  - `optional_chain_counts_only_when_asked`: `function f(a){ return a?.b }` scores 1, or 2 with `optional-chain=true`

- [ ] **Step 2: Fail → Step 3: implement → Step 4: pass**

Run: `cargo test -p cmplx-js -p jscx 2>&1 | rg 'test result'`. Expected: all `ok`, including jscx's 35 moved unit tests unedited.

- [ ] **Step 5: Commit**

`refactor(js): move jscx's lexer and analyzer into cmplx-js behind Language`

---

### Task 6: Core config: `cmplx.toml`, `rustcx.toml` fallback, `initializationOptions`

**Files:**
- Create: `crates/cmplx-core/src/config.rs`, ported from `legacy/rustcx/src/config.rs` and generalised. Keep `Watched` with the same semantics.

**Interfaces:**
- Consumes: `Registry`, `Lang`, `LangOptions`, `OptionSpec`
- Produces:
  ```rust
  pub const FILE_NAME: &str = "cmplx.toml";
  pub const LEGACY_RUST_FILE: (&str, &str) = ("rustcx.toml", "rust"); // file name, language id
  #[derive(Clone, Debug, Default, PartialEq, Eq)]
  pub struct FileConfig { pub max: Option<u32>, pub threshold: Option<u32>, langs: BTreeMap<&'static str, LangOptions> }
  pub struct ConfigError { pub path: PathBuf, pub message: String }
  impl FileConfig {
      pub fn parse(path: &Path, text: &str, reg: &Registry) -> Result<FileConfig, ConfigError>;
      pub fn parse_legacy_rust(path: &Path, text: &str, reg: &Registry) -> Result<FileConfig, ConfigError>;
      pub fn discover(start: &Path, reg: &Registry) -> Result<Option<(PathBuf, FileConfig)>, ConfigError>;
      pub fn from_init_options(v: &serde_json::Value, reg: &Registry, active: &[Lang]) -> (FileConfig, Vec<String>); // config, warnings
      pub fn or(self, lower: FileConfig) -> FileConfig;
      pub fn options_for(&self, lang: Lang) -> LangOptions;
  }
  pub struct Watched { … }  // discover(start, &Registry), refresh(), config(): same contract as rustcx's
  ```
- Rules:
  - A top-level bool key applies to every language that declares it.
  - A `[<id>]` table sets that language's keys and wins over the top level.
  - Unknown key or table: `ConfigError` when `reg.is_complete()`. Otherwise (an alias) it is ignored when it belongs to no linked language.
  - A table key the linked language doesn't declare is always an error.
  - `discover` walks up from `start`. At each directory it prefers `cmplx.toml` and falls back to `rustcx.toml` only when `rust` is registered.

- [ ] **Step 1: Port rustcx's `config.rs` tests and adapt them to the new signatures, using a fake two-language registry. Add these tests:**

  - `top_level_logical_reaches_every_language_that_declares_it`
  - `a_language_table_overrides_the_top_level`: `logical=false` at the top with `[js] logical=true` gives js `true` and the other language `false`
  - `cmplx_rejects_an_unknown_table`: `[cobol]` → `Err`, and the message names `cobol` and the file
  - `an_alias_ignores_tables_for_languages_it_does_not_link`: `Registry::alias(fake_a)` parsing `[fake_b] x=true` → `Ok` (Review Focus 3)
  - `a_misspelled_key_in_a_linked_table_is_an_error_even_in_an_alias`
  - `rustcx_toml_is_read_as_the_rust_table_when_no_cmplx_toml_exists`, and `cmplx_toml_wins_over_rustcx_toml_in_the_same_directory`
  - `flat_init_options_key_belongs_to_the_one_active_language_declaring_it`: `{threshold:6, tests:false}` with active = [rust-like fake] gives threshold 6 and that language's `tests=false`
  - `an_unknown_init_option_is_dropped_alone`: `{threshold:6, bogus:1}` gives threshold 6 and one warning naming `bogus`
  - `nested_init_options_match_the_toml_shape`: `{rust:{tests:false}}`

- [ ] **Step 2: Fail → implement → pass**

Run: `cargo test -p cmplx-core config`.

- [ ] **Step 3: Commit**

`feat(core): layered cmplx.toml config with per-language tables`

---

### Task 7: Core CLI parsing and help

**Files:**
- Create: `crates/cmplx-core/src/cli.rs`, ported from `legacy/rustcx/src/main.rs` (`FLAGS`, `parse_args`, `take_number`)

**Interfaces:**
- Consumes: `Registry`, `FileConfig`, `LangOptions`
- Produces:
  ```rust
  pub struct Cli {
      pub program: &'static str,          // "cmplx" | alias name
      pub paths: Vec<PathBuf>, pub stdin: bool, pub ext: Option<String>,
      pub min: u32, pub max: Option<u32>, pub top: Option<usize>,
      pub sort: bool, pub json: bool, pub explain: bool, pub no_header: bool, pub lsp: bool,
      pub active: Vec<Lang>,              // explicit language flags, else every registered language
      pub explicit_lang: bool,            // true when an alias or language flag chose the languages
      pub flags: FileConfig,              // CLI layer; options set by language flags
  }
  pub fn parse_args(program: &'static str, args: &[String], reg: &Registry) -> Result<Option<Cli>, String>; // Ok(None) = help/version printed
  pub fn help(program: &'static str, reg: &Registry) -> String;
  ```
- Shared flags as listed in the spec, plus `--ext <EXT>`. Language flags are `--<id>`, offered only by `cmplx` (an alias has its language fixed). Language option flags come from each active language's `OptionSpec`s, and one flag applies to every active language that declares it. Options are checked against the active languages after all arguments are read, so order doesn't matter.
- `-V` prints `<program> <crate version>`.

- [ ] **Step 1: Port rustcx's `parse_args` unit tests, then add:**

  - `language_flags_restrict_the_active_set`: `cmplx --js src` → active = [js]
  - `no_language_flag_means_every_language`
  - `a_flag_of_an_inactive_language_is_rejected`: `jscx --no-tests src` → `Err("--no-tests is not an option for js")`, and `cmplx --js --no-tests src` → the same (Review Focus 2)
  - `a_shared_option_flag_reaches_every_active_language`: `cmplx --no-logical` sets `logical=false` for both
  - `stdin_without_a_language_is_an_error`: `cmplx -` → `Err("reading stdin needs a language: pass one of --js, --rust")` (Review Focus 4)
  - `aliases_do_not_offer_language_flags`: `jscx --rust x` → `Err` unknown flag
  - `help_lists_each_languages_options_under_its_name`: `help("cmplx", reg)` contains `JavaScript/TypeScript` followed by `--optional-chain`, and `help("jscx", …)` does not contain `--no-tests`

- [ ] **Step 2: Fail → implement → pass**

Run: `cargo test -p cmplx-core cli`.

- [ ] **Step 3: Commit**

`feat(core): CLI parsing with language-declared flags`

---

### Task 8: Core walk, report and run; the binaries

**Files:**
- Create: `crates/cmplx-core/src/{walk.rs,report.rs,run.rs}`, ported from `legacy/rustcx/src/main.rs` (`collect`, `select`, `summarize`, `write_table`, `write_grouped`, `write_flat`, `write_points`, `write_summary`, `write_json`, `run`, `exit_code`, `resolve`)
- Create: `crates/cmplx/{Cargo.toml,src/main.rs}`, `crates/aliases/jscx/{Cargo.toml,src/main.rs}`, `crates/aliases/rustcx/{Cargo.toml,src/main.rs}`
- Create: `crates/aliases/jscx/tests/no_c_deps.rs`

**Interfaces:**
- Produces:
  ```rust
  pub fn run(program: &'static str, reg: Registry) -> ExitCode;   // reads std::env::args, dispatches --lsp
  pub fn run_as(lang: Lang) -> ExitCode;                          // run(lang.alias().unwrap(), Registry::alias(lang))
  ```
  `crates/cmplx/src/main.rs`: `fn main() -> ExitCode { cmplx_core::run::run("cmplx", Registry::all(&[&cmplx_js::JS, &cmplx_rust::RUST])) }`. Each alias: `run_as(&cmplx_js::JS)` / `run_as(&cmplx_rust::RUST)`. Package names `cmplx`, `jscx`, `rustcx`. Rename the legacy packages to `jscx-legacy`/`rustcx-legacy` (with their `[[bin]]` names left as they are) so the names don't clash.
- Skipped directories: the union of both tools' `SKIP_DIRS` plus `__pycache__`, `.venv`, `venv`, `vendor`.
- Explicitly named file: under `explicit_lang` with one active language, it is scored as that language. Otherwise use `for_path`; if nothing claims the file, print `cmplx: no language for <path>; pass --js, --rust` on stderr and count it as unreadable (exit 2).
- Table: rustcx's layout, including its one-line summary. When `max` is set and exceeded, also print `{n} function{s} above the limit of {max}`. No banner.
- JSON: the serde shape in the spec's Output section, with `language` taken from `Lang::id()`, `kind` from `Kind::id`, `limit` = `max`, `overLimit` = the number of functions with `complexity > max` (0 when `max` is unset), and `worst` `null` when there are no functions.

- [ ] **Step 1: Port rustcx's `main.rs` unit tests into `report.rs`/`run.rs`, then add:**

  - `json_has_both_tools_summary_fields`: the summary has `functions, files, mean, limit, overLimit, worst`
  - `json_functions_carry_their_language`
  - `over_limit_line_appears_only_when_max_is_exceeded`

- [ ] **Step 2: Write `no_c_deps.rs`**

The test runs `cargo tree -p jscx -e normal --prefix none` through `std::process::Command` and asserts no line starts with `syn `, `proc-macro2 `, `cc ` or `tree-sitter`.

- [ ] **Step 3: Fail → implement → pass**

Run: `cargo test -p cmplx-core -p cmplx -p jscx -p rustcx`.

- [ ] **Step 4: Commit**

`feat: cmplx binary with jscx and rustcx aliases on the shared core`

---

### Task 9: CLI behaviour, goldens and hook compatibility

**Files:**
- Create: `crates/cmplx/tests/{cli.rs,golden.rs,hook_compat.rs}`, and `crates/cmplx/tests/fixtures/complexity.jq`, a verbatim copy of the `jq -rs` program in `~/code/skills/hooks/complexity.sh` (commit `7369624`), with that source noted in a comment
- Move: `legacy/rustcx/tests/cli.rs` → `crates/cmplx/tests/cli.rs`.
- Create: `crates/cmplx/tests/common/mod.rs` with `pub fn bin(name: &str) -> PathBuf`. On first call (`OnceLock`) it runs `cargo build -p cmplx -p jscx -p rustcx` and returns `target/debug/<name>`. This is needed because `env!("CARGO_BIN_EXE_…")` only covers binaries in the same package, and the aliases are separate packages.

**Interfaces:**
- Consumes: Task 2 goldens; the binaries from Task 8.

- [ ] **Step 1: Write `golden.rs`, one test per golden case, using these helpers**

  - `json_superset(old: &Value, new: &Value) -> Result<(), String>`: every object key in `old` exists in `new` with an equal value, arrays have the same length and match element by element, and numbers compare within `1e-3`
  - `table_rows(stdout: &str) -> Vec<(u32 /*cc*/, u32 /*line*/, String /*kind*/, String /*name*/, usize /*indent*/)>`: parse only row lines, ignoring the header, banner and summary
  - Exit codes are compared exactly. For the stderr of `missing`, assert it contains the path.

- [ ] **Step 2: Write `hook_compat.rs`**

For `jscx --json` and `rustcx --json` on a golden input, feed `[new_output, {"functions":[]}]` and `[golden_output, {"functions":[]}]` through `jq -rs --argjson max 1 -f complexity.jq`, and assert the two results are identical. If `jq` isn't on PATH, the test fails with `install jq to run hook_compat`. It is never skipped silently.

- [ ] **Step 3: Add mixed-language CLI tests to `cli.rs`**

  - `cmplx_scores_js_and_rust_in_one_walk`: a temporary directory with `a.ts` and `b.rs` gives rows for both, and the summary says `2 files`
  - `a_parse_failure_in_one_language_does_not_stop_the_others`: adding `bad.rs` (`fn f( {`) gives rows for `a.ts`, a warning on stderr naming `bad.rs`, and exit 2 (Review Focus 1)
  - `an_unclaimed_explicit_file_exits_two`: `cmplx README.md` → exit 2, and stderr contains `no language for README.md`
  - `rustcx_scores_an_explicit_file_regardless_of_extension`: `rustcx snippet.txt` containing Rust → a row, exit 0

- [ ] **Step 4: Run, fix, pass**

Run: `cargo test -p cmplx`. Expected: PASS. A golden mismatch is investigated against the spec's Compatibility section; the golden is not regenerated.

- [ ] **Step 5: Commit**

`test: golden, hook and mixed-language checks for cmplx and its aliases`

---

### Task 10: Core LSP

**Files:**
- Create: `crates/cmplx-core/src/lsp.rs`, ported from `legacy/rustcx/src/lsp.rs` with framing, `Server`, `hover_at`, `diagnostics_for` and `Watched` reload
- Move: `legacy/rustcx/tests/lsp.rs` → `crates/cmplx/tests/lsp.rs`, parameterised over the `cmplx` and `rustcx` binaries

**Interfaces:**
- Produces: `pub fn serve(program: &'static str, reg: &Registry) -> io::Result<()>`, called by `run` on `--lsp`
- Per document: language = `reg.for_path(uri_path, reg.langs())`. A document no language claims gets empty diagnostics. `skip` is applied. Options come from `Watched` config `.or(init options)`. `source` = `program`.

- [ ] **Step 1: Port the tests, then add:**

  - `js_diagnostics_match_jscx`: open `file:///x/a.ts` with a function scoring 12 at the default threshold. Expect one diagnostic with severity 2, message `Complexity is 12 for function 'f'`, range on that whole line, and `source` `"cmplx"` (and `"jscx"` from the jscx binary)
  - `a_document_no_language_claims_gets_no_diagnostics`: open `file:///x/README.md` → `publishDiagnostics` with `[]`, then a following request still gets answered (Review Focus 5)
  - `dts_documents_are_skipped_on_the_server`
  - `flat_legacy_init_options_still_apply_to_rustcx`: `initializationOptions {threshold:6, tests:false}` sent to `rustcx --lsp` applies threshold 6

- [ ] **Step 2: Fail → implement → pass**

Run: `cargo test -p cmplx --test lsp && cargo test -p cmplx-core lsp`.

- [ ] **Step 3: Commit**

`feat(core): one language server for every registered language`

---

### Task 11: Editor extensions as `cmplx`

**Files:**
- Move: `legacy/rustcx/extensions/{nvim,zed,vscode,jetbrains}` → `extensions/` (`git mv`)
- Modify: the identities in every file currently naming `rustcx`, as listed by `rg -l rustcx extensions/`

**Interfaces:**
- Each extension launches `cmplx --lsp` for these languages:
  - Neovim filetypes `javascript, javascriptreact, typescript, typescriptreact, rust`
  - Zed languages `JavaScript, TypeScript, TSX, Rust`
  - VS Code `documentSelector` languages `javascript, javascriptreact, typescript, typescriptreact, rust`
  - JetBrains: file extensions from `cmplx`'s JS and Rust lists
- Identities:
  - Neovim: module `cmplx`, `vim.lsp.config("cmplx")`
  - Zed: `id = "cmplx"`
  - VS Code: `name: "cmplx"`
  - JetBrains: plugin id and package `dev.cmplx`, classes `Cmplx*`
- Settings keys renamed `rustcx.*` → `cmplx.*`.
- Drop jscx's client-side `.d.ts` filters; the server does this now (Task 10).
- No fallback to `jscx`/`rustcx` binaries.

One step group per editor, committed separately:

- [ ] **Neovim:** rename, then run the test command documented in `extensions/nvim/README.md` against `test/cmplx_spec.lua` (renamed from `rustcx_spec.lua`) → PASS. Commit `feat(nvim): cmplx client for JS, TS and Rust`.
- [ ] **Zed:** rename, then run the build command documented in `extensions/zed/README.md` → OK. Commit `feat(zed): cmplx extension for JS, TS and Rust`.
- [ ] **VS Code:** rename, then `cd extensions/vscode && npm ci && npm test` → PASS. Commit `feat(vscode): cmplx extension for JS, TS and Rust`.
- [ ] **JetBrains:** rename, then `cd extensions/jetbrains && ./gradlew test` → PASS. `.intellijPlatform/` stays ignored. Commit `feat(jetbrains): cmplx plugin for JS, TS and Rust`.

---

### Task 12: Remove `legacy/`, README, final verification

**Files:**
- Delete: `legacy/` (both packages; history keeps them)
- Modify: root `Cargo.toml` members → `["crates/*", "crates/aliases/*"]`; `tools/capture-goldens.sh` gets a header noting it ran against the pre-merge tools and is kept for reference
- Create: `README.md`. It merges both READMEs' usage, scoring rules (one section per language, saying which reference tool each matches), `cmplx.toml`, aliases, the editor table and exit codes. The first line is "Cyclomatic complexity CLI and language server for JavaScript, TypeScript and Rust."
- Move: `legacy/jscx/tools/colors.sh` → `tools/colors.sh`, only if anything still references it (`rg colors.sh`); otherwise drop it

- [ ] **Step 1: Delete `legacy/` and update the members**

- [ ] **Step 2: Full verification**

```bash
cargo test --workspace 2>&1 | rg 'test result'     # all ok
cargo clippy --workspace --all-targets -- -D warnings
rg -n 'jscx|rustcx|cmplx_js|cmplx_rust|syn' crates/cmplx-core/src   # only the rustcx.toml fallback in config.rs
cargo install --path crates/cmplx && cargo install --path crates/aliases/jscx && cargo install --path crates/aliases/rustcx
which -a cmplx jscx rustcx                          # all in ~/.cargo/bin
```

Then pipe a real `complexity.sh` run against an edited `.ts` file. It must still report as it did before.

- [ ] **Step 3: Commit**

`chore: drop legacy crates and document cmplx`
