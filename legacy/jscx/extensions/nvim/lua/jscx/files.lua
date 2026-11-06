--- Which buffers jscx should score.
---
--- Mirrors `isSupportedByJscx` in the JetBrains plugin. The declaration-file
--- exclusion is load-bearing: the CLI skips those files unless `--include-dts`
--- is passed, but `src/lsp.rs` analyzes whatever text arrives on the wire, so
--- without a client-side filter Neovim would report complexity the command line
--- never shows for the same file.
local M = {}

local SUPPORTED = {
  js = true,
  jsx = true,
  mjs = true,
  cjs = true,
  ts = true,
  tsx = true,
  mts = true,
  cts = true,
}

local DECLARATIONS = { "%.d%.ts$", "%.d%.mts$", "%.d%.cts$" }

--- @param name string buffer name, usually an absolute path
--- @return boolean
function M.is_scored(name)
  local lower = name:lower()

  for _, pattern in ipairs(DECLARATIONS) do
    if lower:match(pattern) then
      return false
    end
  end

  -- Excluding separators from the captured run keeps a dot in a directory name
  -- from being read as the file's extension.
  local ext = lower:match("%.([^%./\\]+)$")
  return ext ~= nil and SUPPORTED[ext] == true
end

return M
