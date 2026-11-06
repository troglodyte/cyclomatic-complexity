#!/usr/bin/env bash
# Records stdout/stderr/exit of the pre-merge jscx and rustcx binaries.
# Run from the repo root: tools/capture-goldens.sh
set -euo pipefail

export NO_COLOR=1 CLICOLOR=0

cargo build --release -p jscx-legacy -p rustcx-legacy

JSCX="$PWD/target/release/jscx-legacy"
RUSTCX="$PWD/target/release/rustcx-legacy"
GOLDEN_DIR="tests/golden"

rm -rf "$GOLDEN_DIR/jscx" "$GOLDEN_DIR/rustcx"
mkdir -p "$GOLDEN_DIR/jscx" "$GOLDEN_DIR/rustcx"

# capture <tool> <case> <stdin-file|-> <args...>
capture() {
  local tool="$1" case_name="$2" stdin_file="$3"
  shift 3
  local bin out
  case "$tool" in
    jscx) bin="$JSCX" ;;
    rustcx) bin="$RUSTCX" ;;
  esac
  out="$GOLDEN_DIR/$tool/$case_name"
  local code=0
  if [[ "$stdin_file" == "-" ]]; then
    "$bin" "$@" >"$out.stdout" 2>"$out.stderr" </dev/null || code=$?
  else
    "$bin" "$@" >"$out.stdout" 2>"$out.stderr" <"$stdin_file" || code=$?
  fi
  echo "$code" >"$out.exit"
}

for fixture_path in legacy/rustcx/fixtures/*.rs; do
  fixture_name="$(basename "$fixture_path" .rs)"
  capture rustcx "json-$fixture_name" - --json "$fixture_path"
  capture rustcx "explain-$fixture_name" - --explain "$fixture_path"
done
capture rustcx top - --top 5 legacy/rustcx/fixtures
capture rustcx max - --max 3 legacy/rustcx/fixtures
capture rustcx missing - does-not-exist.rs

for fixture_name in test.js test.ts; do
  capture jscx "json-$fixture_name" - --json "legacy/jscx/$fixture_name"
  capture jscx "explain-$fixture_name" - --explain "legacy/jscx/$fixture_name"
done
capture jscx top - --top 5 legacy/jscx
capture jscx max - --max 3 legacy/jscx
capture jscx stdin-ts legacy/jscx/test.ts --json --ext ts -
capture jscx missing - does-not-exist.js
