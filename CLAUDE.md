# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

An MCP server that runs and debugs Atari 2600 (VCS) games. It is two programs in two languages:

- **`src/`** — the Rust MCP server (`stella-mcp`), built on `rmcp` 1.7, speaking MCP over stdio.
- **`emu/`** — a small Go helper (`gopher2600-mcp`) that embeds the upstream [Gopher2600](https://github.com/JetSetIlly/Gopher2600) debugger engine headlessly.

The server spawns one helper process per debug session and drives it over a private framed protocol. The name is historical (Stella is the canonical 2600 emulator); the actual engine is Gopher2600, chosen because its debugger has a real command grammar that can be driven programmatically.

## Build & run

```sh
./scripts/build-emu.sh        # build the Go helper -> bin/gopher2600-mcp (clones+prunes Gopher2600 on first run)
cargo build --release         # build the Rust server -> target/release/stella-mcp
python3 scripts/smoke.py      # end-to-end test (needs helper built + dasm); CI runs this
cargo clippy --all-targets -- -D warnings   # CI lints with -D warnings; keep it clean
cargo fmt --all               # CI checks formatting (and gofmt for emu/)
```

There is no `cargo test` suite; correctness is covered by `scripts/smoke.py`, which drives the compiled server over stdio (build ROM → load → step → screenshot → assert a non-blank frame). The helper protocol can be exercised directly:

```sh
printf 'CPU\nSTEP\nQUIT\n' | ./bin/gopher2600-mcp <rom.bin>   # markers use \x1e; pipe through `cat -v` to see them
```

Shell convenience scripts: `scripts/play.sh <rom>` (launch in a windowed emulator), `scripts/build-and-play.sh <src.asm>` (assemble + launch).

## The framed protocol (the contract between Rust and Go)

This is the core design and spans `emu/mcpterm/mcpterm.go` and `src/session.rs`. Gopher2600's stock terminal suppresses its prompt over a pipe, so there is no command boundary. `mcpterm` is a custom `terminal.Terminal` that fences every command's output with markers built from ASCII Record Separator (`\x1e`), which never appears in debugger output:

```
\x1eBEGIN seq=<n>\x1e
  ...command output, one line each...
\x1ePROMPT\x1e <next-instruction>     # post-command position (drives STEP visibility)
\x1eERR\x1e <message>                 # error-styled line; sets the err flag
\x1eEND seq=<n> err=<0|1>\x1e
```

ROM-load output is reported under `seq=0` so the driver can sync on startup. `src/session.rs::collect_frame` reads until `END`, strips markers, and surfaces `PROMPT` as a trailing `→ ...` line. If you change one side's marker format, change both.

`SCREENSHOT <path>` is a **terminal-level pseudo-command** (handled inside `mcpterm.TermRead`, never passed to the debugger): it saves the latest frame via the `emu/screen` `PixelRenderer` attached to the display-less TV, and gets its own protocol frame.

## Architecture notes that aren't obvious from one file

- **Headless = no SDL.** `emu/main.go` mirrors Gopher2600's `headless()` path: `debugger.NewDebugger` with a `gui.Stub` plus our terminal. It imports only `debugger`/`gui`/`terminal`, never `sdlimgui`, so the build needs only Go — no SDL/OpenGL/cgo. Keep it that way.
- **Vendoring.** Gopher2600's published module exceeds Go's module-zip limit (its `hardware/cpu/tests` fixtures are ~1 GB), so `emu/go.mod` uses a `replace` pointing at `third_party/gopher2600`, a local clone created and pruned by `scripts/build-emu.sh`. `third_party/` and `bin/` are git-ignored.
- **`run` auto-halt (`src/session.rs`).** Unbounded `RUN` never returns to the prompt. `Session::run` issues `RUN`, races a natural halt against a timer, and only on timeout sends SIGINT. For SIGINT to *halt* (not quit) the debugger, `mcpterm.IsRealTerminal()` returns **true** (Gopher2600 quits a non-real terminal on interrupt), and `TermRead` auto-declines the resulting "really quit?" confirm. Prefer breakpoints + bounded `step` over free-running.
- **Screenshot capture (`emu/screen/capture.go`).** When halted, the debugger forces a refresh of the freshly-reset (empty) frame with `last==0`; those are ignored so the last fully-drawn frame is kept. Pixels are RGB (Go's encoder drops alpha when opaque).
- **`rmcp` tools.** `#[tool_handler]` dispatches via `Self::tool_router()`, so `StellaMcp` does **not** store the router. The crate enables pedantic clippy lints (see `Cargo.toml` `[lints]`); CI fails on warnings.
- **Building ROMs.** `build_rom` / `scripts/build-and-play.sh` assemble DASM / 8bitworkshop source with `dasm -f3`, always putting `assets/dasm/` (bundled `vcs.h`/`macro.h`) and the source dir on the include path, and emit a sibling `.sym` that the debugger auto-loads for symbolic disassembly.

## Tooling expected on PATH

Go ≥ 1.25 (helper), `dasm` (`build_rom`), `gh` (`fetch_gist`), and a windowed emulator for `play_rom` (Stella via `/Applications/Stella.app`, or a full Gopher2600). Each is resolved via an env override (`STELLA_MCP_EMU`, `DASM`, `GH`, `STELLA`, `GOPHER2600`) then PATH; `emulator_status` reports what was found. On macOS the Stella cask is unsigned — if launches fail, clear quarantine: `xattr -dr com.apple.quarantine /Applications/Stella.app`.

## Releases

CI (`.github/workflows/ci.yml`) runs rust/go/smoke jobs on push and PR. Pushing a `v*` tag triggers `release.yml`, which builds and publishes per-platform tarballs (server + helper + `assets/`).
