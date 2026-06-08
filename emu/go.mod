module github.com/aovestdipaperino/stella-mcp/emu

go 1.25.0

require github.com/jetsetilly/gopher2600 v0.56.1

require (
	github.com/go-audio/audio v1.0.0 // indirect
	github.com/go-audio/riff v1.0.0 // indirect
	github.com/go-audio/wav v1.1.0 // indirect
	github.com/hajimehoshi/go-mp3 v0.3.4 // indirect
	golang.org/x/image v0.38.0 // indirect
)

// Gopher2600's published module tree exceeds Go's module-zip size limit
// (its hardware/cpu/tests fixtures are ~1GB), so it cannot be fetched via the
// module proxy. We build against a local clone instead; scripts/build-emu.sh
// creates third_party/gopher2600 (with the test fixtures pruned).
replace github.com/jetsetilly/gopher2600 => ../third_party/gopher2600
