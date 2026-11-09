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

Open `crates/cmplx-rust/fixtures/patterns.rs` from the repository root with the threshold set to
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
