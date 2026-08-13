# Set up `jscx` as an LSP server in Neovim

Hand this file to Claude (or follow it yourself) to wire `jscx` into an
`init.lua`. Everything Claude needs is here; it should not have to guess.

## What jscx is

`jscx` is a single Rust binary that reports **cyclomatic complexity** for
JavaScript / TypeScript / JSX functions. It has a `--lsp` mode that speaks
LSP over stdio and publishes complexity warnings as diagnostics.

- Repository: <https://github.com/troglodyte/jscx>
- Server command: `jscx --lsp`
- Transport: stdio, standard `Content-Length` framing

## Step 1 — install the binary

```console
git clone https://github.com/troglodyte/jscx
cd jscx
cargo install --path .
```

That puts `jscx` in `~/.cargo/bin`. Confirm it is on `$PATH` and that LSP mode
starts (it will sit waiting on stdin — `Ctrl-C` to quit):

```console
jscx --version
jscx --lsp
```

If `jscx` is not on `$PATH`, use an absolute path in `cmd` below instead of
adding a shim.

## Step 2 — what the server actually supports

Do **not** configure capabilities it does not have. The full contract:

| Aspect | Value |
| --- | --- |
| Launch | `jscx --lsp` (stdio) |
| Sync kind | Full (`textDocumentSync: 1`) |
| Publishes | `textDocument/publishDiagnostics` |
| Handles | `initialize`, `didOpen`, `didChange`, `didSave`, `shutdown`, `exit` |
| Settings | `initializationOptions.threshold` (integer, default `10`) |
| Filetypes | `javascript`, `javascriptreact`, `typescript`, `typescriptreact` |

There is **no** hover, completion, formatting, code actions, symbols, or
rename. There is no `workspace/didChangeConfiguration` handler — the threshold
is read once at `initialize`, so changing it requires `:LspRestart`.

Diagnostic severity: a function scoring `>= threshold` is a **warning**;
`>= threshold * 2` is an **error**. Diagnostics anchor to the function's
opening line.

`didSave` carries no text unless the client opts in, so saving alone may not
refresh diagnostics — `didChange` (full text) is what drives updates. This is
expected; no configuration is needed for it.

## Step 3 — the Neovim config

Pick the section matching the user's Neovim version. Check with `nvim --version`.

### Neovim 0.11+ (native `vim.lsp.config`, no plugins)

Add to `init.lua`:

```lua
vim.lsp.config.jscx = {
  cmd = { 'jscx', '--lsp' },
  filetypes = {
    'javascript',
    'javascriptreact',
    'typescript',
    'typescriptreact',
  },
  root_markers = { 'package.json', 'tsconfig.json', '.git' },
  init_options = {
    threshold = 10, -- warn at 10, error at 20
  },
}

vim.lsp.enable('jscx')
```

`root_markers` is only used to group buffers into one server instance; jscx
works fine on a single file with no project root.

### Neovim 0.8 – 0.10 (no plugins, autocmd + `vim.lsp.start`)

```lua
vim.api.nvim_create_autocmd('FileType', {
  pattern = {
    'javascript',
    'javascriptreact',
    'typescript',
    'typescriptreact',
  },
  callback = function(args)
    vim.lsp.start({
      name = 'jscx',
      cmd = { 'jscx', '--lsp' },
      root_dir = vim.fs.root(args.buf, { 'package.json', 'tsconfig.json', '.git' }),
      init_options = { threshold = 10 },
    })
  end,
})
```

`vim.lsp.start` reuses an existing client when `name` and `root_dir` match, so
this does not spawn one server per buffer.

### With `nvim-lspconfig`

`jscx` is not in lspconfig's registry, so register it first. On lspconfig's
current API:

```lua
vim.lsp.config('jscx', {
  cmd = { 'jscx', '--lsp' },
  filetypes = {
    'javascript',
    'javascriptreact',
    'typescript',
    'typescriptreact',
  },
  root_markers = { 'package.json', 'tsconfig.json', '.git' },
  init_options = { threshold = 10 },
})
vim.lsp.enable('jscx')
```

On older lspconfig (pre-`vim.lsp.config`), register through `configs`:

```lua
local lspconfig = require('lspconfig')
local configs = require('lspconfig.configs')

if not configs.jscx then
  configs.jscx = {
    default_config = {
      cmd = { 'jscx', '--lsp' },
      filetypes = {
        'javascript',
        'javascriptreact',
        'typescript',
        'typescriptreact',
      },
      root_dir = lspconfig.util.root_pattern('package.json', 'tsconfig.json', '.git'),
      single_file_support = true,
      init_options = { threshold = 10 },
    },
  }
end

lspconfig.jscx.setup({})
```

### With `lazy.nvim`

Put the version-appropriate block above inside a `config` function — jscx needs
no plugin of its own, only the binary:

```lua
{
  'neovim/nvim-lspconfig',
  config = function()
    vim.lsp.config('jscx', {
      cmd = { 'jscx', '--lsp' },
      filetypes = {
        'javascript',
        'javascriptreact',
        'typescript',
        'typescriptreact',
      },
      root_markers = { 'package.json', 'tsconfig.json', '.git' },
      init_options = { threshold = 10 },
    })
    vim.lsp.enable('jscx')
  end,
}
```

## Step 4 — coexisting with `tsserver` / `vtsls` / `eslint`

jscx is additive. Neovim attaches multiple clients to one buffer without
conflict, and jscx claims no capability another server provides, so nothing
needs disabling. Its diagnostics show up alongside the others with
`source = "jscx"`.

To visually separate them, optionally prefix the source in virtual text:

```lua
vim.diagnostic.config({ virtual_text = { source = 'if_many' } })
```

## Step 5 — verify

1. Open a `.ts` or `.tsx` file containing a function with 10+ branches.
2. `:checkhealth vim.lsp` — jscx should be listed as attached.
3. `:lua =vim.lsp.get_clients({ name = 'jscx' })` — should return one client.
4. A warning reading `Complexity is N for function 'name'` should appear on
   the function's signature line.
5. `:LspLog` if nothing shows up.

If diagnostics never appear, lower the threshold to `2` temporarily and edit
the buffer (typing triggers `didChange`, which is what recomputes scores).

## Tuning the threshold

`threshold` is the warning floor; double it is the error floor. Reasonable
picks:

| threshold | Effect |
| --- | --- |
| `5` | aggressive — flags most non-trivial functions |
| `10` | default; matches ESLint `complexity` conventions |
| `15` | quiet; only genuinely gnarly functions |

Changing it requires restarting the server (`:LspRestart`), since it is read
once at `initialize`.
