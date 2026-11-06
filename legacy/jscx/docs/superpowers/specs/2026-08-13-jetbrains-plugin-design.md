# JetBrains plugin for jscx

Bring the complexity diagnostics that already work in Zed to WebStorm and IntelliJ IDEA Ultimate,
and raise the default threshold from 10 to 11 everywhere it is configured.

## Goals

- A JetBrains plugin that reports cyclomatic complexity as editor warnings and errors,
  driven by the existing `jscx --lsp` server rather than a second analysis implementation.
- One configurable threshold, defaulting to 11, consistent across the CLI's siblings:
  the LSP server, the Zed extension, and the new plugin.
- Installable from a locally built zip. No marketplace publishing, signing, or release pipeline.

## Non-goals

- IntelliJ IDEA Community Edition and Android Studio support.
  The IntelliJ Platform LSP API is unavailable in unpaid IDEs, and Community has no
  JavaScript or TypeScript support to annotate in the first place.
- Exposing the CLI's `--optional-chain`, `--no-logical`, `--min`, or `--top` flags in either editor.
  The LSP server hardcodes `Options::default()` and stays that way.
- Quick fixes, intentions, gutter icons, or inspection suppression comments.

## Decisions

| Question | Decision | Why |
| --- | --- | --- |
| Target IDEs | WebStorm, IDEA Ultimate, and other paid IDEs | Where the platform LSP API exists |
| Integration | Thin LSP client wrapper around `jscx --lsp` | ~150 lines, no duplicated scoring logic |
| Distribution | Locally built zip, installed from disk | Mirrors the Zed dev-extension flow |
| Binary lookup | `PATH` with a settings override and a `~/.cargo/bin` fallback | GUI-launched IDEs do not inherit shell `PATH` |
| Threshold default | 11, in the LSP server, Zed extension, and plugin | One number, three clients |

The alternative considered was a native `ExternalAnnotator` shelling out to `jscx --json -`.
It would work in every IDE, apply settings changes without a restart, and could expose the
scoring flags as inspection options.
It was rejected because it costs roughly 350 lines, duplicates the threshold-to-severity
mapping that `src/lsp.rs` already owns, and its main advantage — Community Edition support —
is worth little for a JavaScript and TypeScript tool.

## Architecture

```text
WebStorm / IDEA Ultimate
┌──────────────────────────────────────────────┐
│  jscx plugin  (Kotlin)                       │
│   JscxLspServerSupportProvider   ← EP hook   │
│   JscxLspServerDescriptor        → command   │
│   JscxSettings                   threshold   │
│   JscxConfigurable               Settings UI │
├──────────────────────────────────────────────┤
│  platform LSP client  (JetBrains-owned)      │
└───────────────────────┬──────────────────────┘
                        │ stdio JSON-RPC
                        ▼
              jscx --lsp   ← existing src/lsp.rs
```

The plugin owns no analysis logic.
It resolves a binary, launches it, and passes the threshold as `initializationOptions`.
Scoring and severity remain in `src/lsp.rs`, shared with the Zed extension.

Platform requirements, confirmed against the JetBrains LSP documentation:

- `<depends>com.intellij.modules.lsp</depends>` in `plugin.xml`
- Extension point `com.intellij.platform.lsp.serverSupportProvider`
- Platform 2023.2 or later

## Components

### JscxLspServerSupportProvider

Implements `fileOpened(project, file, serverStarter)`.
Starts the server when the file extension is one of
`js`, `jsx`, `mjs`, `cjs`, `ts`, `tsx`, `mts`, `cts`, excluding any name ending in `.d.ts`.

The `.d.ts` exclusion is load-bearing.
The CLI skips declaration files unless `--include-dts` is passed, but `src/lsp.rs` analyzes
whatever text arrives on the wire.
Without a client-side filter, JetBrains would report complexity that `jscx` on the command line
never shows for the same file.

### JscxLspServerDescriptor

Extends `ProjectWideLspServerDescriptor(project, "jscx")`.

- `isSupportedFile(file)` applies the same predicate as the provider.
- `createCommandLine()` returns `GeneralCommandLine(resolvedBinary, "--lsp")`.
- `createInitializationOptions()` returns `{"threshold": settings.threshold}`.
- Completion, hover, go-to-definition, and formatting support are explicitly disabled.

Disabling those capabilities is a correctness requirement, not tidiness.
`src/lsp.rs` ignores unrecognized methods without sending an error response, so any request
the platform issues for an unsupported feature would go unanswered and leave the client waiting.

### JscxSettings

An application-level `PersistentStateComponent` holding two fields:

| Field | Default | Meaning |
| --- | --- | --- |
| `threshold` | `11` | Warning at or above this score, error at or above double it |
| `binaryPath` | `""` | Empty means resolve `jscx` from `PATH` |

Application scope rather than project scope, matching how the same setting is written once in
Zed's user settings.

### JscxConfigurable

A *Settings | Tools | jscx* panel with a threshold spinner (minimum 1) and a path field.
On `apply()` it persists state, then calls
`LspServerManager.getInstance(project).stopAndRestartIfNeeded(JscxLspServerSupportProvider::class.java)`
for every open project.

The restart is required because the server reads the threshold once, during `initialize`
(`src/lsp.rs:49-55`), and implements no `workspace/didChangeConfiguration` handler.

### Binary resolution

Resolution order:

1. `binaryPath` from settings, if non-blank and executable
2. `PathEnvironmentVariableUtil.findInPath("jscx")`
3. `~/.cargo/bin/jscx`

If all three fail, the descriptor reports a clear error and the plugin shows a notification with
an action that opens the settings panel.
Step 3 exists because an IDE launched from the macOS Dock does not inherit the shell `PATH`,
which is the most likely way this integration breaks for a `cargo install` user.

## Data flow

1. A supported file opens. `fileOpened` starts `jscx --lsp` for the project.
2. `initialize` carries `initializationOptions.threshold`; the server stores it.
3. Each edit sends a full-text `didChange`, since the server advertises sync mode `1`.
4. The server re-analyzes and replies with `textDocument/publishDiagnostics`.
5. A settings change restarts the server, which picks up the new threshold at `initialize`.

With the default threshold of 11, scores of 11 through 21 render as warnings and 22 or above
as errors.

## Changes outside the plugin

| File | Change |
| --- | --- |
| `src/lsp.rs:22` | Threshold default `10` becomes `11` |
| `extensions/jscx/src/lib.rs:35` | `unwrap_or(10)` becomes `unwrap_or(11)` |
| `src/lsp.rs:124-128` | Diagnostic range ends at the line's real length, not a hardcoded `100` |
| `src/lsp.rs:32` | `Content-Length` parse no longer panics on a malformed header |
| `src/lsp.rs:65` | `msg.params.unwrap()` no longer panics on a notification without params |
| `src/lsp.rs:10` | `method` is omitted from responses instead of serialized as `null`, which a JSON-RPC Response must not carry |
| `README.md` | Threshold documentation updated, JetBrains installation section added |
| `.gitignore` | Add `.gradle/`; the existing unanchored `build/` rule already covers the Gradle output |
| `tools/colors.sh` | New; output helpers for the build handoff script |

The CLI's `--min` default of 1 and `--max` (no default) are unchanged.
They express different ideas than the editor threshold and changing them would alter exit codes
for existing users.

The two `.unwrap()` calls matter more now than they did with a single client.
A panic there kills the process, and the JetBrains client surfaces that to the user as a crashed
language server.

## Error handling

| Failure | Behavior |
| --- | --- |
| Binary not found | Notification with a "Configure" action; no server started |
| Binary not executable | Same notification, with the resolved path in the message |
| Server exits unexpectedly | Platform LSP client reports it; the plugin adds no retry logic |
| Malformed JSON-RPC frame | Server skips the frame instead of panicking |
| Document with no functions over threshold | Empty `diagnostics` array published, clearing stale markers |

## Testing

`src/lsp.rs` has no tests and its logic is currently welded to stdin.
Extract a pure function, `diagnostics_for(text: &str, uri: &str, threshold: u32) -> Vec<Value>`,
and drive the following cases test-first:

- Default threshold is 11 when `initializationOptions` omits it
- A function scoring exactly 11 is a warning; exactly 22 is an error; 10 produces nothing
- Diagnostic range ends at the reported line's actual length
- A `.d.ts` document analyzed directly still returns rows, confirming the filter is the client's job
- A malformed frame does not panic

Kotlin-side coverage:

- Unit tests for binary resolution against a fake `PATH` and a fake home directory
- `./gradlew verifyPlugin` in CI-equivalent form
- Manual `./gradlew runIde` smoke test for the LSP wiring

Full `LspServerManager` integration tests need a heavy test fixture and are deliberately out of
scope; the `runIde` smoke test covers the wiring instead.

## Build and install

Built and verified with these pins, each forced by a compatibility constraint rather than chosen:

| Component | Version | Constraint |
| --- | --- | --- |
| IntelliJ Platform Gradle Plugin | 2.18.1 | Requires Gradle 9.0.0 or newer since 2.12.0 |
| Gradle (wrapper) | 9.5.0 | Kotlin 2.4.10's tested ceiling; the wrapper pins it so `PATH` does not decide |
| Kotlin | 2.4.10 | First Kotlin release supporting Gradle 9.x |
| foojay-resolver-convention | 1.0.0 | Earlier versions reference `JvmVendorSpec.IBM_SEMERU`, removed in Gradle 9 |
| WebStorm SDK | 2026.2.1 | Current release; `pluginSinceBuild` 262 follows from the LSP API rename |

The LSP API was renamed in 2026.2 and the plugin uses the current names, which is what sets the
2026.2 floor:

| Pre-rename | Current |
| --- | --- |
| `LspServerSupportProvider` | `LspIntegrationProvider` |
| `LspServerStarter.ensureServerStarted` | `LspClientStarter.ensureClientStarted` |
| `ProjectWideLspServerDescriptor` | `ProjectWideLspClientDescriptor` |
| `LspServerManager.stopAndRestartIfNeeded` | `LspClientManager.stopAndRestartClientsIfNeeded` |
| EP `platform.lsp.serverSupportProvider` | EP `platform.lsp.integrationProvider` |

Both extension points are still registered in 2026.2, so reverting to the pre-rename names is a
mechanical swap if support for 2023.2–2026.1 is ever needed.

Two class locations worth recording, since both differ from what the documentation implies:
`PathEnvironmentVariableUtil` lives in `com.intellij.execution.configurations`, not
`com.intellij.util`; and `TestFrameworkType.Platform` must be left out of the dependencies, because
it registers `com.intellij.tests.JUnit5TestSessionListener`, which cannot start outside a real
platform test run. Plain JUnit 5 is used instead.

```console
cd extensions/jetbrains
./gradlew buildPlugin
```

The result lands in `build/distributions/`.
Install it through *Settings | Plugins | ⚙ | Install Plugin from Disk*, then set the threshold
under *Settings | Tools | jscx*.

## Repository layout

```text
extensions/jetbrains/
├─ build.gradle.kts
├─ settings.gradle.kts
├─ gradle.properties
├─ src/main/kotlin/dev/jscx/intellij/
│  ├─ JscxLspServerSupportProvider.kt
│  ├─ JscxLspServerDescriptor.kt
│  ├─ JscxBinary.kt
│  ├─ JscxSettings.kt
│  └─ JscxConfigurable.kt
├─ src/main/resources/META-INF/plugin.xml
└─ src/test/kotlin/dev/jscx/intellij/
   └─ JscxBinaryTest.kt
```
