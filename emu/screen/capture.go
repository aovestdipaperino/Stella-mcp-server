// Package screen captures the emulator's television output as PNG images.
//
// A Capture is registered as a television.PixelRenderer on the headless
// debugger's TV. The TV has no display, but it still produces a full frame of
// pixel signals each frame; Capture keeps the most recent frame and can encode
// it to a PNG on demand, which is how the MCP server takes screenshots without
// a windowed emulator.
package screen

import (
	"errors"
	"image"
	"image/color"
	"image/png"
	"os"
	"sync"

	"github.com/jetsetilly/gopher2600/hardware/television/frameinfo"
	"github.com/jetsetilly/gopher2600/hardware/television/signal"
	"github.com/jetsetilly/gopher2600/hardware/television/specification"
)

// errNoFrame is returned when a screenshot is requested before any frame has
// been rendered (the emulation must be stepped or run first).
var errNoFrame = errors.New("no frame rendered yet; step or run the emulation first")

// hScale doubles pixel width so the saved image has a roughly correct aspect
// ratio (each TIA colour clock is half the width of a scanline pixel).
const hScale = 2

// Capture is a television.PixelRenderer that retains the latest frame so it can
// be saved as a PNG. It is safe for concurrent use.
type Capture struct {
	mu    sync.Mutex
	info  frameinfo.Current
	sig   []signal.SignalAttributes
	last  int
	valid bool
}

// New returns an empty Capture ready to be registered with a television.
func New() *Capture {
	return &Capture{}
}

// NewFrame records the geometry of the frame about to be drawn.
func (c *Capture) NewFrame(info frameinfo.Current) error {
	c.mu.Lock()
	c.info = info
	c.mu.Unlock()
	return nil
}

// NewScanline is unused; the whole frame is captured in SetPixels.
func (c *Capture) NewScanline(int) error { return nil }

// SetPixels retains a copy of the frame's pixel signals.
//
// When the emulation halts, the debugger forces a display refresh of the
// current (often freshly-reset, empty) frame, reporting last==0. Those renders
// are ignored so the most recent fully-drawn frame is kept for screenshots.
func (c *Capture) SetPixels(sig []signal.SignalAttributes, last int) error {
	if last == 0 {
		return nil
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if cap(c.sig) < len(sig) {
		c.sig = make([]signal.SignalAttributes, len(sig))
	}
	c.sig = c.sig[:len(sig)]
	copy(c.sig, sig)
	c.last = last
	c.valid = true
	return nil
}

// Reset discards the retained frame.
func (c *Capture) Reset() {
	c.mu.Lock()
	c.valid = false
	c.mu.Unlock()
}

// EndRendering performs no work.
func (c *Capture) EndRendering() error { return nil }

// Save encodes the most recent frame to a PNG file at path.
//
// It returns an error if no frame has been rendered yet (step or run the
// emulation first) or if the file cannot be written.
func (c *Capture) Save(path string) (int, int, error) {
	c.mu.Lock()
	img, w, h, err := c.render()
	c.mu.Unlock()
	if err != nil {
		return 0, 0, err
	}

	f, err := os.Create(path)
	if err != nil {
		return 0, 0, err
	}
	defer f.Close()
	if err := png.Encode(f, img); err != nil {
		return 0, 0, err
	}
	return w, h, nil
}

// render builds an RGBA image of the visible area from the retained signals.
// The caller must hold the mutex.
func (c *Capture) render() (*image.RGBA, int, int, error) {
	if !c.valid {
		return nil, 0, 0, errNoFrame
	}

	spec := c.info.Spec
	top := c.info.VisibleTop
	bottom := c.info.VisibleBottom
	if bottom <= top {
		// no visible bounds yet: fall back to the spec's nominal extent.
		top = spec.AtariSafeVisibleTop
		bottom = spec.AtariSafeVisibleBottom
	}
	if bottom <= top {
		return nil, 0, 0, errNoFrame
	}

	height := bottom - top
	width := specification.ClksVisible
	img := image.NewRGBA(image.Rect(0, 0, width*hScale, height))

	for i := 0; i <= c.last && i < len(c.sig); i++ {
		scanline := i / specification.ClksScanline
		clock := i % specification.ClksScanline
		if scanline < top || scanline >= bottom || clock < specification.ClksHBlank {
			continue
		}
		sig := c.sig[i]
		var col color.RGBA
		if sig.Color == signal.ZeroBlack {
			col = color.RGBA{A: 0xff}
		} else {
			col = spec.GetColor(sig.Color)
			col.A = 0xff
		}
		x := (clock - specification.ClksHBlank) * hScale
		y := scanline - top
		for dx := 0; dx < hScale; dx++ {
			img.SetRGBA(x+dx, y, col)
		}
	}

	return img, width * hScale, height, nil
}
