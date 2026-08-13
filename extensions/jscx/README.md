# jscx Zed Extension

Cyclomatic complexity diagnostics for JavaScript and TypeScript.

## Installation

1. Install the `jscx` CLI tool:
   ```bash
   cargo install --path ../..
   ```
2. In Zed, run `zed: install dev extension` and select this directory
   (`extensions/jscx`, relative to the repository root). Zed compiles the
   extension to WebAssembly itself, so there is nothing to build first.
