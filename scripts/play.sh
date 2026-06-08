#!/usr/bin/env bash
# Open an Atari 2600 ROM in a windowed emulator for visual inspection.
#
# Resolves an emulator the same way the MCP server's play_rom tool does:
# Stella first (STELLA env, PATH, or the macOS /Applications/Stella.app bundle),
# then a full Gopher2600 (GOPHER2600 env or PATH). The headless debugger helper
# has no display, so one of these must be installed.
#
# Usage: scripts/play.sh <rom> [emulator args...]
set -euo pipefail

if [ $# -lt 1 ]; then
  echo "usage: $(basename "$0") <rom> [emulator args...]" >&2
  exit 2
fi

rom="$1"
shift
if [ ! -f "$rom" ]; then
  echo "error: ROM not found: $rom" >&2
  exit 1
fi

find_emulator() {
  # Stella, preferred.
  if [ -n "${STELLA:-}" ] && [ -x "$STELLA" ]; then echo "$STELLA"; return; fi
  if command -v stella >/dev/null 2>&1; then command -v stella; return; fi
  if [ -x "/Applications/Stella.app/Contents/MacOS/Stella" ]; then
    echo "/Applications/Stella.app/Contents/MacOS/Stella"; return
  fi
  # Full Gopher2600, fallback.
  if [ -n "${GOPHER2600:-}" ] && [ -x "$GOPHER2600" ]; then echo "$GOPHER2600"; return; fi
  if command -v gopher2600 >/dev/null 2>&1; then command -v gopher2600; return; fi
}

emulator="$(find_emulator || true)"
if [ -z "$emulator" ]; then
  echo "error: no windowed emulator found." >&2
  echo "  install one: brew install --cask stella" >&2
  echo "  or build a full Gopher2600 and set GOPHER2600=/path/to/gopher2600" >&2
  exit 1
fi

echo ">> $emulator $rom"
exec "$emulator" "$rom" "$@"
