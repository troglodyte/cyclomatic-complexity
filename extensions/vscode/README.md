# cmplx for VS Code

Cyclomatic complexity diagnostics and hover from `cmplx --lsp` for JavaScript, TypeScript (and React variants) and Rust.

Saved files and unsaved (`untitled:`) buffers are both sent to the server. The
server picks the language from the extension in the document's URI, so an
unsaved buffer is scored when its URI carries one (as `code new.rs` gives a file
that does not exist yet); a plain *Untitled-1* buffer gets no diagnostics.

## Requirements

- The `cmplx` binary: `cargo install --path ../../crates/cmplx`
- Node and npm, to build the extension

## Build and install

```console
npm ci
npx @vscode/vsce package --allow-missing-repository --skip-license
code --install-extension cmplx-0.1.0.vsix --force
```

## Configure

*Settings → Extensions → cmplx*

| Setting | Default | Meaning |
| --- | --- | --- |
| `cmplx.threshold` | `null` | Warning at or above this score, error at or above twice it. `null` hands the decision to `cmplx.toml`. |
| `cmplx.path` | `""` | Blank resolves from `PATH`, then `~/.cargo/bin/cmplx` |

Changing either setting restarts the language server automatically, because the
server reads its options only during `initialize`. Editing `cmplx.toml` needs
no restart — the server re-reads it when its modification time changes.

## Check it works

Open `crates/cmplx-rust/fixtures/patterns.rs` from the repository root with `cmplx.threshold` set
to 8. `describe` on line 16 scores 8, so a working setup shows a warning there
and explains the score on hover. *Output → cmplx* carries the server log.

## Tests

```console
npm test
```

Covers binary resolution and the document selector, the parts of this
extension that can run outside a VS Code process.
