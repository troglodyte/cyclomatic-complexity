-- Headless test suite. Run with:
--   nvim -l extensions/nvim/test/jscx_spec.lua
-- Exits non-zero if any assertion fails.

local script = debug.getinfo(1, "S").source:sub(2)
local root = vim.fs.dirname(vim.fs.dirname(script))
vim.opt.runtimepath:prepend(root)

local files = require("jscx.files")
local binary = require("jscx.binary")

local passed, failed = 0, 0

local function check(name, ok)
  if ok then
    passed = passed + 1
    io.write("ok    ", name, "\n")
  else
    failed = failed + 1
    io.write("FAIL  ", name, "\n")
  end
end

-- jscx.files.is_scored

local supported = { "js", "jsx", "mjs", "cjs", "ts", "tsx", "mts", "cts" }
for _, ext in ipairs(supported) do
  check(
    "is_scored accepts ." .. ext,
    files.is_scored("/project/src/checkout." .. ext) == true
  )
end

for _, name in ipairs({ "types.d.ts", "types.d.mts", "types.d.cts" }) do
  check(
    "is_scored rejects " .. name,
    files.is_scored("/project/src/" .. name) == false
  )
end

check("is_scored rejects an unrelated extension", files.is_scored("/project/init.lua") == false)
check("is_scored rejects an extensionless name", files.is_scored("/project/Makefile") == false)
check("is_scored rejects an empty name", files.is_scored("") == false)
check("is_scored ignores extension case", files.is_scored("/project/App.TSX") == true)
check(
  "is_scored is not fooled by a dot in a directory name",
  files.is_scored("/project/v1.2/Makefile") == false
)

-- jscx.binary.resolve

--- Dependency set with nothing found anywhere, for tests to override piecemeal.
local function deps(overrides)
  return vim.tbl_extend("force", {
    exepath = function()
      return ""
    end,
    executable = function()
      return false
    end,
    home = "/home/tester",
    windows = false,
  }, overrides or {})
end

check(
  "resolve returns the PATH hit",
  binary.resolve(deps({
    exepath = function()
      return "/usr/local/bin/jscx"
    end,
    executable = function(path)
      return path == "/usr/local/bin/jscx"
    end,
  })) == "/usr/local/bin/jscx"
)

check(
  "resolve falls back to the cargo bin when PATH has no hit",
  binary.resolve(deps({
    executable = function(path)
      return path == "/home/tester/.cargo/bin/jscx"
    end,
  })) == "/home/tester/.cargo/bin/jscx"
)

check(
  "resolve falls back when the PATH hit is not executable",
  binary.resolve(deps({
    exepath = function()
      return "/usr/local/bin/jscx"
    end,
    executable = function(path)
      return path == "/home/tester/.cargo/bin/jscx"
    end,
  })) == "/home/tester/.cargo/bin/jscx"
)

check(
  "resolve returns nil when the binary is nowhere",
  binary.resolve(deps()) == nil
)

check(
  "resolve returns nil when the cargo path exists but is not executable",
  binary.resolve(deps({
    executable = function()
      return false
    end,
  })) == nil
)

check(
  "resolve looks for jscx.exe in the cargo bin on Windows",
  binary.resolve(deps({
    windows = true,
    executable = function(path)
      return path == "/home/tester/.cargo/bin/jscx.exe"
    end,
  })) == "/home/tester/.cargo/bin/jscx.exe"
)

check("EXECUTABLE is the plain binary name", binary.EXECUTABLE == "jscx")

-- lsp/jscx.lua

local notified = {}
local real_notify = vim.notify
vim.notify = function(msg)
  table.insert(notified, msg)
end
local config = dofile(vim.fs.joinpath(root, "lsp", "jscx.lua"))
vim.notify = real_notify

check("config default threshold is 11", config.init_options.threshold == 11)
check(
  "config declares the four JS and TS filetypes",
  vim.deep_equal(config.filetypes, {
    "javascript",
    "javascriptreact",
    "typescript",
    "typescriptreact",
  })
)
check("config passes --lsp to the binary", config.cmd[2] == "--lsp")
check("config resolves a command name", type(config.cmd[1]) == "string" and config.cmd[1] ~= "")
check("config exposes root_dir as a predicate", type(config.root_dir) == "function")

-- root_dir declines rather than attaching, which is Neovim's only per-buffer
-- opt-out. Nothing may call on_dir for a declaration file.
local buf = vim.api.nvim_create_buf(false, true)
vim.api.nvim_buf_set_name(buf, vim.fs.joinpath(root, "types.d.ts"))
local attached = false
config.root_dir(buf, function()
  attached = true
end)
check("root_dir declines a declaration file", attached == false)

-- A scored buffer attaches, and only when the binary was found. Skipped rather
-- than failed when jscx is not installed, so the suite passes on a clean machine.
vim.api.nvim_buf_set_name(buf, vim.fs.joinpath(root, "scored.ts"))
local dir = nil
config.root_dir(buf, function(found)
  dir = found
end)
if #notified == 0 then
  check("root_dir accepts a scored file", type(dir) == "string" and dir ~= "")
else
  io.write("skip  root_dir accepts a scored file (jscx not installed)\n")
  check("a missing binary warns exactly once", #notified == 1)
end

io.write(string.format("\n%d passed, %d failed\n", passed, failed))
os.exit(failed == 0 and 0 or 1)
