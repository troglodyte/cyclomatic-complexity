# Neovim plugin for jscx

Bring the complexity diagnostics that already work in Zed and the JetBrains IDEs to Neovim,
driven by the same `jscx --lsp` server.

## Goals

- A Neovim integration that reports cyclomatic complexity as buffer diagnostics, using the
  existing LSP server rather than a third analysis implementation.
- The same threshold, defaulting to 11, as the CLI's other clients.
- Loadable from a directory in this repository. No plugin-manager publishing, no mason
  registry entry, no release pipeline.

## Non-goals

- Neovim 0.10 and earlier. `vim.lsp.config` and `lsp/` runtimepath discovery arrived in
  0.11, and supporting the older `nvim-lspconfig` setup path would mean shipping two
  configurations of the same twelve lines.
- A `require("jscx").setup()` entry point. `vim.lsp.config` already merges user overrides
  over a shipped default; a `setup()` would be a second way to write the same table.
- Exposing the CLI's `--optional-chain`, `--no-logical`, `--min`, or `--top` flags. The LSP
  server hardcodes `Options::default()` and stays that way, as it does for the other two
  editors.
- Statusline components, a `:JscxToggle` command, virtual-text formatting, or any other
  presentation choice. Diagnostics land in `vim.diagnostic` and the user's existing
  configuration renders them.

## Decisions

| Question | Decision | Why |
| --- | --- | --- |
| Minimum version | Neovim 0.11 | Where `lsp/` discovery and `vim.lsp.config` exist |
| Distribution | In-repo directory, loaded off disk | Mirrors Zed's dev extension and JetBrains' install-from-zip |
| Config surface | `vim.lsp.config("jscx", ...)` override | Native to 0.11+; no API of our own to keep in sync |
| Binary lookup | `PATH`, then `~/.cargo/bin` | GUI frontends do not inherit the shell's `PATH` |
| Threshold default | 11 | Fourth client, same number |
| Declaration files | Excluded client-side | Matches the CLI and the JetBrains plugin |

The alternative considered was a `nvim-lint` linter definition shelling out to `jscx --json -`.
It would work on Neovim 0.10, and `nvim-lint` is a common dependency already present in many
configurations. It was rejected because it duplicates the threshold-to-severity mapping that
`src/lsp.rs` owns, re-runs the binary per lint trigger instead of holding one process open,
and makes `nvim-lint` a hard prerequisite where the LSP path has none.

## Architecture

```text
Neovim 0.11+
┌──────────────────────────────────────────────┐
│  extensions/nvim  (Lua)                      │
│   lsp/jscx.lua          ← rtp discovery      │
│   lua/jscx/binary.lua     resolve()          │
│   lua/jscx/files.lua      is_scored()        │
├──────────────────────────────────────────────┤
│  vim.lsp  client  (Neovim-owned)             │
└───────────────────────┬──────────────────────┘
                        │ stdio JSON-RPC
                        ▼
              jscx --lsp   ← existing src/lsp.rs
```

The integration owns no analysis logic. It resolves a binary, decides which buffers are
eligible, and passes the threshold as `initializationOptions`. Scoring and severity remain in
`src/lsp.rs`, shared with the Zed extension and the JetBrains plugin.

Neovim requirements, confirmed against `:help lsp-config`:

- A file at `lsp/<name>.lua` anywhere on the runtimepath returns the base config for `<name>`.
- `vim.lsp.enable("jscx")` arms it; the client starts when a buffer matches `filetypes`.
- `root_dir` may be `fun(bufnr, on_dir)`. Returning without calling `on_dir` prevents the
  client from attaching to that buffer.

## Components

### lsp/jscx.lua

The whole configuration, declarative apart from the `root_dir` predicate.

```lua
local binary = require("jscx.binary")
local files = require("jscx.files")

-- Keep in sync with jscx::lsp::DEFAULT_THRESHOLD.
local DEFAULT_THRESHOLD = 11
local MARKERS = { "package.json", "tsconfig.json", "jsconfig.json", ".git" }
local NOT_FOUND = table.concat({
  "jscx not found on PATH or in ~/.cargo/bin. Install it with",
  "'cargo install --path .', or point at it with",
  'vim.lsp.config("jscx", { cmd = { "/path/to/jscx", "--lsp" } }).',
}, "\n")

local exe = binary.resolve()
if not exe then
  vim.notify(NOT_FOUND, vim.log.levels.WARN)
end

return {
  cmd = { exe or "jscx", "--lsp" },
  filetypes = { "javascript", "javascriptreact", "typescript", "typescriptreact" },
  init_options = { threshold = DEFAULT_THRESHOLD },
  root_dir = function(bufnr, on_dir)
    if not exe or not files.is_scored(vim.api.nvim_buf_get_name(bufnr)) then
      return
    end
    on_dir(vim.fs.root(bufnr, MARKERS) or vim.fn.getcwd())
  end,
}
```

Those four filetypes cover all eight extensions the other clients accept: `.mjs` and `.cjs`
are `javascript`, `.mts` and `.cts` are `typescript`, `.jsx` is `javascriptreact`, and `.tsx`
is `typescriptreact`. No filetype work is needed for JSX lexing — the server decides that from
the document URI's extension, in `is_jsx_uri`.

`root_dir` is a function rather than a `root_markers` list because it does double duty. Both
the declaration-file exclusion and the missing-binary case are expressed by declining to call
`on_dir`, which is the only hook Neovim offers for "do not attach to this buffer". The
`getcwd()` fallback keeps a single-file edit outside any project working, where a marker
search finds nothing.

### lua/jscx/binary.lua

`resolve(opts)` returns an absolute path or `nil`, in this order:

1. `vim.fn.exepath("jscx")`, if non-empty
2. `~/.cargo/bin/jscx`, or `jscx.exe` on Windows, if executable

`nil` rather than an unchecked path, so a missing binary leads to one warning and no client
instead of a client that fails to spawn. `resolve` itself stays pure and silent — the warning
is `lsp/jscx.lua`'s, raised once when the config is first evaluated, which keeps every
resolution branch testable without capturing notifications.

Step 2 exists for the same reason it does in the JetBrains plugin: a GUI frontend such as
Neovide or VimR launched from the Dock does not inherit the shell's `PATH`, so `~/.cargo/bin`
is routinely invisible even though the binary is installed and works in a terminal.

`opts` carries injected `exepath`, `executable`, `home`, and `windows` for testing, defaulting
to the real ones.

### lua/jscx/files.lua

`is_scored(name)` returns whether jscx should score a buffer, applying the same predicate as
`isSupportedByJscx` in the JetBrains plugin: extension in `js`, `jsx`, `mjs`, `cjs`, `ts`,
`tsx`, `mts`, `cts`, excluding any name ending in `.d.ts`, `.d.mts`, or `.d.cts`.

The declaration-file exclusion is load-bearing. The CLI skips those files unless
`--include-dts` is passed, but `src/lsp.rs` analyzes whatever text arrives on the wire.
Without a client-side filter, Neovim would report complexity that `jscx` on the command line
never shows for the same file.

The extension check is not redundant with `filetypes`. A buffer can carry the `typescript`
filetype with a name that no extension matches — a `:set filetype` by hand, a scratch buffer —
and the extension list is what keeps the two clients' answers identical.

## Configuration

Overriding the threshold is a merge over the shipped table:

```lua
vim.lsp.config("jscx", { init_options = { threshold = 15 } })
```

The server reads the threshold once, during `initialize`, and implements no
`workspace/didChangeConfiguration` handler, so a change needs `:LspRestart jscx`. This matches
Zed, where the equivalent is `editor: restart language server`, and differs from the JetBrains
plugin, which restarts the server itself on `apply()`. The README says so, because a silently
ignored setting is the most likely way a user concludes the integration is broken.

An explicit binary path is the same mechanism:

```lua
vim.lsp.config("jscx", { cmd = { "/opt/bin/jscx", "--lsp" } })
```

No capabilities beyond diagnostics are declared. The server advertises only `textDocumentSync`
in its `initialize` response and ignores methods it does not recognize without replying, so a
request for an unsupported feature would never be answered. Neovim gates its requests on
advertised capabilities, so nothing further is required — but the reasoning is worth recording
alongside the explicit disabling the JetBrains descriptor needs.

## Data flow

1. A buffer with a matching filetype opens. `root_dir` accepts it and Neovim starts
   `jscx --lsp` for the resolved project root.
2. `initialize` carries `initializationOptions.threshold`; the server stores it.
3. Each edit sends a full-text `didChange`, since the server advertises sync mode `1`.
4. The server re-analyzes and replies with `textDocument/publishDiagnostics`, which
   `vim.diagnostic` renders using the user's existing configuration.
5. `:LspRestart jscx` picks up a changed threshold at the next `initialize`.

With the default threshold of 11, scores of 11 through 21 render as warnings and 22 or above
as errors.

## Error handling

| Failure | Behavior |
| --- | --- |
| Binary on neither `PATH` nor `~/.cargo/bin` | One `vim.notify` at `WARN` naming both fixes; no client started |
| Buffer is a declaration file | No client attached to that buffer; others unaffected |
| Buffer outside any project | Root falls back to `getcwd()`; the client starts normally |
| Server exits unexpectedly | `vim.lsp` reports it; the integration adds no retry logic |
| Document with no functions over threshold | Empty `diagnostics` array published, clearing stale markers |

The not-found message, shown in full in the `lsp/jscx.lua` listing above, names both remedies —
installing the binary and pointing at an existing one — since either may be the right fix.

## Testing

`nvim -l extensions/nvim/test/jscx_spec.lua` runs headless with no plugin manager and no
plenary checkout, prints one line per assertion, and exits non-zero on failure. Written
test-first, covering the two modules that branch:

- `resolve` returns the `exepath` hit when `PATH` has one
- `resolve` falls back to `~/.cargo/bin/jscx` when `exepath` is empty
- `resolve` returns `nil` when neither exists
- `resolve` looks for `jscx.exe` in the Cargo fallback on Windows
- `resolve` rejects a Cargo path that exists but is not executable
- `is_scored` accepts all eight supported extensions
- `is_scored` rejects `.d.ts`, `.d.mts`, and `.d.cts`
- `is_scored` rejects an unrelated extension and an extensionless name
- The shipped `init_options.threshold` is 11

Testing the LSP handshake itself would mean spawning a real server and waiting on diagnostics.
That is deliberately out of scope; the manual check below covers the wiring, as `runIde` does
for the JetBrains plugin.

Manual verification is unchanged from the other two integrations: open `test.ts` in the
repository root, which holds functions that are clean, over the threshold, and over double it,
so a working setup shows one warning and one error.

## Installation

The directory goes on the runtimepath. With lazy.nvim:

```lua
{ dir = "~/code/jscx/extensions/nvim" },
```

Then, once, alongside any other servers:

```lua
vim.lsp.enable("jscx")
```

The spec must not be lazy-loaded on `ft`, `event`, or `cmd`. `vim.lsp.enable("jscx")` runs at
startup and needs `lsp/jscx.lua` on the runtimepath already; a lazy trigger adds the directory
later, and the config is silently not found. Eager-loading a file this size costs nothing.

Configurations that do not use a plugin manager can prepend the directory directly:

```lua
vim.opt.runtimepath:prepend(vim.fn.expand("~/code/jscx/extensions/nvim"))
```

## Changes outside the plugin

| File | Change |
| --- | --- |
| `README.md` | A `### Neovim` section under Editor integration, and the introduction's "Both editor integrations" wording updated for three |
| `extensions/nvim/README.md` | New; install, configure, and the restart caveat |

No Rust changes. The server already accepts everything this client sends, and the threshold
default it applies is the one being shipped.
