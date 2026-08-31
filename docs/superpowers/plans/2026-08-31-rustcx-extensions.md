# rustcx Editor Extensions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship four thin LSP clients — Neovim, Zed, JetBrains, VS Code — so `rustcx --lsp` attaches in an editor without hand-written configuration.

**Architecture:** Each client resolves the `rustcx` binary, runs `rustcx --lsp`, and sends a threshold only if the user explicitly set one. All scoring, defaults, and `rustcx.toml` handling stay in `src/lsp.rs`, which is already complete and is not modified by this plan. The four targets are independent and may be built in any order.

**Tech Stack:** Lua (Neovim 0.11), Rust/WASM (`zed_extension_api`), Kotlin/Gradle (IntelliJ Platform), TypeScript/Node (`vscode-languageclient`).

**Spec:** `docs/superpowers/specs/2026-08-31-rustcx-extensions-design.md`

## Global Constraints

Every task's requirements implicitly include this section.

- **Binary resolution order, identical in all four clients:** configured path (if set and executable — if set and *not* executable, fail with an error naming the setting, never fall through) → `PATH` → `~/.cargo/bin/rustcx` (`rustcx.exe` on Windows) → no client at all, with one message naming `cargo install --path .`.
- **`DEFAULT_THRESHOLD = 11` is defined once, in `src/lsp.rs`.** No client file may contain the literal `11` as a threshold default, and no extension README may state it as that extension's own default.
- **An unset threshold sends no `initializationOptions` key at all.** Per-platform representation of "unset": Neovim — the `init_options` key is absent; Zed — `Ok(None)`; JetBrains — `0` in persisted state, mapped to `null`; VS Code — `null` in `package.json`. Sending `11` would override `rustcx.toml`, because `Server::settings` is `self.init.or(self.config.config())`.
- **Attach to `.rs` files only,** via each editor's native filetype mechanism. No client-side file-content or filename filter module.
- **No client may implement any LSP feature the server does not advertise.** No code actions, no commands, no status bar items.
- **`src/lsp.rs`, `src/analyze.rs`, `src/config.rs`, and `src/main.rs` are not modified by any task in this plan.**
- Neovim 0.11+. `zed_extension_api` 0.7.0 on `wasm32-unknown-unknown`. IntelliJ Platform 2026.2, `pluginSinceBuild = 262`, Gradle 9.5.0, Kotlin 2.4.10, IntelliJ Platform Gradle Plugin 2.18.1, `jvmToolchain(21)`, Kotlin package `dev.rustcx.intellij`. VS Code engine `^1.90`, `vscode-languageclient` `^9.0.1`.
- The manual check for every target is `fixtures/patterns.rs`, whose `describe` scores 8: it warns at threshold 8 and errors at threshold 4.

---

## File Structure

| File | Responsibility |
| --- | --- |
| `extensions/nvim/lua/rustcx/binary.lua` | Binary resolution, dependency-injected for tests |
| `extensions/nvim/lsp/rustcx.lua` | The `vim.lsp.config` table Neovim discovers off the runtimepath |
| `extensions/nvim/test/rustcx_spec.lua` | Headless suite for both of the above |
| `extensions/zed/src/lib.rs` | Zed extension: command + initialization options |
| `extensions/jetbrains/src/main/kotlin/dev/rustcx/intellij/RustcxBinary.kt` | Resolution, injected `pathLookup`/`home` |
| `…/RustcxSettings.kt` | Persisted threshold + binary path |
| `…/RustcxConfigurable.kt` | The Settings \| Tools \| rustcx panel |
| `…/RustcxNotifications.kt` | Once-per-project balloon when the binary is missing |
| `…/RustcxLspClientDescriptor.kt` | Command line + initialization options |
| `…/RustcxLspIntegrationProvider.kt` | Which files start the server |
| `extensions/vscode/src/resolve.ts` | Resolution, pure and unit-testable under `node --test` |
| `extensions/vscode/src/extension.ts` | Client lifecycle: activate, restart on config change, deactivate |

Resolution logic is reimplemented four times because it lives in four languages. That is unavoidable; the Global Constraints section is what keeps the four implementations agreeing.

---

## Task 1: Neovim plugin

**Files:**
- Create: `extensions/nvim/lua/rustcx/binary.lua`
- Create: `extensions/nvim/lsp/rustcx.lua`
- Create: `extensions/nvim/test/rustcx_spec.lua`
- Create: `extensions/nvim/README.md`

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces: `require("rustcx.binary").resolve(overrides?) -> string|nil` and `require("rustcx.binary").EXECUTABLE -> "rustcx"`. `lsp/rustcx.lua` returns a table with keys `cmd`, `filetypes`, `root_dir`, and deliberately **no** `init_options`.

- [ ] **Step 1: Write the failing test**

Create `extensions/nvim/test/rustcx_spec.lua`:

```lua
-- Headless test suite. Run with:
--   nvim -l extensions/nvim/test/rustcx_spec.lua
-- Exits non-zero if any assertion fails.

local script = debug.getinfo(1, "S").source:sub(2)
local root = vim.fs.dirname(vim.fs.dirname(script))
vim.opt.runtimepath:prepend(root)

local binary = require("rustcx.binary")

local passed, failed = 0, 0

local function check(name, ok)
  if ok then
    passed = passed + 1
    io.write("ok    ", name, "\n")
  else
    failed = failed + 1
    io.write("FAIL  ", name, "\n")
  end
end

-- rustcx.binary.resolve

--- Dependency set with nothing found anywhere, for tests to override piecemeal.
local function deps(overrides)
  return vim.tbl_extend("force", {
    exepath = function()
      return ""
    end,
    executable = function()
      return false
    end,
    home = "/home/tester",
    windows = false,
  }, overrides or {})
end

check(
  "resolve returns the PATH hit",
  binary.resolve(deps({
    exepath = function()
      return "/usr/local/bin/rustcx"
    end,
    executable = function(path)
      return path == "/usr/local/bin/rustcx"
    end,
  })) == "/usr/local/bin/rustcx"
)

check(
  "resolve falls back to the cargo bin when PATH has no hit",
  binary.resolve(deps({
    executable = function(path)
      return path == "/home/tester/.cargo/bin/rustcx"
    end,
  })) == "/home/tester/.cargo/bin/rustcx"
)

check(
  "resolve falls back when the PATH hit is not executable",
  binary.resolve(deps({
    exepath = function()
      return "/usr/local/bin/rustcx"
    end,
    executable = function(path)
      return path == "/home/tester/.cargo/bin/rustcx"
    end,
  })) == "/home/tester/.cargo/bin/rustcx"
)

check("resolve returns nil when the binary is nowhere", binary.resolve(deps()) == nil)

check(
  "resolve returns nil when the cargo path exists but is not executable",
  binary.resolve(deps({
    executable = function()
      return false
    end,
  })) == nil
)

check(
  "resolve looks for rustcx.exe in the cargo bin on Windows",
  binary.resolve(deps({
    windows = true,
    executable = function(path)
      return path == "/home/tester/.cargo/bin/rustcx.exe"
    end,
  })) == "/home/tester/.cargo/bin/rustcx.exe"
)

check("EXECUTABLE is the plain binary name", binary.EXECUTABLE == "rustcx")

-- lsp/rustcx.lua

local notified = {}
local real_notify = vim.notify
vim.notify = function(msg)
  table.insert(notified, msg)
end
local config = dofile(vim.fs.joinpath(root, "lsp", "rustcx.lua"))
vim.notify = real_notify

check(
  "config sends no init_options, so rustcx.toml applies",
  config.init_options == nil
)
check("config declares only the rust filetype", vim.deep_equal(config.filetypes, { "rust" }))
check("config passes --lsp to the binary", config.cmd[2] == "--lsp")
check("config resolves a command name", type(config.cmd[1]) == "string" and config.cmd[1] ~= "")
check("config exposes root_dir as a predicate", type(config.root_dir) == "function")

-- root_dir declines rather than attaching when the binary is missing, which is
-- Neovim's only per-buffer opt-out. Skipped rather than failed when rustcx is
-- not installed, so the suite passes on a clean machine.
local buf = vim.api.nvim_create_buf(false, true)
vim.api.nvim_buf_set_name(buf, vim.fs.joinpath(root, "scored.rs"))
local dir = nil
config.root_dir(buf, function(found)
  dir = found
end)
if #notified == 0 then
  check("root_dir accepts a rust file", type(dir) == "string" and dir ~= "")
else
  io.write("skip  root_dir accepts a rust file (rustcx not installed)\n")
  check("a missing binary warns exactly once", #notified == 1)
  check("a missing binary declines to attach", dir == nil)
end

io.write(string.format("\n%d passed, %d failed\n", passed, failed))
os.exit(failed == 0 and 0 or 1)
```

- [ ] **Step 2: Run test to verify it fails**

Run: `nvim -l extensions/nvim/test/rustcx_spec.lua`
Expected: FAIL — `module 'rustcx.binary' not found`.

- [ ] **Step 3: Write the resolver**

Create `extensions/nvim/lua/rustcx/binary.lua`:

```lua
--- Locates the `rustcx` executable.
local M = {}

M.EXECUTABLE = "rustcx"

--- The real lookups, injectable so every branch below is testable.
local function real_deps()
  return {
    exepath = function(name)
      return vim.fn.exepath(name)
    end,
    executable = function(path)
      return vim.fn.executable(path) == 1
    end,
    home = vim.uv.os_homedir(),
    windows = vim.fn.has("win32") == 1,
  }
end

--- Resolve the binary: `PATH` first, then Cargo's bin directory.
---
--- The Cargo fallback is not redundant. A GUI frontend such as Neovide or VimR
--- launched from the Dock does not inherit the shell's `PATH`, so
--- `~/.cargo/bin` is routinely invisible to the first step even though the
--- binary is installed and works fine in a terminal.
---
--- Returns `nil` rather than an unchecked path, so a missing binary produces one
--- warning and no client instead of a client that fails to spawn.
---
--- @param overrides table|nil injected `exepath`, `executable`, `home`, `windows` for testing
--- @return string|nil
function M.resolve(overrides)
  local deps = vim.tbl_extend("force", real_deps(), overrides or {})

  local found = deps.exepath(M.EXECUTABLE)
  if found ~= "" and deps.executable(found) then
    return found
  end

  local name = deps.windows and (M.EXECUTABLE .. ".exe") or M.EXECUTABLE
  local cargo = vim.fs.joinpath(deps.home, ".cargo", "bin", name)
  if deps.executable(cargo) then
    return cargo
  end

  return nil
end

return M
```

- [ ] **Step 4: Write the LSP config**

Create `extensions/nvim/lsp/rustcx.lua`:

```lua
--- Neovim LSP config for rustcx. Discovered off the runtimepath by
--- `vim.lsp.enable("rustcx")`; see extensions/nvim/README.md.
local binary = require("rustcx.binary")

local MARKERS = { "Cargo.toml", ".git" }

local NOT_FOUND = table.concat({
  "rustcx not found on PATH or in ~/.cargo/bin. Install it with",
  "'cargo install --path .', or point at it with",
  'vim.lsp.config("rustcx", { cmd = { "/path/to/rustcx", "--lsp" } }).',
}, "\n")

local exe = binary.resolve()
if not exe then
  vim.notify(NOT_FOUND, vim.log.levels.WARN)
end

return {
  cmd = { exe or binary.EXECUTABLE, "--lsp" },
  filetypes = { "rust" },
  -- No `init_options` key on purpose. An absent threshold is what lets
  -- rustcx.toml apply; sending a default here would override the file in every
  -- project. Set one explicitly with:
  --   vim.lsp.config("rustcx", { init_options = { threshold = 15 } })
  --
  -- A predicate rather than `root_markers`, because declining to call `on_dir`
  -- is the only hook Neovim offers for "do not attach to this buffer" — which
  -- is what a missing binary needs.
  root_dir = function(bufnr, on_dir)
    if not exe then
      return
    end
    on_dir(vim.fs.root(bufnr, MARKERS) or vim.fn.getcwd())
  end,
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `nvim -l extensions/nvim/test/rustcx_spec.lua`
Expected: PASS — `13 passed, 0 failed` (or 14 with the skip branch, if `rustcx` is not on `PATH`).

- [ ] **Step 6: Verify a real attach**

Run `cargo install --path .` if `rustcx` is not already on `PATH`, then:

```bash
nvim --clean --cmd 'set rtp^=extensions/nvim' \
     -c 'lua vim.lsp.enable("rustcx")' \
     -c 'lua vim.lsp.config("rustcx", { init_options = { threshold = 8 } })' \
     fixtures/patterns.rs
```

Expected: `:LspInfo` shows the rustcx client attached. `:lua =vim.diagnostic.get(0)` returns at least one entry, and hovering line 16 (`describe`) with `K` shows `complexity 8` and its decision points. Quit with `:qa!`.

- [ ] **Step 7: Write the README**

Create `extensions/nvim/README.md`:

````markdown
# rustcx for Neovim

Cyclomatic complexity diagnostics and hover from `rustcx --lsp`. Requires
Neovim 0.11 or newer and the `rustcx` binary:

```console
cargo install --path .
```

## Install

Two edits to your `init.lua`. First, put this directory on the runtimepath.
With lazy.nvim, add it to your plugin spec list:

```lua
-- Eager on purpose: vim.lsp.enable below runs at startup and needs
-- lsp/rustcx.lua on the runtimepath already.
{ dir = "~/code/rustcx/extensions/nvim" },
```

Adjust that path to wherever you cloned rustcx. Then enable the server,
alongside whatever else you run:

```lua
vim.lsp.enable({ "rust_analyzer", "rustcx" })
```

Do not lazy-load the spec on `ft`, `event`, or `cmd`. `vim.lsp.enable` runs at
startup and needs `lsp/rustcx.lua` on the runtimepath already; a lazy trigger
adds the directory later and the config is silently not found — with no error,
because a config that was never discovered looks the same as one that was never
asked for.

Without a plugin manager, prepend the directory yourself. `runtimepath` does not
expand `~`, so pass it through `expand`:

```lua
vim.opt.runtimepath:prepend(vim.fn.expand("~/code/rustcx/extensions/nvim"))
vim.lsp.enable("rustcx")
```

To confirm the config was found, `:lua =vim.lsp.config.rustcx` should print a
table rather than `nil`. `:LspInfo` then shows whether the client attached to
the current buffer, and `:LspLog` shows a spawn failure if the binary could not
be run.

## Configure

Out of the box this plugin sends **no** settings, so `rustcx.toml` governs —
including its `threshold` key, which is what the editor warns from. See the
main README for that file.

To override the threshold for yourself, without touching a file your whole team
shares:

```lua
vim.lsp.config("rustcx", { init_options = { threshold = 15 } })
```

A function scoring at or above the threshold is a warning; at or above twice
it, an error. The server reads the threshold once, at `initialize`, so run
`:LspRestart rustcx` after changing it. Editing `rustcx.toml` needs no restart —
the server re-reads it when its modification time changes.

The binary is found on `PATH`, falling back to `~/.cargo/bin/rustcx` — that
fallback matters because a GUI frontend launched from the Dock does not inherit
your shell's `PATH`. To point somewhere else:

```lua
vim.lsp.config("rustcx", { cmd = { "/path/to/rustcx", "--lsp" } })
```

Diagnostics arrive through `vim.diagnostic`, so your existing display settings
apply. Attaches to `rust` buffers.

## Check it works

Open `fixtures/patterns.rs` from the repository root with a threshold of 8. It
holds `describe` at complexity 8, so a working setup shows a warning on line 16
and answers `K` there with the decision points behind the score.

## Tests

```console
nvim -l extensions/nvim/test/rustcx_spec.lua
```
````

- [ ] **Step 8: Commit**

```bash
git add extensions/nvim
git commit -m "feat: Neovim plugin for rustcx --lsp"
```

---

## Task 2: Zed extension

**Files:**
- Create: `extensions/zed/extension.toml`
- Create: `extensions/zed/Cargo.toml`
- Create: `extensions/zed/src/lib.rs`
- Create: `extensions/zed/README.md`

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces: nothing other tasks rely on.

Zed cannot be run on this machine, so this task's gate is a clean
`wasm32-unknown-unknown` build rather than a live check.

- [ ] **Step 1: Write the manifest**

Create `extensions/zed/extension.toml`:

```toml
id = "rustcx"
name = "rustcx"
version = "0.1.0"
schema_version = 1
description = "Cyclomatic complexity diagnostics for Rust"
authors = ["Michael Harris <throwingfish@gmail.com>"]
repository = "https://github.com/troglodyte/rustcx"

[language_servers.rustcx]
name = "rustcx"
languages = ["Rust"]
```

- [ ] **Step 2: Write the crate manifest**

Create `extensions/zed/Cargo.toml`:

```toml
[package]
name = "rustcx-extension"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
zed_extension_api = "0.7.0"
serde_json = "1.0"
```

- [ ] **Step 3: Write the extension**

Create `extensions/zed/src/lib.rs`:

```rust
use zed_extension_api as zed;

struct RustcxExtension;

/// `~/.cargo/bin/rustcx`, built from the worktree shell's `HOME`.
///
/// This fallback is what jscx's Zed extension lacks. `which` searches the
/// worktree's `PATH`, which a desktop-launched Zed may not have inherited from
/// a login shell, leaving a cargo-installed binary invisible.
///
/// The path is returned unchecked: the extension runs in a WASI sandbox with no
/// access to arbitrary filesystem paths, so there is no way to stat it from
/// here. A wrong guess surfaces as a spawn failure in Zed's language server
/// log, which is the best this end of the sandbox can do.
fn cargo_bin(worktree: &zed::Worktree) -> Option<String> {
    let home = worktree
        .shell_env()
        .into_iter()
        .find(|(key, _)| key == "HOME")
        .map(|(_, value)| value)?;
    if home.is_empty() {
        return None;
    }
    Some(format!("{home}/.cargo/bin/rustcx"))
}

impl zed::Extension for RustcxExtension {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        _language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> zed::Result<zed::Command> {
        let command = worktree
            .which("rustcx")
            .or_else(|| cargo_bin(worktree))
            .ok_or_else(|| {
                "rustcx was not found on PATH or in ~/.cargo/bin. Install it with \
                 'cargo install --path .' in the rustcx directory."
                    .to_string()
            })?;

        Ok(zed::Command {
            command,
            args: vec!["--lsp".to_string()],
            env: Default::default(),
        })
    }

    /// The threshold, only if the user set one.
    ///
    /// Returning `None` rather than a default is load-bearing: the server
    /// resolves `initializationOptions` over `rustcx.toml`, so sending a
    /// default here would override that file in every project.
    fn language_server_initialization_options(
        &mut self,
        _language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> zed::Result<Option<serde_json::Value>> {
        let threshold = zed::settings::LspSettings::for_worktree("rustcx", worktree)
            .ok()
            .and_then(|s| s.settings)
            .and_then(|s| s.get("threshold").and_then(|v| v.as_u64()))
            .filter(|t| *t > 0);

        Ok(threshold.map(|t| serde_json::json!({ "threshold": t })))
    }
}

zed::register_extension!(RustcxExtension);
```

- [ ] **Step 4: Build for wasm to verify it compiles**

Run: `cd extensions/zed && cargo build --target wasm32-unknown-unknown --release`
Expected: PASS — compiles clean.

If `zed_extension_api` 0.7.0's `Worktree` has no `shell_env` method, or its
signature differs from `Vec<(String, String)>`, read the error, check the
version's API with `cargo doc -p zed_extension_api --open`, and adjust
`cargo_bin` to match. Do not delete the fallback — it is the reason this
extension differs from jscx's.

- [ ] **Step 5: Verify the root crate still builds**

Run: `cargo build && cargo test`
Expected: PASS. The nested crate must not disturb the root package; the root
manifest has no `[workspace]` and cargo globs only `src/`.

- [ ] **Step 6: Write the README**

Create `extensions/zed/README.md`:

````markdown
# rustcx for Zed

Cyclomatic complexity diagnostics and hover from `rustcx --lsp`, running
alongside rust-analyzer.

## Requirements

- The `rustcx` binary: `cargo install --path ../..`
- The `wasm32-unknown-unknown` target, to build the extension:
  `rustup target add wasm32-unknown-unknown`

## Install

Zed builds the extension itself. *Extensions → Install Dev Extension*, and
choose this directory. Zed rebuilds it whenever you reload the extension.

## Configure

By default this extension sends no settings, so `rustcx.toml` governs —
including its `threshold` key. To override the threshold in Zed's settings:

```json
{
  "lsp": {
    "rustcx": {
      "initialization_options": {},
      "settings": { "threshold": 15 }
    }
  }
}
```

A function scoring at or above the threshold is a warning; at or above twice
it, an error. The server reads it once at startup, so restart the language
server after changing it. Editing `rustcx.toml` needs no restart.

The binary is found on the worktree's `PATH`, falling back to
`~/.cargo/bin/rustcx`.

## Check it works

Open `fixtures/patterns.rs` with `threshold` set to 8. `describe` on line 16
scores 8, so a working setup shows a warning there and hover explains the
score.
````

- [ ] **Step 7: Commit**

```bash
git add extensions/zed
git commit -m "feat: Zed extension for rustcx --lsp"
```

---

## Task 3: JetBrains plugin

**Files:**
- Create: `extensions/jetbrains/settings.gradle.kts`, `build.gradle.kts`, `gradle.properties`
- Create: `extensions/jetbrains/gradlew`, `gradlew.bat`, `gradle/wrapper/gradle-wrapper.jar`, `gradle/wrapper/gradle-wrapper.properties` (copied from jscx)
- Create: `extensions/jetbrains/src/main/kotlin/dev/rustcx/intellij/{RustcxBinary,RustcxSettings,RustcxConfigurable,RustcxNotifications,RustcxLspClientDescriptor,RustcxLspIntegrationProvider}.kt`
- Create: `extensions/jetbrains/src/main/resources/META-INF/plugin.xml`
- Create: `extensions/jetbrains/src/test/kotlin/dev/rustcx/intellij/RustcxBinaryTest.kt`
- Create: `extensions/jetbrains/README.md`

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces: `RustcxBinary.resolve(configured: String, pathLookup: (String) -> File?, home: Path): String?` — the only member the tests touch.

Requires a paid JetBrains IDE 2026.2+ to run. `./gradlew test` is this task's
gate; `verifyPlugin` and `runIde` are deliberately not run, because they
download a multi-GB IntelliJ platform.

- [ ] **Step 1: Copy the Gradle wrapper**

`gradle` is not installed on this machine, so the committed wrapper is the only
way to build. jscx pins the same version this plugin needs.

```bash
mkdir -p extensions/jetbrains/gradle/wrapper
cp ~/code/jscx/extensions/jetbrains/gradlew extensions/jetbrains/gradlew
cp ~/code/jscx/extensions/jetbrains/gradlew.bat extensions/jetbrains/gradlew.bat
cp ~/code/jscx/extensions/jetbrains/gradle/wrapper/gradle-wrapper.jar \
   extensions/jetbrains/gradle/wrapper/gradle-wrapper.jar
cp ~/code/jscx/extensions/jetbrains/gradle/wrapper/gradle-wrapper.properties \
   extensions/jetbrains/gradle/wrapper/gradle-wrapper.properties
chmod +x extensions/jetbrains/gradlew
```

Verify `gradle-wrapper.properties` pins `gradle-9.5.0-bin.zip`. The wrapper jar
must be committed; many boilerplate `.gitignore` files exclude it, and Task 5's
`.gitignore` carries a comment saying not to.

- [ ] **Step 2: Write the build files**

Create `extensions/jetbrains/settings.gradle.kts`:

```kotlin
rootProject.name = "rustcx-intellij"

plugins {
    // Lets Gradle provision the JDK 21 toolchain the IntelliJ Platform needs,
    // rather than requiring it to be installed and JAVA_HOME'd by hand.
    // 1.0.0 or newer is required on Gradle 9: earlier versions reference
    // JvmVendorSpec.IBM_SEMERU, which Gradle 9 removed.
    id("org.gradle.toolchains.foojay-resolver-convention") version "1.0.0"
}
```

Create `extensions/jetbrains/gradle.properties`:

```properties
pluginGroup = dev.rustcx
pluginName = rustcx
pluginVersion = 0.1.0

# The IDE the plugin compiles against. Any paid JetBrains IDE with the LSP API
# can run the result; IDEA Ultimate is used here because Rust support and the
# LSP API are both present in it. If this exact build is not published for IDEA
# Ultimate, Gradle's error lists the versions that are — pick the nearest 2026.2
# and update this line.
platformVersion = 2026.2.1

# 262 is 2026.2, which is where the current LSP API names live
# (LspIntegrationProvider / ProjectWideLspClientDescriptor / LspClientManager).
# The pre-rename names — LspServerSupportProvider, ProjectWideLspServerDescriptor,
# LspServerManager — still work in 2026.2 and reach back to 2023.2, so switching
# back is an option if you need to support older IDEs.
pluginSinceBuild = 262

# The platform bundles kotlin-stdlib; bundling a second copy is a load error.
kotlin.stdlib.default.dependency = false

org.gradle.jvmargs = -Xmx2g
org.gradle.caching = true
org.gradle.configuration-cache = true
```

Create `extensions/jetbrains/build.gradle.kts`:

```kotlin
plugins {
    // Kotlin 2.4 is the first release to support Gradle 9.x. Its tested ceiling
    // is Gradle 9.5, which is what the wrapper pins — see tasks.wrapper below.
    kotlin("jvm") version "2.4.10"
    id("org.jetbrains.intellij.platform") version "2.18.1"
}

group = providers.gradleProperty("pluginGroup").get()
version = providers.gradleProperty("pluginVersion").get()

repositories {
    mavenCentral()
    intellijPlatform {
        defaultRepositories()
    }
}

dependencies {
    intellijPlatform {
        intellijIdeaUltimate(providers.gradleProperty("platformVersion"))
    }

    // Plain JUnit 5, deliberately without TestFrameworkType.Platform: these tests
    // exercise pure functions and need no IDE fixture. Pulling in the platform
    // test framework registers com.intellij.tests.JUnit5TestSessionListener,
    // which cannot start outside a real platform test run.
    testImplementation(kotlin("test"))
    testImplementation("org.junit.jupiter:junit-jupiter:5.11.4")
    testRuntimeOnly("org.junit.platform:junit-platform-launcher")
}

intellijPlatform {
    pluginConfiguration {
        name = providers.gradleProperty("pluginName")
        version = providers.gradleProperty("pluginVersion")
        ideaVersion {
            sinceBuild = providers.gradleProperty("pluginSinceBuild")
            // Open-ended: this plugin is installed from disk, not distributed
            // through the marketplace, so pinning an upper bound only creates
            // needless breakage on the next IDE update.
            untilBuild = provider { null }
        }
    }
}

kotlin {
    jvmToolchain(21)
}

tasks.test {
    useJUnitPlatform()
}

tasks.wrapper {
    // Held at a Gradle version the Kotlin plugin actually supports, so
    // regenerating the wrapper does not silently move off it.
    gradleVersion = "9.5.0"
}
```

- [ ] **Step 3: Write the failing test**

Create `extensions/jetbrains/src/test/kotlin/dev/rustcx/intellij/RustcxBinaryTest.kt`:

```kotlin
package dev.rustcx.intellij

import java.io.File
import java.nio.file.Files
import java.nio.file.Path
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

/**
 * Resolution is pure apart from the two dependencies it takes as parameters, so
 * these tests need no IDE fixture — only real files, because `canExecute` is the
 * one thing that cannot be faked.
 */
class RustcxBinaryTest {

    private val temp: Path = Files.createTempDirectory("rustcx-binary-test")

    @AfterTest
    fun cleanUp() {
        temp.toFile().deleteRecursively()
    }

    /** A real, executable file at [relative], and its absolute path. */
    private fun executable(relative: String): String {
        val file = temp.resolve(relative).toFile()
        file.parentFile.mkdirs()
        file.writeText("#!/bin/sh\n")
        check(file.setExecutable(true)) { "could not mark $file executable" }
        return file.absolutePath
    }

    @Test
    fun `a configured path wins`() {
        val configured = executable("custom/rustcx")
        assertEquals(
            configured,
            RustcxBinary.resolve(configured, pathLookup = { null }, home = temp),
        )
    }

    @Test
    fun `a configured path that is not executable resolves to null`() {
        // Not a fallthrough: a typo must surface as an error, not as PATH's copy.
        val onPath = executable("bin/rustcx")
        assertNull(
            RustcxBinary.resolve(
                temp.resolve("nope/rustcx").toString(),
                pathLookup = { File(onPath) },
                home = temp,
            )
        )
    }

    @Test
    fun `PATH is used when nothing is configured`() {
        val onPath = executable("bin/rustcx")
        assertEquals(
            onPath,
            RustcxBinary.resolve("", pathLookup = { File(onPath) }, home = temp),
        )
    }

    @Test
    fun `blank and whitespace configured paths are treated as unset`() {
        val onPath = executable("bin/rustcx")
        assertEquals(
            onPath,
            RustcxBinary.resolve("   ", pathLookup = { File(onPath) }, home = temp),
        )
    }

    @Test
    fun `the cargo bin is the fallback when PATH has no hit`() {
        val cargo = executable(".cargo/bin/rustcx")
        assertEquals(
            cargo,
            RustcxBinary.resolve("", pathLookup = { null }, home = temp),
        )
    }

    @Test
    fun `a non-executable PATH hit falls through to the cargo bin`() {
        val cargo = executable(".cargo/bin/rustcx")
        val plainFile = temp.resolve("bin/plain").toFile()
        plainFile.parentFile.mkdirs()
        plainFile.writeText("not executable")
        plainFile.setExecutable(false)

        assertEquals(
            cargo,
            RustcxBinary.resolve("", pathLookup = { plainFile }, home = temp),
        )
    }

    @Test
    fun `nothing anywhere resolves to null`() {
        assertNull(RustcxBinary.resolve("", pathLookup = { null }, home = temp))
    }

    @Test
    fun `the executable name is the plain binary name`() {
        assertEquals("rustcx", RustcxBinary.EXECUTABLE)
    }
}
```

- [ ] **Step 4: Run test to verify it fails**

Run: `cd extensions/jetbrains && ./gradlew test`
Expected: FAIL — `Unresolved reference: RustcxBinary`. The first run downloads
the Gradle 9.5.0 distribution and the IntelliJ platform dependency; allow
several minutes.

- [ ] **Step 5: Write the resolver**

Create `extensions/jetbrains/src/main/kotlin/dev/rustcx/intellij/RustcxBinary.kt`:

```kotlin
package dev.rustcx.intellij

import com.intellij.execution.configurations.PathEnvironmentVariableUtil
import java.io.File
import java.nio.file.Path

/** Locates the `rustcx` executable. */
object RustcxBinary {

    const val EXECUTABLE: String = "rustcx"

    /**
     * Resolution order: the configured path, then `PATH`, then Cargo's bin
     * directory.
     *
     * The Cargo fallback is not redundant. An IDE launched from the Dock or the
     * Start menu does not inherit the shell's `PATH`, so `~/.cargo/bin` is
     * routinely invisible to the second step even though the binary is installed
     * and works fine in a terminal.
     *
     * A configured path that is not executable resolves to `null` rather than
     * silently falling through, so a typo surfaces as an error instead of as
     * mysteriously different behavior.
     *
     * @param pathLookup injected for testing; defaults to the platform's `PATH` search
     * @param home injected for testing; defaults to the user's home directory
     */
    fun resolve(
        configured: String,
        // findExecutableInPathOnAnyOS, rather than findInPath, so Windows finds rustcx.exe.
        pathLookup: (String) -> File? = { PathEnvironmentVariableUtil.findExecutableInPathOnAnyOS(it) },
        home: Path = Path.of(System.getProperty("user.home")),
    ): String? {
        val explicit = configured.trim()
        if (explicit.isNotEmpty()) {
            return explicit.takeIf { File(it).canExecute() }
        }

        pathLookup(EXECUTABLE)?.let { found ->
            if (found.canExecute()) return found.absolutePath
        }

        val cargo = home.resolve(".cargo").resolve("bin").resolve(executableName()).toFile()
        return cargo.absolutePath.takeIf { cargo.canExecute() }
    }

    private fun executableName(): String =
        if (isWindows()) "$EXECUTABLE.exe" else EXECUTABLE

    private fun isWindows(): Boolean =
        System.getProperty("os.name").orEmpty().startsWith("Windows", ignoreCase = true)
}
```

- [ ] **Step 6: Run test to verify it passes**

Run: `cd extensions/jetbrains && ./gradlew test`
Expected: PASS — 8 tests.

- [ ] **Step 7: Write the settings**

Create `extensions/jetbrains/src/main/kotlin/dev/rustcx/intellij/RustcxSettings.kt`:

```kotlin
package dev.rustcx.intellij

import com.intellij.openapi.components.BaseState
import com.intellij.openapi.components.Service
import com.intellij.openapi.components.SimplePersistentStateComponent
import com.intellij.openapi.components.State
import com.intellij.openapi.components.Storage
import com.intellij.openapi.components.service

/**
 * Application-wide configuration for the rustcx language server.
 *
 * Application rather than project scope: the threshold is a judgement about what
 * counts as too complex, which does not usually change from one checkout to the
 * next, and this mirrors setting it once in Zed's user settings.
 */
@Service(Service.Level.APP)
@State(name = "RustcxSettings", storages = [Storage("rustcx.xml")])
class RustcxSettings : SimplePersistentStateComponent<RustcxSettings.State>(State()) {

    class State : BaseState() {
        /**
         * Warn at this score, error at twice it. `UNSET` means send nothing and
         * let `rustcx.toml` decide — a real threshold is always at least 1, so
         * zero is free to carry that meaning.
         */
        var threshold: Int by property(UNSET)

        /** Explicit path to the rustcx executable; blank means resolve from PATH. */
        var binaryPath: String? by string("")
    }

    var threshold: Int
        get() = state.threshold
        set(value) {
            state.threshold = value
        }

    var binaryPath: String
        get() = state.binaryPath.orEmpty()
        set(value) {
            state.binaryPath = value
        }

    companion object {
        /**
         * "No threshold set here." The server's own default lives in
         * `rustcx::lsp::DEFAULT_THRESHOLD` and is deliberately not duplicated:
         * sending a copy of it would override the user's `rustcx.toml`.
         */
        const val UNSET: Int = 0

        fun getInstance(): RustcxSettings = service()
    }
}
```

- [ ] **Step 8: Write the notifications**

Create `extensions/jetbrains/src/main/kotlin/dev/rustcx/intellij/RustcxNotifications.kt`:

```kotlin
package dev.rustcx.intellij

import com.intellij.notification.NotificationAction
import com.intellij.notification.NotificationGroupManager
import com.intellij.notification.NotificationType
import com.intellij.openapi.options.ShowSettingsUtil
import com.intellij.openapi.project.Project
import java.util.concurrent.ConcurrentHashMap

internal object RustcxNotifications {

    private const val GROUP = "rustcx"

    /** Projects already warned this session, so opening ten files warns once. */
    private val warned: MutableSet<String> = ConcurrentHashMap.newKeySet()

    fun binaryNotFound(project: Project, configured: String) {
        if (!warned.add(project.locationHash)) return

        val message = if (configured.isBlank()) {
            "rustcx was not found on PATH. Install it with <code>cargo install --path .</code>, " +
                "or set its location in settings."
        } else {
            "rustcx is not executable at <code>$configured</code>."
        }

        NotificationGroupManager.getInstance()
            .getNotificationGroup(GROUP)
            .createNotification("rustcx unavailable", message, NotificationType.WARNING)
            .addAction(
                NotificationAction.createSimple("Configure") {
                    ShowSettingsUtil.getInstance()
                        .showSettingsDialog(project, RustcxConfigurable::class.java)
                }
            )
            .notify(project)
    }
}
```

- [ ] **Step 9: Write the LSP client descriptor and integration provider**

Create `extensions/jetbrains/src/main/kotlin/dev/rustcx/intellij/RustcxLspClientDescriptor.kt`:

```kotlin
package dev.rustcx.intellij

import com.intellij.execution.ExecutionException
import com.intellij.execution.configurations.GeneralCommandLine
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.platform.lsp.api.ProjectWideLspClientDescriptor

/**
 * Runs `rustcx --lsp` and hands it the threshold, if one is set.
 *
 * No feature support is declared here: the platform client gates its requests on
 * the capabilities the server advertises in its `initialize` response, which are
 * `textDocumentSync` and `hoverProvider`. If hover does not work in a running
 * IDE, this is the file that needs an explicit override — see the README.
 */
internal class RustcxLspClientDescriptor(project: Project) :
    ProjectWideLspClientDescriptor(project, "rustcx") {

    override fun isSupportedFile(file: VirtualFile): Boolean = isSupportedByRustcx(file)

    override fun createCommandLine(): GeneralCommandLine {
        val configured = RustcxSettings.getInstance().binaryPath
        val binary = RustcxBinary.resolve(configured)
            ?: throw ExecutionException(
                if (configured.isBlank()) {
                    "rustcx was not found on PATH. Install it with 'cargo install --path .' " +
                        "or set its location under Settings | Tools | rustcx."
                } else {
                    "rustcx is not executable at '$configured'. Fix the path under Settings | Tools | rustcx."
                }
            )

        return GeneralCommandLine(binary, "--lsp")
    }

    /**
     * `null` when no threshold is set, so the server falls back to `rustcx.toml`
     * — `Server::settings` resolves initialization options *over* the file, so a
     * default sent from here would silently disable that key everywhere.
     */
    override fun createInitializationOptions(): Any? =
        RustcxSettings.getInstance().threshold
            .takeIf { it != RustcxSettings.UNSET }
            ?.let { mapOf("threshold" to it) }
}
```

Create `extensions/jetbrains/src/main/kotlin/dev/rustcx/intellij/RustcxLspIntegrationProvider.kt`:

```kotlin
package dev.rustcx.intellij

import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.platform.lsp.api.LspIntegrationProvider

internal class RustcxLspIntegrationProvider : LspIntegrationProvider {

    override fun fileOpened(
        project: Project,
        file: VirtualFile,
        clientStarter: LspIntegrationProvider.LspClientStarter,
    ) {
        if (!isSupportedByRustcx(file)) return

        val configured = RustcxSettings.getInstance().binaryPath
        if (RustcxBinary.resolve(configured) == null) {
            RustcxNotifications.binaryNotFound(project, configured)
            return
        }

        clientStarter.ensureClientStarted(RustcxLspClientDescriptor(project))
    }
}

/**
 * Whether rustcx should score this file.
 *
 * Just the extension. jscx needs a richer filter because its CLI skips `.d.ts`
 * declaration files while its server scores whatever arrives; rustcx has no such
 * asymmetry, so there is nothing here but `.rs`.
 */
internal fun isSupportedByRustcx(file: VirtualFile): Boolean =
    file.extension?.lowercase() == "rs"
```

- [ ] **Step 10: Write the settings panel**

Create `extensions/jetbrains/src/main/kotlin/dev/rustcx/intellij/RustcxConfigurable.kt`:

```kotlin
package dev.rustcx.intellij

import com.intellij.openapi.options.BoundConfigurable
import com.intellij.openapi.project.ProjectManager
import com.intellij.openapi.ui.DialogPanel
import com.intellij.platform.lsp.api.LspClientManager
import com.intellij.ui.dsl.builder.bindIntText
import com.intellij.ui.dsl.builder.bindText
import com.intellij.ui.dsl.builder.panel

/** Settings | Tools | rustcx */
internal class RustcxConfigurable : BoundConfigurable("rustcx") {

    private val settings = RustcxSettings.getInstance()

    private var threshold: Int = settings.threshold
    private var binaryPath: String = settings.binaryPath

    override fun createPanel(): DialogPanel = panel {
        row("Complexity threshold:") {
            // 0 is offered on purpose: it is how you hand the decision back to
            // rustcx.toml after having set a value here.
            intTextField(range = 0..10_000)
                .bindIntText(::threshold)
                .comment(
                    "Functions scoring at or above this value are warnings; at or above " +
                        "twice it, errors. Leave at <code>0</code> to use the " +
                        "<code>threshold</code> in <code>rustcx.toml</code>, or the built-in " +
                        "default if there is no such file."
                )
        }
        row("rustcx binary:") {
            textField()
                .bindText(::binaryPath)
                .comment(
                    "Leave blank to resolve <code>rustcx</code> from <code>PATH</code>, " +
                        "falling back to <code>~/.cargo/bin/rustcx</code>."
                )
        }
    }

    override fun apply() {
        super.apply()
        settings.threshold = threshold
        settings.binaryPath = binaryPath.trim()
        restartServers()
    }

    /**
     * The server reads its threshold once, during `initialize`, and implements no
     * `workspace/didChangeConfiguration` handler — so a settings change only takes
     * effect on restart.
     */
    private fun restartServers() {
        for (project in ProjectManager.getInstance().openProjects) {
            if (project.isDisposed) continue
            LspClientManager.getInstance(project)
                .stopAndRestartClientsIfNeeded(RustcxLspIntegrationProvider::class.java)
        }
    }
}
```

- [ ] **Step 11: Write the plugin descriptor**

Create `extensions/jetbrains/src/main/resources/META-INF/plugin.xml`:

```xml
<idea-plugin>
    <id>dev.rustcx.intellij</id>
    <name>rustcx</name>
    <vendor url="https://github.com/troglodyte/rustcx">Michael Harris</vendor>

    <description><![CDATA[
        Cyclomatic complexity diagnostics and hover for Rust.
        <p>
        Functions scoring at or above the configured threshold are reported as
        warnings; at or above twice the threshold, as errors. Hovering a
        function's signature or one of its branchy lines explains the score.
        </p>
        <p>
        Requires the <code>rustcx</code> binary on your <code>PATH</code>
        (<code>cargo install --path .</code>), or an explicit path set under
        <b>Settings | Tools | rustcx</b>.
        </p>
    ]]></description>

    <depends>com.intellij.modules.platform</depends>
    <!-- The LSP API is only present in paid JetBrains IDEs. This dependency is
         what keeps the plugin from being installable where it cannot work. -->
    <depends>com.intellij.modules.lsp</depends>

    <extensions defaultExtensionNs="com.intellij">
        <platform.lsp.integrationProvider
                implementation="dev.rustcx.intellij.RustcxLspIntegrationProvider"/>

        <applicationConfigurable
                parentId="tools"
                id="dev.rustcx.intellij.settings"
                displayName="rustcx"
                instance="dev.rustcx.intellij.RustcxConfigurable"/>

        <notificationGroup id="rustcx" displayType="BALLOON"/>
    </extensions>
</idea-plugin>
```

- [ ] **Step 12: Run the full build to verify everything compiles**

Run: `cd extensions/jetbrains && ./gradlew test`
Expected: PASS — 8 tests, and all six Kotlin files compile against the platform.

Compilation is the real check here: the tests only cover `RustcxBinary`, but
`./gradlew test` compiles the whole main source set, which is what catches a
wrong API name in the descriptor or the configurable.

- [ ] **Step 13: Write the README**

Create `extensions/jetbrains/README.md`:

````markdown
# rustcx JetBrains Plugin

Cyclomatic complexity diagnostics and hover for Rust in IntelliJ IDEA Ultimate
and the other paid JetBrains IDEs.

The plugin is a thin LSP client. It resolves the `rustcx` binary, runs
`rustcx --lsp`, and passes the threshold as an initialization option if one is
set — all scoring lives in the Rust server, shared with the other extensions.

## Requirements

- The `rustcx` binary: `cargo install --path ../..`
- A paid JetBrains IDE, 2026.2 or newer. The IntelliJ LSP API does not exist in
  IDEA Community or Android Studio. See `pluginSinceBuild` in
  `gradle.properties` for what it would take to support older IDEs.

The Gradle wrapper is committed, so no system Gradle is needed.

## Build

```console
./gradlew buildPlugin
```

Install the resulting zip — not either jar inside it — with
*Settings | Plugins | ⚙ | Install Plugin from Disk*, then restart the IDE:

```text
build/distributions/rustcx-intellij-0.1.0.zip
```

That path is relative to this directory, and the version in it comes from
`pluginVersion` in `gradle.properties`.

Other useful tasks:

| Task | Purpose |
| --- | --- |
| `./gradlew test` | Unit tests for binary resolution |
| `./gradlew verifyPlugin` | JetBrains plugin verifier against the target IDE |
| `./gradlew runIde` | Launch a sandbox IDE with the plugin loaded |

## Configuration

*Settings | Tools | rustcx*

| Setting | Default | Meaning |
| --- | --- | --- |
| Complexity threshold | 0 | Warning at or above this score, error at or above twice it. `0` hands the decision to `rustcx.toml`. |
| rustcx binary | blank | Blank resolves from `PATH`, then `~/.cargo/bin/rustcx` |

Changing either setting restarts the language server, because the server reads
its options only during `initialize`. Editing `rustcx.toml` needs no restart —
the server re-reads it when its modification time changes.

## Check it works

Open `fixtures/patterns.rs` from the repository root with the threshold set to
8. `describe` on line 16 scores 8, so a working setup shows a warning there and
explains the score on hover.

If diagnostics appear but hover does not, the platform client is not issuing
hover requests despite the server advertising `hoverProvider`. The fix belongs
in `RustcxLspClientDescriptor`, which currently declares no explicit feature
support.

## Layout

| File | Role |
| --- | --- |
| `RustcxLspIntegrationProvider.kt` | Decides which files start the server |
| `RustcxLspClientDescriptor.kt` | Builds the command line and initialization options |
| `RustcxBinary.kt` | Locates the executable |
| `RustcxSettings.kt` | Persisted threshold and binary path |
| `RustcxConfigurable.kt` | The settings panel |
| `RustcxNotifications.kt` | The missing-binary balloon |
````

- [ ] **Step 14: Commit**

```bash
git add extensions/jetbrains
git commit -m "feat: JetBrains plugin for rustcx --lsp"
```

---

## Task 4: VS Code extension

**Files:**
- Create: `extensions/vscode/package.json`, `tsconfig.json`, `.vscodeignore`
- Create: `extensions/vscode/src/resolve.ts`, `src/extension.ts`
- Create: `extensions/vscode/test/resolve.test.js`
- Create: `extensions/vscode/README.md`

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces: `resolveBinary(configured: string, env: ResolveEnv): string | undefined`, where `ResolveEnv` is `{ path: string; home: string; windows: boolean; isExecutable: (p: string) => boolean }`.

This is the one target with no jscx precedent. Resolution lives in its own
module so it can be tested under `node --test` without a running VS Code.

- [ ] **Step 1: Write the package manifest**

Create `extensions/vscode/package.json`:

```json
{
  "name": "rustcx",
  "displayName": "rustcx",
  "description": "Cyclomatic complexity diagnostics and hover for Rust",
  "version": "0.1.0",
  "publisher": "troglodyte",
  "repository": { "type": "git", "url": "https://github.com/troglodyte/rustcx" },
  "engines": { "vscode": "^1.90.0" },
  "categories": ["Linters", "Programming Languages"],
  "activationEvents": ["onLanguage:rust"],
  "main": "./out/extension.js",
  "contributes": {
    "configuration": {
      "title": "rustcx",
      "properties": {
        "rustcx.threshold": {
          "type": ["number", "null"],
          "default": null,
          "minimum": 1,
          "markdownDescription": "Functions scoring at or above this value are warnings; at or above twice it, errors. Leave `null` to use the `threshold` in `rustcx.toml`, or the built-in default if there is no such file."
        },
        "rustcx.path": {
          "type": "string",
          "default": "",
          "markdownDescription": "Path to the `rustcx` executable. Leave blank to resolve from `PATH`, falling back to `~/.cargo/bin/rustcx`."
        }
      }
    }
  },
  "scripts": {
    "compile": "tsc -p .",
    "test": "npm run compile && node --test out/test/"
  },
  "dependencies": {
    "vscode-languageclient": "^9.0.1"
  },
  "devDependencies": {
    "@types/node": "^22.0.0",
    "@types/vscode": "^1.90.0",
    "typescript": "^5.6.0"
  }
}
```

Create `extensions/vscode/tsconfig.json`:

```json
{
  "compilerOptions": {
    "module": "commonjs",
    "target": "ES2022",
    "lib": ["ES2022"],
    "outDir": "out",
    "rootDir": ".",
    "strict": true,
    "sourceMap": true,
    "skipLibCheck": true,
    "allowJs": true
  },
  "include": ["src/**/*.ts", "test/**/*.js"]
}
```

Create `extensions/vscode/.vscodeignore`:

```text
.vscode/**
node_modules/**
src/**
test/**
tsconfig.json
**/*.map
```

- [ ] **Step 2: Install dependencies**

Run: `cd extensions/vscode && npm install`
Expected: `node_modules/` populated, `package-lock.json` written.

- [ ] **Step 3: Write the failing test**

Create `extensions/vscode/test/resolve.test.js`:

```js
const { test } = require("node:test");
const assert = require("node:assert/strict");
const { resolveBinary } = require("../src/resolve.js");

/** An environment where nothing is executable, for tests to override piecemeal. */
function env(overrides = {}) {
  return {
    path: "/usr/bin:/usr/local/bin",
    home: "/home/tester",
    windows: false,
    isExecutable: () => false,
    ...overrides,
  };
}

test("a configured path wins", () => {
  const found = resolveBinary("/opt/rustcx", env({ isExecutable: (p) => p === "/opt/rustcx" }));
  assert.equal(found, "/opt/rustcx");
});

test("a configured path that is not executable resolves to undefined", () => {
  // Not a fallthrough: a typo must surface, not silently use PATH's copy.
  const found = resolveBinary(
    "/opt/typo",
    env({ isExecutable: (p) => p === "/usr/local/bin/rustcx" }),
  );
  assert.equal(found, undefined);
});

test("PATH is searched when nothing is configured", () => {
  const found = resolveBinary("", env({ isExecutable: (p) => p === "/usr/local/bin/rustcx" }));
  assert.equal(found, "/usr/local/bin/rustcx");
});

test("a blank configured path is treated as unset", () => {
  const found = resolveBinary("   ", env({ isExecutable: (p) => p === "/usr/bin/rustcx" }));
  assert.equal(found, "/usr/bin/rustcx");
});

test("the cargo bin is the fallback when PATH has no hit", () => {
  const found = resolveBinary(
    "",
    env({ isExecutable: (p) => p === "/home/tester/.cargo/bin/rustcx" }),
  );
  assert.equal(found, "/home/tester/.cargo/bin/rustcx");
});

test("nothing anywhere resolves to undefined", () => {
  assert.equal(resolveBinary("", env()), undefined);
});

test("windows looks for rustcx.exe, on PATH and in the cargo bin", () => {
  const onPath = resolveBinary(
    "",
    env({
      windows: true,
      path: "C:\\bin;C:\\tools",
      isExecutable: (p) => p === "C:\\tools\\rustcx.exe",
    }),
  );
  assert.equal(onPath, "C:\\tools\\rustcx.exe");

  const inCargo = resolveBinary(
    "",
    env({
      windows: true,
      path: "C:\\bin",
      home: "C:\\Users\\tester",
      isExecutable: (p) => p === "C:\\Users\\tester\\.cargo\\bin\\rustcx.exe",
    }),
  );
  assert.equal(inCargo, "C:\\Users\\tester\\.cargo\\bin\\rustcx.exe");
});

test("an empty PATH is not a crash", () => {
  assert.equal(resolveBinary("", env({ path: "" })), undefined);
});
```

- [ ] **Step 4: Run test to verify it fails**

Run: `cd extensions/vscode && npm test`
Expected: FAIL — `Cannot find module '../src/resolve.js'`.

- [ ] **Step 5: Write the resolver**

Create `extensions/vscode/src/resolve.ts`:

```ts
import { accessSync, constants } from "node:fs";
import { homedir } from "node:os";
import { delimiter, join } from "node:path";

/** Everything resolution touches outside itself, injected so it can be tested. */
export interface ResolveEnv {
  /** The `PATH` variable, unsplit. */
  path: string;
  /** The user's home directory. */
  home: string;
  windows: boolean;
  isExecutable: (candidate: string) => boolean;
}

/** The real environment. */
export function realEnv(): ResolveEnv {
  return {
    path: process.env.PATH ?? "",
    home: homedir(),
    windows: process.platform === "win32",
    isExecutable: (candidate) => {
      try {
        accessSync(candidate, constants.X_OK);
        return true;
      } catch {
        return false;
      }
    },
  };
}

/**
 * Locate the `rustcx` executable: the configured path, then `PATH`, then Cargo's
 * bin directory.
 *
 * The Cargo fallback is not redundant. A VS Code launched from the Dock or the
 * Start menu does not inherit the shell's `PATH`, so `~/.cargo/bin` is routinely
 * invisible to the second step even though the binary is installed and works
 * fine in a terminal.
 *
 * A configured path that is not executable resolves to `undefined` rather than
 * silently falling through, so a typo surfaces as an error instead of as
 * mysteriously different behavior.
 */
export function resolveBinary(configured: string, env: ResolveEnv): string | undefined {
  const name = env.windows ? "rustcx.exe" : "rustcx";

  const explicit = configured.trim();
  if (explicit.length > 0) {
    return env.isExecutable(explicit) ? explicit : undefined;
  }

  for (const dir of env.path.split(delimiter)) {
    if (dir.length === 0) continue;
    const candidate = join(dir, name);
    if (env.isExecutable(candidate)) return candidate;
  }

  const cargo = join(env.home, ".cargo", "bin", name);
  return env.isExecutable(cargo) ? cargo : undefined;
}
```

- [ ] **Step 6: Run test to verify it passes**

Run: `cd extensions/vscode && npm test`
Expected: PASS — 8 tests.

Note `join` normalizes separators per platform, so the Windows assertions pass
only when run on Windows. If they fail on Linux with forward slashes in the
actual value, change those two assertions to build their expected values with
`join` too rather than weakening the test.

- [ ] **Step 7: Write the extension entry point**

Create `extensions/vscode/src/extension.ts`:

```ts
import * as vscode from "vscode";
import { LanguageClient, LanguageClientOptions, ServerOptions } from "vscode-languageclient/node";
import { realEnv, resolveBinary } from "./resolve";

let client: LanguageClient | undefined;

const NOT_FOUND =
  "rustcx was not found on PATH or in ~/.cargo/bin. Install it with " +
  "'cargo install --path .', or set rustcx.path.";

const NOT_EXECUTABLE = (configured: string) =>
  `rustcx is not executable at '${configured}'. Fix rustcx.path.`;

/**
 * The threshold to send, or `undefined` to send nothing.
 *
 * Sending nothing is what lets `rustcx.toml` apply: the server resolves
 * initialization options *over* the file, so a default sent from here would
 * silently disable that key in every project.
 */
function threshold(): number | undefined {
  const configured = vscode.workspace.getConfiguration("rustcx").get<number | null>("threshold");
  return typeof configured === "number" && configured > 0 ? configured : undefined;
}

async function start(): Promise<void> {
  const configured = vscode.workspace.getConfiguration("rustcx").get<string>("path", "");
  const binary = resolveBinary(configured, realEnv());

  if (binary === undefined) {
    const message = configured.trim().length > 0 ? NOT_EXECUTABLE(configured) : NOT_FOUND;
    const action = await vscode.window.showWarningMessage(message, "Open Settings");
    if (action === "Open Settings") {
      await vscode.commands.executeCommand("workbench.action.openSettings", "rustcx");
    }
    return;
  }

  const serverOptions: ServerOptions = { command: binary, args: ["--lsp"] };

  const configured_threshold = threshold();
  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: "file", language: "rust" }],
    initializationOptions:
      configured_threshold === undefined ? undefined : { threshold: configured_threshold },
  };

  client = new LanguageClient("rustcx", "rustcx", serverOptions, clientOptions);
  await client.start();
}

async function stop(): Promise<void> {
  const running = client;
  client = undefined;
  await running?.stop();
}

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  await start();

  // The server reads its options once, at `initialize`, and implements no
  // workspace/didChangeConfiguration handler — so a settings change means a
  // restart. Editing rustcx.toml does not: the server watches its mtime.
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration(async (event) => {
      if (!event.affectsConfiguration("rustcx")) return;
      await stop();
      await start();
    }),
  );
}

export async function deactivate(): Promise<void> {
  await stop();
}
```

- [ ] **Step 8: Compile and verify the tests still pass**

Run: `cd extensions/vscode && npm test`
Expected: PASS — TypeScript compiles clean under `strict`, 8 tests pass.

- [ ] **Step 9: Package and install, then verify live**

```bash
cd extensions/vscode
npx --yes @vscode/vsce package --allow-missing-repository --skip-license
code --install-extension rustcx-0.1.0.vsix --force
```

Then open `fixtures/patterns.rs` in VS Code with `"rustcx.threshold": 8` in
settings. Expected: a warning on line 16 (`describe`, complexity 8), and
hovering that line shows the decision points. Check *Output → rustcx* if
nothing appears.

Delete the `.vsix` afterwards; it is a build artifact, not a source file.

- [ ] **Step 10: Write the README**

Create `extensions/vscode/README.md`:

````markdown
# rustcx for VS Code

Cyclomatic complexity diagnostics and hover from `rustcx --lsp`.

## Requirements

- The `rustcx` binary: `cargo install --path ../..`
- Node and npm, to build the extension

## Build and install

```console
npm install
npx @vscode/vsce package --allow-missing-repository --skip-license
code --install-extension rustcx-0.1.0.vsix --force
```

## Configure

*Settings → Extensions → rustcx*

| Setting | Default | Meaning |
| --- | --- | --- |
| `rustcx.threshold` | `null` | Warning at or above this score, error at or above twice it. `null` hands the decision to `rustcx.toml`. |
| `rustcx.path` | `""` | Blank resolves from `PATH`, then `~/.cargo/bin/rustcx` |

Changing either setting restarts the language server automatically, because the
server reads its options only during `initialize`. Editing `rustcx.toml` needs
no restart — the server re-reads it when its modification time changes.

## Check it works

Open `fixtures/patterns.rs` from the repository root with `rustcx.threshold` set
to 8. `describe` on line 16 scores 8, so a working setup shows a warning there
and explains the score on hover. *Output → rustcx* carries the server log.

## Tests

```console
npm test
```

Covers binary resolution, which is the only part of this extension that can run
outside a VS Code process.
````

- [ ] **Step 11: Commit**

```bash
cd /home/trog/code/rustcx
git add extensions/vscode
git commit -m "feat: VS Code extension for rustcx --lsp"
```

---

## Task 5: Repository documentation and ignores

**Files:**
- Modify: `README.md` — the "In your editor" section
- Modify: `.gitignore`

**Interfaces:**
- Consumes: the four extension directories and their READMEs from Tasks 1–4.
- Produces: nothing.

- [ ] **Step 1: Extend .gitignore**

Replace `.gitignore` with:

```text
/target

# Gradle — JetBrains plugin
.gradle/
.kotlin/
local.properties
# The IntelliJ Platform Gradle Plugin's working directory: downloaded IDE
# artifacts, product releases, and the runIde sandbox. Hundreds of megabytes.
.intellijPlatform/
# The Gradle wrapper is deliberately NOT ignored — gradlew, gradlew.bat and
# gradle/wrapper/ (including the jar) pin the Gradle version the build needs.
# Many boilerplate .gitignore files exclude gradle-wrapper.jar; do not add it.

# Zed extension — compiled by Zed on every install/rebuild
extensions/*/extension.wasm
extensions/*/grammars/

# Node — VS Code extension
node_modules/
out/
*.vsix

# Build output
build/
```

- [ ] **Step 2: Verify nothing needed is now ignored**

Run: `git status --porcelain --ignored extensions | grep '^!!'`
Expected: no line naming `gradle-wrapper.jar`, `gradlew`, or any `.kt`, `.lua`,
`.ts`, `.toml`, `.json`, or `.md` source file. If one appears, the pattern that
caught it is wrong — fix it before committing.

- [ ] **Step 3: Rewrite the README's editor section**

In `README.md`, replace everything from the `## In your editor` heading up to
the `## Configuration` heading with:

````markdown
## In your editor

`rustcx --lsp` speaks LSP over stdio. It publishes a warning for every function
at or above a threshold — an error at twice it — and answers hover over a
function with the decision points behind its score.

Hover works on a function's signature line and on any line it was charged a
point for. Those are the branchy lines, which are the ones worth hovering; a
blank line inside a function answers nothing.

Four editor integrations live under `extensions/`, each with its own README:

| Editor | Directory | Install |
| --- | --- | --- |
| Neovim | [`extensions/nvim`](extensions/nvim/README.md) | Add the directory to your runtimepath, then `vim.lsp.enable("rustcx")` |
| Zed | [`extensions/zed`](extensions/zed/README.md) | *Extensions → Install Dev Extension* |
| JetBrains | [`extensions/jetbrains`](extensions/jetbrains/README.md) | `./gradlew buildPlugin`, then install the zip from disk |
| VS Code | [`extensions/vscode`](extensions/vscode/README.md) | `vsce package`, then `code --install-extension` |

All four run the same `rustcx --lsp` binary, resolve it the same way (`PATH`,
then `~/.cargo/bin/rustcx`), and expose the same two settings: a threshold and
an explicit binary path. Each sends a threshold **only if you set one**, so
`rustcx.toml` governs by default in every editor.

The JetBrains plugin needs a paid IDE — the IntelliJ LSP API does not exist in
IDEA Community or Android Studio.

### Helix

Helix needs no extension. In `languages.toml`:

```toml
[language-server.rustcx]
command = "rustcx"
args = ["--lsp"]

[[language]]
name = "rust"
language-servers = ["rust-analyzer", "rustcx"]
```

### While you type

A buffer is unparseable for most of the time you are typing into it. rustcx
keeps the last diagnostics it published and keeps answering hover from the last
successful parse, rather than clearing warnings on every keystroke or reporting
a syntax error rust-analyzer has already reported. The parse failure is written
to the language-server log (`:LspLog` in Neovim), once per distinct error.
````

- [ ] **Step 4: Verify the README's own claims still hold**

Run: `cargo run -- --max 10 --no-tests src/ && cargo test`
Expected: PASS. No source changed, so this is a guard against an accidental
edit outside the README's editor section.

Then read back the rewritten section and confirm no link points at a file that
does not exist:

Run: `for f in nvim zed jetbrains vscode; do test -f extensions/$f/README.md && echo "ok $f" || echo "MISSING $f"; done`
Expected: four `ok` lines.

- [ ] **Step 5: Commit**

```bash
git add README.md .gitignore
git commit -m "docs: point the README at the four editor extensions"
```

---

## Done when

- `nvim -l extensions/nvim/test/rustcx_spec.lua` passes.
- `cd extensions/zed && cargo build --target wasm32-unknown-unknown --release` succeeds.
- `cd extensions/jetbrains && ./gradlew test` passes.
- `cd extensions/vscode && npm test` passes.
- `cargo test` at the root still passes, and `cargo run -- --max 10 --no-tests src/` still exits 0.
- `fixtures/patterns.rs` shows a warning on `describe` and explains it on hover in Neovim and in VS Code — the two editors that can be driven on this machine.
