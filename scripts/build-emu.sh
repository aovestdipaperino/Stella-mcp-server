#!/usr/bin/env bash
# Build the headless Gopher2600 debugger helper (bin/gopher2600-mcp).
#
# Gopher2600's published module is too large for Go's module proxy (its CPU
# test fixtures are ~1GB), so we build against a local clone referenced by a
# `replace` directive in emu/go.mod. This script creates and prunes that clone
# on first run, then compiles the helper. It is safe to re-run.
set -euo pipefail

REPO_URL="https://github.com/JetSetIlly/Gopher2600"
PIN="${GOPHER2600_REF:-v0.56.1}"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
clone="$root/third_party/gopher2600"
out="$root/bin/gopher2600-mcp"

if ! command -v go >/dev/null 2>&1; then
  echo "error: Go toolchain not found on PATH (need Go >= 1.25). Try: brew install go" >&2
  exit 1
fi

if [ ! -d "$clone/.git" ]; then
  echo ">> cloning Gopher2600 ($PIN) into third_party/ ..."
  rm -rf "$clone"
  git clone --depth 1 --branch "$PIN" "$REPO_URL" "$clone" 2>/dev/null \
    || git clone --depth 1 "$REPO_URL" "$clone"
fi

# Prune the ~1GB CPU test-suite fixtures; only the 6502 emulation source is
# needed to compile, never the test corpus.
if [ -d "$clone/hardware/cpu/tests" ]; then
  echo ">> pruning hardware/cpu/tests fixtures ..."
  rm -rf "$clone/hardware/cpu/tests"
fi

echo ">> building bin/gopher2600-mcp ..."
mkdir -p "$root/bin"
( cd "$root/emu" && go build -o "$out" . )

echo ">> done: $out"
"$out" -h 2>&1 | head -3 || true
