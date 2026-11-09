# cmplx for Zed

Cyclomatic complexity diagnostics and hover from `cmplx --lsp` for JavaScript, TypeScript, TSX
and Rust, running alongside rust-analyzer.

## Requirements

- The `cmplx` binary: `cargo install --path ../../crates/cmplx`
- The `wasm32-wasip2` target, to build the extension:
  `rustup target add wasm32-wasip2`

## Install

Zed builds the extension itself. *Extensions → Install Dev Extension*, and
choose this directory. Zed rebuilds it whenever you reload the extension.

## Configure

By default this extension sends no settings, so `cmplx.toml` governs —
including its `threshold` key. To override the threshold in Zed's settings:

```json
{
  "lsp": {
    "cmplx": {
      "initialization_options": {},
      "settings": { "threshold": 15 }
    }
  }
}
```

A function scoring at or above the threshold is a warning; at or above twice
it, an error. The server reads it once at startup, so restart the language
server after changing it. Editing `cmplx.toml` needs no restart.

The binary is found on the worktree's `PATH`, falling back to
`~/.cargo/bin/cmplx`.

## Check it works

Open `crates/cmplx-rust/fixtures/patterns.rs` with `threshold` set to 8. `describe` on line 16
scores 8, so a working setup shows a warning there and hover explains the
score.
