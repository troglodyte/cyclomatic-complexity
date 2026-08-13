--- Neovim LSP config for jscx. Discovered off the runtimepath by
--- `vim.lsp.enable("jscx")`; see extensions/nvim/README.md.
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
  cmd = { exe or binary.EXECUTABLE, "--lsp" },
  filetypes = { "javascript", "javascriptreact", "typescript", "typescriptreact" },
  init_options = { threshold = DEFAULT_THRESHOLD },
  -- A predicate rather than `root_markers`, because declining to call `on_dir`
  -- is the only hook Neovim offers for "do not attach to this buffer" — needed
  -- both for declaration files and for a missing binary.
  root_dir = function(bufnr, on_dir)
    if not exe or not files.is_scored(vim.api.nvim_buf_get_name(bufnr)) then
      return
    end
    on_dir(vim.fs.root(bufnr, MARKERS) or vim.fn.getcwd())
  end,
}
