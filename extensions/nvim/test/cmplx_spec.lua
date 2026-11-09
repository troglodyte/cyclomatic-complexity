-- Headless test suite. Run with:
--   nvim -l extensions/nvim/test/cmplx_spec.lua
-- Exits non-zero if any assertion fails.

local script = debug.getinfo(1, "S").source:sub(2)
local root = vim.fs.dirname(vim.fs.dirname(script))
vim.opt.runtimepath:prepend(root)

local binary = require("cmplx.binary")

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

-- cmplx.binary.resolve

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
      return "/usr/local/bin/cmplx"
    end,
    executable = function(path)
      return path == "/usr/local/bin/cmplx"
    end,
  })) == "/usr/local/bin/cmplx"
)

check(
  "resolve falls back to the cargo bin when PATH has no hit",
  binary.resolve(deps({
    executable = function(path)
      return path == "/home/tester/.cargo/bin/cmplx"
    end,
  })) == "/home/tester/.cargo/bin/cmplx"
)

check(
  "resolve falls back when the PATH hit is not executable",
  binary.resolve(deps({
    exepath = function()
      return "/usr/local/bin/cmplx"
    end,
    executable = function(path)
      return path == "/home/tester/.cargo/bin/cmplx"
    end,
  })) == "/home/tester/.cargo/bin/cmplx"
)

check("resolve returns nil when the binary is nowhere", binary.resolve(deps()) == nil)

check(
  "resolve returns nil when the cargo path exists but is not executable",
  binary.resolve(deps({
    executable = function()
      return false
    end,
  })) == nil
)

check(
  "resolve looks for cmplx.exe in the cargo bin on Windows",
  binary.resolve(deps({
    windows = true,
    executable = function(path)
      return path == "/home/tester/.cargo/bin/cmplx.exe"
    end,
  })) == "/home/tester/.cargo/bin/cmplx.exe"
)

check("EXECUTABLE is the plain binary name", binary.EXECUTABLE == "cmplx")

-- lsp/cmplx.lua

local notified = {}
local real_notify = vim.notify
vim.notify = function(msg)
  table.insert(notified, msg)
end
local config = dofile(vim.fs.joinpath(root, "lsp", "cmplx.lua"))
vim.notify = real_notify

check(
  "config sends no init_options, so cmplx.toml applies",
  config.init_options == nil
)
check(
  "config declares the JS, TS and Rust filetypes",
  vim.deep_equal(
    config.filetypes,
    { "javascript", "javascriptreact", "typescript", "typescriptreact", "rust" }
  )
)
check(
  "config roots on JS, TS and Rust project markers",
  vim.deep_equal(
    config.root_markers,
    { "Cargo.toml", "package.json", "tsconfig.json", "jsconfig.json", ".git" }
  )
)
check("config passes --lsp to the binary", config.cmd[2] == "--lsp")
check("config resolves a command name", type(config.cmd[1]) == "string" and config.cmd[1] ~= "")
check("config exposes root_dir as a predicate", type(config.root_dir) == "function")

-- root_dir declines rather than attaching when the binary is missing, which is
-- Neovim's only per-buffer opt-out. Skipped rather than failed when cmplx is
-- not installed, so the suite passes on a clean machine.
local buf = vim.api.nvim_create_buf(false, true)
vim.api.nvim_buf_set_name(buf, vim.fs.joinpath(root, "scored.rs"))
local dir = nil
config.root_dir(buf, function(found)
  dir = found
end)
if #notified == 0 then
  check("root_dir accepts a scored file", type(dir) == "string" and dir ~= "")
else
  io.write("skip  root_dir accepts a scored file (cmplx not installed)\n")
  check("a missing binary warns exactly once", #notified == 1)
  check("a missing binary declines to attach", dir == nil)
end

io.write(string.format("\n%d passed, %d failed\n", passed, failed))
os.exit(failed == 0 and 0 or 1)
