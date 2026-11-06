# jscx for Neovim

Cyclomatic complexity diagnostics from `jscx --lsp`. Requires Neovim 0.11 or
newer and the `jscx` binary:

```console
cargo install --path .
```

## Install

Two edits to your `init.lua`. First, put this directory on the runtimepath. With
lazy.nvim, add it to your plugin spec list:

```lua
-- Eager on purpose: vim.lsp.enable below runs at startup and needs
-- lsp/jscx.lua on the runtimepath already.
{ dir = "~/code/jscx/extensions/nvim" },
```

Adjust that path to wherever you cloned jscx. Then enable the server, alongside
whatever else you run:

```lua
vim.lsp.enable({ "ts_ls", "rust_analyzer", "jscx" })
```

`vim.lsp.enable("jscx")` on its own works too, if you enable servers one at a
time.

Do not lazy-load the spec on `ft`, `event`, or `cmd`. `vim.lsp.enable` runs at
startup and needs `lsp/jscx.lua` on the runtimepath already; a lazy trigger adds
the directory later and the config is silently not found — with no error, because
a config that was never discovered looks the same as one that was never asked
for.

Without a plugin manager, prepend the directory yourself. `runtimepath` does not
expand `~`, so pass it through `expand`:

```lua
vim.opt.runtimepath:prepend(vim.fn.expand("~/code/jscx/extensions/nvim"))
vim.lsp.enable("jscx")
```

To confirm the config was found, `:lua =vim.lsp.config.jscx` should print a table
rather than `nil`. `:LspInfo` then shows whether the client attached to the
current buffer, and `:LspLog` shows a spawn failure if the binary could not be
run.

## Configure

One knob. A function scoring at or above the threshold is a warning; at or above
twice it, an error. The default is 11, so 11 through 21 warn and 22 or above is
an error.

```lua
vim.lsp.config("jscx", { init_options = { threshold = 15 } })
```

The server reads the threshold once, at `initialize`, so run `:LspRestart jscx`
after changing it.

The binary is found on `PATH`, falling back to `~/.cargo/bin/jscx` — that
fallback matters because a GUI frontend launched from the Dock does not inherit
your shell's `PATH`. To point somewhere else:

```lua
vim.lsp.config("jscx", { cmd = { "/path/to/jscx", "--lsp" } })
```

Diagnostics arrive through `vim.diagnostic`, so your existing display settings
apply. Attaches to `javascript`, `javascriptreact`, `typescript`, and
`typescriptreact` buffers, skipping declaration files to match the CLI.

## Check it works

Open `test.ts` from the repository root. It holds functions that are clean, over
the threshold, and over double it, so a working setup shows both a warning and an
error.

## Tests

```console
nvim -l extensions/nvim/test/jscx_spec.lua
```
