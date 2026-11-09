--- Neovim LSP config for cmplx. Discovered off the runtimepath by
--- `vim.lsp.enable("cmplx")`; see extensions/nvim/README.md.
local binary = require("cmplx.binary")

local ROOT_MARKERS = { "Cargo.toml", "package.json", "tsconfig.json", "jsconfig.json", ".git" }

local NOT_FOUND = table.concat({
  "cmplx not found on PATH or in ~/.cargo/bin. Install it with",
  "'cargo install --path crates/cmplx' from the repository root, or point at it with",
  'vim.lsp.config("cmplx", { cmd = { "/path/to/cmplx", "--lsp" } }).',
}, "\n")

local exe = binary.resolve()
if not exe then
  vim.notify(NOT_FOUND, vim.log.levels.WARN)
end

return {
  cmd = { exe or binary.EXECUTABLE, "--lsp" },
  -- Exposed for tests and for `vim.lsp.config` users; `root_dir` below is what
  -- Neovim actually consults.
  root_markers = ROOT_MARKERS,
  filetypes = { "javascript", "javascriptreact", "typescript", "typescriptreact", "rust" },
  -- No `init_options` key on purpose. An absent threshold is what lets
  -- cmplx.toml apply; sending a default here would override the file in every
  -- project. Set one explicitly with:
  --   vim.lsp.config("cmplx", { init_options = { threshold = 15 } })
  --
  -- A predicate rather than `root_markers`, because declining to call `on_dir`
  -- is the only hook Neovim offers for "do not attach to this buffer" — which
  -- is what a missing binary needs.
  root_dir = function(bufnr, on_dir)
    if not exe then
      return
    end
    on_dir(vim.fs.root(bufnr, ROOT_MARKERS) or vim.fn.getcwd())
  end,
}
