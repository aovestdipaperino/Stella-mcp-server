// Command gopher2600-mcp runs the Gopher2600 debugger headlessly, driven over a
// framed stdin/stdout protocol so it can be controlled programmatically by the
// stella-mcp server.
//
// It reuses the upstream Gopher2600 debugger engine unmodified: it constructs a
// debugger with a stub GUI (no window, no SDL/OpenGL) and the framed terminal
// from the mcpterm package, then enters debug mode on the supplied ROM. This
// mirrors the upstream "headless" launch path but swaps the plain terminal for
// one that emits machine-readable command frames.
//
// Usage:
//
//	gopher2600-mcp [-mapping AUTO] [-tv AUTO] [-log] [-script FILE] <rom>
//
// Commands are read one per line from stdin; output is framed on stdout. See the
// mcpterm package for the protocol. Sending QUIT or closing stdin exits cleanly.
package main

import (
	"flag"
	"fmt"
	"os"

	"github.com/aovestdipaperino/stella-mcp/emu/mcpterm"
	"github.com/aovestdipaperino/stella-mcp/emu/screen"
	"github.com/jetsetilly/gopher2600/debugger"
	"github.com/jetsetilly/gopher2600/debugger/terminal"
	"github.com/jetsetilly/gopher2600/gui"
)

func main() {
	if err := run(os.Args[1:]); err != nil {
		fmt.Fprintf(os.Stderr, "gopher2600-mcp: %v\n", err)
		os.Exit(1)
	}
}

func run(args []string) error {
	var opts debugger.CommandLineOptions
	opts.Default()

	flgs := flag.NewFlagSet("gopher2600-mcp", flag.ContinueOnError)
	flgs.BoolVar(&opts.Log, "log", false, "echo the emulation log to output")
	flgs.StringVar(&opts.Spec, "tv", "AUTO", "television specification: AUTO, NTSC, PAL, PAL60, SECAM")
	flgs.StringVar(&opts.Mapping, "mapping", "AUTO", "force cartridge mapper selection")
	flgs.StringVar(&opts.Bank, "bank", "AUTO", "selected cartridge bank on reset")
	flgs.StringVar(&opts.Script, "script", "", "debugger script to run on start")
	if err := flgs.Parse(args); err != nil {
		return err
	}

	rest := flgs.Args()
	if len(rest) == 0 {
		return fmt.Errorf("no ROM file specified")
	}
	if len(rest) > 1 {
		return fmt.Errorf("too many arguments; expected a single ROM file")
	}
	romFile := rest[0]

	// build a debugger with a headless stub GUI and the framed MCP terminal.
	// the create function is the injection point upstream uses for its own GUI
	// and terminal; supplying a stub GUI keeps the build free of SDL/OpenGL. A
	// frame capturer is attached to the (display-less) TV so the terminal's
	// SCREENSHOT pseudo-command can save PNGs.
	capture := screen.New()
	dbg, err := debugger.NewDebugger(opts, func(e *debugger.Debugger) (gui.GUI, terminal.Terminal, error) {
		e.TV().AddPixelRenderer(capture)
		return &gui.Stub{}, &mcpterm.MCPTerminal{Capture: capture}, nil
	})
	if err != nil {
		return err
	}

	return dbg.StartInDebugMode(romFile)
}
