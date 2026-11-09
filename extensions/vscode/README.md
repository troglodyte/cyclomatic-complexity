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

Open `crates/cmplx-rust/fixtures/patterns.rs` from the repository root with `rustcx.threshold` set
to 8. `describe` on line 16 scores 8, so a working setup shows a warning there
and explains the score on hover. *Output → rustcx* carries the server log.

## Tests

```console
npm test
```

Covers binary resolution, which is the only part of this extension that can run
outside a VS Code process.
