#!/usr/bin/env bash
# Assemble an Atari 2600 ASM source and open the resulting ROM in an emulator.
#
# Assembles with DASM in the same way as the server's build_rom tool: -f3 raw
# output, a sibling .sym symbol file, and the bundled vcs.h/macro.h plus the
# source's own directory on the include path. Then launches the ROM via
# scripts/play.sh (Stella, else a full Gopher2600).
#
# Usage: scripts/build-and-play.sh <source.asm> [output.bin] [-- emulator args...]
#
# Examples:
#   scripts/build-and-play.sh tests/fixtures/colorbars.asm
#   scripts/build-and-play.sh game.dasm roms/game.bin
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [ $# -lt 1 ]; then
  echo "usage: $(basename "$0") <source.asm> [output.bin] [-- emulator args...]" >&2
  exit 2
fi

source="$1"
shift

# Optional output path (anything before a `--` separator that isn't a flag).
output=""
if [ $# -gt 0 ] && [ "$1" != "--" ]; then
  output="$1"
  shift
fi
# Drop a leading `--` so the rest are emulator args.
if [ $# -gt 0 ] && [ "$1" = "--" ]; then
  shift
fi

if [ ! -f "$source" ]; then
  echo "error: source not found: $source" >&2
  exit 1
fi
if ! command -v dasm >/dev/null 2>&1; then
  echo "error: dasm not found (brew install dasm)" >&2
  exit 1
fi

# Default output: source with a .bin extension.
if [ -z "$output" ]; then
  output="${source%.*}.bin"
fi
symbols="${output%.*}.sym"
srcdir="$(cd "$(dirname "$source")" && pwd)"

echo ">> assembling $source -> $output"
dasm "$source" -f3 "-o$output" "-s$symbols" -I"$root/assets/dasm" -I"$srcdir"

echo ">> launching $output"
exec "$root/scripts/play.sh" "$output" "$@"
