#!/usr/bin/env bash
# Readable output helpers for scripts handed to a human to run.
# Colors switch themselves off when the output is not a terminal, or when
# NO_COLOR is set, so this is safe under `tee` and in CI.

if [[ -t 1 && -z "${NO_COLOR:-}" ]]; then
  _c_cyan=$'\033[36m'
  _c_green=$'\033[32m'
  _c_yellow=$'\033[33m'
  _c_red=$'\033[31m'
  _c_dim=$'\033[2m'
  _c_bold=$'\033[1m'
  _c_reset=$'\033[0m'
else
  _c_cyan='' _c_green='' _c_yellow='' _c_red='' _c_dim='' _c_bold='' _c_reset=''
fi

step() { printf '\n%s==>%s %s%s%s\n' "$_c_cyan" "$_c_reset" "$_c_bold" "$*" "$_c_reset"; }
ok()   { printf '%s  ok%s %s\n' "$_c_green" "$_c_reset" "$*"; }
warn() { printf '%swarn%s %s\n' "$_c_yellow" "$_c_reset" "$*" >&2; }
err()  { printf '%s err%s %s\n' "$_c_red" "$_c_reset" "$*" >&2; }
info() { printf '%s     %s%s\n' "$_c_dim" "$*" "$_c_reset"; }
