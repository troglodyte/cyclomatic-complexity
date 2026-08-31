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
