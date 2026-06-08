// Package mcpterm implements the Gopher2600 terminal.Terminal interface with a
// machine-readable, framed line protocol.
//
// The stock plain terminal (debugger/terminal/plainterm) suppresses its prompt
// when stdin is not a real TTY, so a program driving the debugger over a pipe
// has no reliable way to know when a command's output is complete. This
// terminal instead brackets every command's output with sentinel lines built
// from the ASCII Record Separator control character (0x1e), which never appears
// in disassembly, memory dumps, or other debugger output:
//
//	\x1eBEGIN seq=<n>\x1e        emitted just before reading command <n>
//	  ...command <n> output, one line per TermPrintLine...
//	\x1eEND seq=<n> err=<0|1>\x1e  emitted when command <n> has finished
//
// A driver therefore writes a command line to stdin and reads stdout until it
// sees the matching END marker; the err field reports whether the command
// produced any error-styled output. The output produced while the ROM is being
// loaded (before the first command) is reported under seq=0, which lets a
// driver synchronise on startup by reading until the first END marker.
package mcpterm

import (
	"bufio"
	"fmt"
	"io"
	"os"
	"strings"

	"github.com/aovestdipaperino/stella-mcp/emu/screen"
	"github.com/jetsetilly/gopher2600/debugger/terminal"
	"github.com/jetsetilly/gopher2600/debugger/terminal/commandline"
)

// rs is the ASCII Record Separator (0x1e) used to fence protocol markers so they
// cannot be confused with ordinary debugger output.
const rs = "\x1e"

// screenshotCmd is a terminal-level pseudo-command (not a debugger command)
// that saves the current frame as a PNG: "SCREENSHOT <path>".
const screenshotCmd = "SCREENSHOT"

// MCPTerminal is a terminal.Terminal that speaks the framed line protocol
// documented in the package comment. It reads commands from stdin and writes
// output and markers to stdout.
type MCPTerminal struct {
	// Capture, if set, retains the latest frame so SCREENSHOT can save it.
	Capture *screen.Capture

	input    *bufio.Reader
	output   io.Writer
	silenced bool

	// seq is the sequence number of the command currently being executed. It
	// starts at 0, which represents the ROM-loading output emitted before the
	// first command is read.
	seq uint64

	// err records whether the current command has emitted any error-styled
	// output. It is reported in the END marker and reset for each command.
	err bool
}

// Initialise sets up stdin/stdout for the framed protocol. It never returns an error.
func (t *MCPTerminal) Initialise() error {
	t.input = bufio.NewReader(os.Stdin)
	t.output = os.Stdout
	return nil
}

// CleanUp performs no work; the protocol holds no resources to release.
func (t *MCPTerminal) CleanUp() {}

// RegisterTabCompletion is a no-op; tab completion is meaningless over a pipe.
func (t *MCPTerminal) RegisterTabCompletion(*commandline.TabCompletion) {}

// Silence suppresses non-error output when true, matching the plain terminal.
func (t *MCPTerminal) Silence(silenced bool) {
	t.silenced = silenced
}

// TermPrintLine writes one line of command output, tracking error styling.
//
// StyleEcho lines (the echoed user input) are dropped. Error lines are prefixed
// with an "ERR" marker and set the error flag reported by the next END marker.
func (t *MCPTerminal) TermPrintLine(style terminal.Style, s string) {
	if style == terminal.StyleEcho {
		return
	}
	if t.silenced && style != terminal.StyleError {
		return
	}
	if style == terminal.StyleError {
		t.err = true
		fmt.Fprintf(t.output, "%sERR%s %s\n", rs, rs, s)
		return
	}
	fmt.Fprintf(t.output, "%s\n", s)
}

// TermRead closes the previous command's frame, opens the next, and reads a
// command line from stdin.
//
// The prompt reflects emulation state *after* the command that just finished
// (the next instruction to execute), so it is emitted as a PROMPT line within
// that command's frame. This is what makes the result of STEP and other
// execution commands visible over the protocol.
//
// # Errors
//
// Returns the underlying read error (including io.EOF when stdin is closed),
// which signals the debugger loop to terminate.
func (t *MCPTerminal) TermRead(prompt terminal.Prompt, events *terminal.ReadEvents) (string, error) {
	// The debugger asks for confirmation ("really quit?") when an interrupt
	// arrives at the idle prompt. A machine driver never wants to quit on an
	// interrupt (it uses interrupts only to halt a running emulation), so
	// decline without emitting a frame, keeping the protocol stream in sync.
	if prompt.Type == terminal.PromptTypeConfirm {
		return "n\n", nil
	}

	if t.silenced {
		return "", nil
	}

	// report the post-command position, then close the frame for the command
	// that just finished (seq 0 is startup).
	if content := strings.TrimSpace(prompt.Content); content != "" {
		fmt.Fprintf(t.output, "%sPROMPT%s %s\n", rs, rs, content)
	}
	t.endFrame()

	// open a frame and read a command. Pseudo-commands (e.g. SCREENSHOT) are
	// handled here and given their own frame; the loop then reads the next
	// command. The first real command is returned to the debugger.
	for {
		t.seq++
		fmt.Fprintf(t.output, "%sBEGIN seq=%d%s\n", rs, t.seq, rs)

		line, err := t.input.ReadString('\n')
		if err != nil {
			// surface a pending interrupt rather than the read error if both occur.
			select {
			case sig := <-events.Signal:
				return "", events.SignalHandler(sig)
			default:
			}
			return "", err
		}

		// a signal may have arrived while we were blocked on Read.
		select {
		case sig := <-events.Signal:
			return "", events.SignalHandler(sig)
		default:
		}

		if path, ok := parseScreenshot(line); ok {
			t.screenshot(path)
			t.endFrame()
			continue
		}

		return line, nil
	}
}

// endFrame writes the END marker for the current command and resets the error flag.
func (t *MCPTerminal) endFrame() {
	fmt.Fprintf(t.output, "%sEND seq=%d err=%s%s\n", rs, t.seq, boolField(t.err), rs)
	t.err = false
}

// parseScreenshot reports whether a line is a SCREENSHOT pseudo-command and, if
// so, returns the requested output path.
func parseScreenshot(line string) (string, bool) {
	fields := strings.Fields(line)
	if len(fields) == 0 || !strings.EqualFold(fields[0], screenshotCmd) {
		return "", false
	}
	return strings.TrimSpace(strings.TrimPrefix(strings.TrimSpace(line), fields[0])), true
}

// screenshot saves the current frame to path, reporting the result on the protocol.
func (t *MCPTerminal) screenshot(path string) {
	if t.Capture == nil {
		t.TermPrintLine(terminal.StyleError, "screenshots are not available")
		return
	}
	if path == "" {
		t.TermPrintLine(terminal.StyleError, "SCREENSHOT requires an output path")
		return
	}
	w, h, err := t.Capture.Save(path)
	if err != nil {
		t.TermPrintLine(terminal.StyleError, err.Error())
		return
	}
	t.TermPrintLine(terminal.StyleFeedback, fmt.Sprintf("screenshot %dx%d saved to %s", w, h, path))
}

// TermReadCheck reports no buffered input; the debugger reads on demand.
func (t *MCPTerminal) TermReadCheck() bool {
	return false
}

// IsInteractive reports false; this terminal is always machine-driven.
func (t *MCPTerminal) IsInteractive() bool {
	return false
}

// IsRealTerminal reports true so that an interrupt during a running emulation
// halts it (returning to the debugger) instead of quitting the process. The
// debugger treats a non-real terminal's interrupt as an immediate exit; a real
// terminal's interrupt halts a run and otherwise prompts for quit confirmation,
// which TermRead auto-declines. Input and output are still pipes.
func (t *MCPTerminal) IsRealTerminal() bool {
	return true
}

// boolField renders a bool as the "0"/"1" used in protocol marker fields.
func boolField(b bool) string {
	if b {
		return "1"
	}
	return "0"
}
