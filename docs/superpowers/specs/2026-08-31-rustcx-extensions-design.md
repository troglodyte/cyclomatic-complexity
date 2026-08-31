# rustcx editor extensions — design

Four editor integrations for `rustcx --lsp`: Neovim, Zed, JetBrains, and VS
Code. Each is a thin LSP client that resolves the `rustcx` binary, runs it, and
gets out of the way. All scoring, all defaults, and all configuration handling
stay in the server.

The pattern is ported from the sibling [`jscx`](https://github.com/troglodyte/jscx)
repository, whose `extensions/` directory holds three of the four. Where this
design departs from that port, it says so and why.

## Why this work exists

`src/lsp.rs` is complete and, on features, ahead of jscx's: diagnostics, hover,
`rustcx.toml` discovery with mtime reload, retention of the last good parse
while a buffer is mid-edit, and log dedupe. What is missing is entirely the
client layer. Today an editor can only attach through hand-written
configuration pasted from the README, and only Neovim and Helix can do even
that.

## The client contract

Every target implements the same four rules. This section is the reason the
four are one spec rather than four: the risk in this work is not volume, it is
four clients drifting into four different ideas of where the binary lives and
what the default threshold is.

### Command

`rustcx --lsp`. No other arguments. No setting is ever passed on the command
line.

### Binary resolution

In order:

1. **A configured path**, if the user set one. If it is not executable, that is
   a hard error naming the setting — not a fallthrough to step 2. A typo must
   surface as a broken path rather than as mysteriously default behavior.
2. **`PATH`.**
3. **`~/.cargo/bin/rustcx`**, or `rustcx.exe` on Windows. This step is not
   redundant with `PATH`. A GUI-launched editor — Neovide, or an IDE started
   from the Dock or Start menu — does not inherit the shell's `PATH`, so
   `~/.cargo/bin` is routinely invisible to step 2 even though the binary is
   installed and works in a terminal.
4. **Otherwise, no client at all**, plus one clear message naming
   `cargo install --path .`. Never start a client that will fail to spawn.

### Initialization options

The client sends `{}` — nothing at all — when its threshold setting is unset.
It sends `{"threshold": N}` only when the user has explicitly set one.

This is a deliberate departure from jscx, and it is the single most important
decision in this design. jscx has no configuration file, so its clients each
carry a `DEFAULT_THRESHOLD = 11` constant and send it unconditionally. rustcx
*does* have a configuration file, and `Server::settings` resolves
`initializationOptions` **over** `rustcx.toml`:

```rust
fn settings(&self) -> FileConfig {
    self.init.or(self.config.config())
}
```

Porting jscx's behavior verbatim would therefore make every client permanently
override `rustcx.toml`'s `threshold` with 11, silently killing that key in
every editor. Sending nothing keeps the precedence chain intact:
explicit editor setting → `rustcx.toml` → `DEFAULT_THRESHOLD`.

`DEFAULT_THRESHOLD = 11` is defined once, in `src/lsp.rs`. No client redefines
it, and no client's README states it as its own default.

### Which files attach

`.rs` files only, expressed with each editor's native filetype mechanism.

This is simpler than jscx, which needs a client-side filter — `files.lua` in
Neovim, `isSupportedByJscx` in Kotlin — because its CLI skips `.d.ts`
declaration files while its server scores whatever text arrives, so without a
filter the editor would report complexity the command line never shows.

rustcx has no such asymmetry. `--no-tests` is a scoring option the server reads
from `rustcx.toml` on both the CLI and the LSP path, not a filename rule. The
filter module is therefore **not ported**; a filetype check replaces it.

### What stays in the server

The default threshold; the scoring options `question`, `logical`, and `tests`;
`rustcx.toml` discovery and its mtime reload. Clients know about none of it.

## Settings surface

Two settings per client, and only two:

| Setting | Default | Meaning |
| --- | --- | --- |
| threshold | unset | Warn at or above this score, error at or above twice it. Unset means `rustcx.toml`, then 11. |
| binary path | unset | Blank resolves per the rules above. |

The scoring options are deliberately absent from every editor UI. They belong
in `rustcx.toml` because they must agree with CI: if an editor scores a
function differently from `rustcx --max` in a build script, the tool has lied.
Threshold and binary path are the two things that are legitimately per-person
and per-machine, which is exactly what a config file committed to a repository
cannot hold.

`max` is a CI gate with no meaning in an editor and is not exposed anywhere.

Neovim is the exception to "two settings": it has no settings UI, so both knobs
are ordinary `vim.lsp.config` keys — `init_options.threshold` and `cmd`. The
resolver still runs, and `cmd` is the override. Zed's threshold likewise comes
from `LspSettings`, and its binary override is a matter of `PATH`; only the
JetBrains and VS Code targets present an explicit binary-path control.

## Non-goals

- **`workspace/didChangeConfiguration`.** The server reads
  `initializationOptions` once, at `initialize`, so changing an editor's
  threshold requires an LSP restart. Supporting configuration changes would
  mean modifying `src/lsp.rs`, and `rustcx.toml` already hot-reloads on mtime
  change — live tuning has a supported path today. Each client's README
  documents its restart command instead.
- **Prebuilt binaries and release CI.** Every install path is
  `cargo install --path .`. Publishing to the Zed or VS Code marketplaces would
  require cross-compiled releases, checksums, version pinning, and an update
  path; that is its own spec, if it is ever wanted.
- **Any LSP feature beyond what the server advertises.** No code actions, no
  commands, no status bar items. The clients add nothing the server cannot
  already answer.

## Layout

```
extensions/
  nvim/       lsp/rustcx.lua · lua/rustcx/binary.lua · test/rustcx_spec.lua · README.md
  zed/        extension.toml · Cargo.toml · src/lib.rs · README.md
  jetbrains/  build.gradle.kts · settings.gradle.kts · gradle.properties
              gradlew · gradlew.bat · gradle/wrapper/
              src/main/kotlin/dev/rustcx/intellij/*.kt
              src/main/resources/META-INF/plugin.xml
              src/test/kotlin/dev/rustcx/intellij/RustcxBinaryTest.kt · README.md
  vscode/     package.json · tsconfig.json · src/extension.ts · README.md
```

jscx names its Zed directory `extensions/jscx`, which reads as "the extension"
beside siblings named for their editors. This uses `extensions/zed/`.

The nested `Cargo.toml` under `extensions/zed/` is safe: the root manifest has
no `[workspace]` section, and cargo globs only `src/`. This is the same
arrangement jscx already runs.

The four targets have no dependencies on each other and can be implemented in
any order.

## Target: Neovim

Requires Neovim 0.11 or newer, for `vim.lsp.config` and runtimepath-discovered
`lsp/` configs.

`lsp/rustcx.lua` returns a table with `cmd`, `filetypes = { "rust" }`, and a
`root_dir` predicate over the markers `Cargo.toml` and `.git`. A predicate
rather than plain `root_markers` for one reason: declining to call `on_dir` is
the only hook Neovim offers for "do not attach to this buffer", which is what a
missing binary needs. No `init_options` key is present unless the user sets
one.

`lua/rustcx/binary.lua` is jscx's resolver, with its dependency injection kept —
`exepath`, `executable`, `home`, and `windows` are injectable, which is what
makes every branch testable without touching a real filesystem. It returns
`nil` rather than an unchecked path, so a missing binary produces one warning
and no client instead of a client that fails to spawn.

jscx's `lua/jscx/files.lua` is not ported, per the contract above.

Configuration, in `init.lua`:

```lua
vim.lsp.config("rustcx", { init_options = { threshold = 15 } })
```

Restart after changing it with `:LspRestart rustcx`.

## Target: Zed

`extension.toml` declares one language server for `languages = ["Rust"]`; Zed
runs it alongside rust-analyzer. `Cargo.toml` is a `cdylib` against
`zed_extension_api`, built for `wasm32-unknown-unknown`.

`src/lib.rs` implements two methods:

- `language_server_command` — resolution per the contract. This adds the
  `~/.cargo/bin` fallback that jscx's Zed extension lacks entirely; it calls
  only `worktree.which()`, so a Zed launched from a desktop entry cannot find a
  cargo-installed binary.
- `language_server_initialization_options` — returns `Ok(None)` when no
  threshold is configured, rather than jscx's hardcoded `11`.

## Target: JetBrains

Requires a paid JetBrains IDE, 2026.2 or newer. The IntelliJ LSP API does not
exist in IDEA Community or Android Studio, and the `com.intellij.modules.lsp`
dependency in `plugin.xml` is what keeps the plugin from being installable
where it cannot work.

Six Kotlin files, ported name-for-name from jscx under `dev.rustcx.intellij`:

| File | Role |
| --- | --- |
| `RustcxBinary.kt` | Resolution, with `pathLookup` and `home` injected for tests |
| `RustcxSettings.kt` | Persisted application-level state: threshold, binary path |
| `RustcxConfigurable.kt` | The *Settings \| Tools \| rustcx* panel |
| `RustcxNotifications.kt` | The balloon shown when the binary cannot be found |
| `RustcxLspClientDescriptor.kt` | Command line and initialization options |
| `RustcxLspIntegrationProvider.kt` | Decides which files start the server |

`build.gradle.kts` targets `intellijIdeaUltimate` at `platformVersion`
2026.2 — jscx compiles against WebStorm, which is the smallest paid IDE with
JavaScript support; IDEA Ultimate is the equivalent choice for Rust and is what
is installed here. `pluginSinceBuild = 262`, `untilBuild` open-ended, Kotlin
`jvmToolchain(21)`, and `kotlin.stdlib.default.dependency = false` because the
platform bundles its own stdlib and a second copy is a load error.

The Gradle wrapper — `gradlew`, `gradlew.bat`, and `gradle/wrapper/` including
the jar — is copied from jscx at 9.5.0 and **committed**. `gradle` is not
installed on this machine; the wrapper is what makes the build runnable at all.
Many boilerplate `.gitignore` files exclude `gradle-wrapper.jar`; ours must not.

**One item to confirm during implementation.** jscx's descriptor declares no
feature support beyond diagnostics, with the comment that the platform client
gates its requests on the server's advertised capabilities and its server
advertises only `textDocumentSync`. rustcx additionally advertises
`hoverProvider`. Whether `ProjectWideLspClientDescriptor` picks hover up from
the capabilities automatically or needs an explicit override must be checked
against the 2026.2 API, and hover verified in a running IDE before the plugin
is called done.

## Target: VS Code

The only target with no jscx precedent to port; it is designed here.

A TypeScript extension using `vscode-languageclient/node`, activating on
`onLanguage:rust`. `package.json` contributes two configuration keys:

| Key | Type | Default |
| --- | --- | --- |
| `rustcx.threshold` | `number \| null` | `null` |
| `rustcx.path` | `string` | `""` |

`null` rather than `11`, per the contract — the extension must be able to tell
"unset" from "set to 11", because only the former lets `rustcx.toml` apply.

`src/extension.ts` resolves the binary, starts the client with a document
selector limited to Rust files, and registers an
`onDidChangeConfiguration` listener that restarts the client when either key
changes, since the server reads options only at `initialize`.

Resolution needs its own implementation of the contract: Node's
`process.env.PATH` walk plus `os.homedir()`, with `fs.accessSync(…, X_OK)` for
the executable check.

## Testing

| Target | What runs here | What does not |
| --- | --- | --- |
| Neovim | `nvim -l extensions/nvim/test/rustcx_spec.lua` for every resolver branch, then a real attach against `fixtures/patterns.rs` confirming a warning and a hover | — |
| VS Code | `npm run compile`, package, `code --install-extension`, then the same live check | — |
| Zed | `cargo build --target wasm32-unknown-unknown --release` | Zed is not installed; no runtime check |
| JetBrains | `./gradlew test` on the resolver tests | `verifyPlugin` and `runIde` pull a multi-GB IntelliJ platform; the installable zip is built by hand when wanted |

`fixtures/patterns.rs` is the shared manual check: it holds `describe` at
complexity 8, which warns at a threshold of 8 and errors at 4.

## Documentation

The README's "In your editor" section is rewritten to point at the four
`extensions/*/README.md` files, replacing the hand-written `vim.lsp.config`
snippet and the "VS Code and Zed … None is shipped here" paragraph. The Helix
snippet stays: it needs no extension.

Each extension README covers install, the two settings, the restart command
after a settings change, and how to tell the client attached.

`.gitignore` grows the Gradle entries (`.gradle/`, `.kotlin/`,
`local.properties`, `.intellijPlatform/`, `build/`), Node entries
(`node_modules/`, `dist/`), and `extensions/*/extension.wasm`, along with
jscx's comment warning that the wrapper jar is deliberately not ignored.
