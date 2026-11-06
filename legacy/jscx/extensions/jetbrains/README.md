# jscx JetBrains Plugin

Cyclomatic complexity diagnostics for JavaScript, TypeScript, and JSX in WebStorm,
IntelliJ IDEA Ultimate, and the other paid JetBrains IDEs.

The plugin is a thin LSP client. It resolves the `jscx` binary, runs `jscx --lsp`,
and passes the threshold as an initialization option — all scoring lives in the
Rust server, shared with the Zed extension.

## Requirements

- The `jscx` binary: `cargo install --path ../..`
- Gradle, to build the plugin
- A paid JetBrains IDE, 2026.2 or newer. The IntelliJ LSP API does not exist in
  IDEA Community or Android Studio. See `pluginSinceBuild` in `gradle.properties`
  for what it would take to support older IDEs.

## Build

```console
gradle wrapper        # first time only; or open this directory in your IDE
./gradlew buildPlugin
```

Install the resulting zip — not either jar inside it — with
*Settings | Plugins | ⚙ | Install Plugin from Disk*, then restart the IDE:

```text
build/distributions/jscx-intellij-0.1.0.zip
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

*Settings | Tools | jscx*

| Setting | Default | Meaning |
| --- | --- | --- |
| Complexity threshold | 11 | Warning at or above this score, error at or above twice it |
| jscx binary | blank | Blank resolves from `PATH`, then `~/.cargo/bin/jscx` |

Changing either setting restarts the language server, because the server reads the
threshold only during `initialize`.

## Layout

| File | Role |
| --- | --- |
| `JscxLspIntegrationProvider.kt` | Decides which files start the server |
| `JscxLspClientDescriptor.kt` | Builds the command line and initialization options |
| `JscxBinary.kt` | Resolves the executable |
| `JscxSettings.kt` | Persisted threshold and binary path |
| `JscxConfigurable.kt` | Settings panel, restarts servers on apply |
| `JscxNotifications.kt` | Warns once per project when the binary is missing |

The `platformVersion` and `pluginSinceBuild` values live in `gradle.properties`.
