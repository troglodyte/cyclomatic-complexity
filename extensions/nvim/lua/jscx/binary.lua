--- Locates the `jscx` executable.
local M = {}

M.EXECUTABLE = "jscx"

--- The real lookups, injectable so every branch below is testable.
local function real_deps()
  return {
    exepath = function(name)
      return vim.fn.exepath(name)
    end,
    executable = function(path)
      return vim.fn.executable(path) == 1
    end,
    home = vim.uv.os_homedir(),
    windows = vim.fn.has("win32") == 1,
  }
end

--- Resolve the binary: `PATH` first, then Cargo's bin directory.
---
--- The Cargo fallback is not redundant. A GUI frontend such as Neovide or VimR
--- launched from the Dock does not inherit the shell's `PATH`, so
--- `~/.cargo/bin` is routinely invisible to the first step even though the
--- binary is installed and works fine in a terminal.
---
--- Returns `nil` rather than an unchecked path, so a missing binary produces one
--- warning and no client instead of a client that fails to spawn.
---
--- @param overrides table|nil injected `exepath`, `executable`, `home`, `windows` for testing
--- @return string|nil
function M.resolve(overrides)
  local deps = vim.tbl_extend("force", real_deps(), overrides or {})

  local found = deps.exepath(M.EXECUTABLE)
  if found ~= "" and deps.executable(found) then
    return found
  end

  local name = deps.windows and (M.EXECUTABLE .. ".exe") or M.EXECUTABLE
  local cargo = vim.fs.joinpath(deps.home, ".cargo", "bin", name)
  if deps.executable(cargo) then
    return cargo
  end

  return nil
end

return M
