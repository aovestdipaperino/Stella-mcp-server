; colorbars.asm - a minimal NTSC test kernel for stella-mcp's CI smoke test.
;
; It renders a vertical colour gradient by writing the scanline counter to the
; background colour register, producing a frame that is unambiguously non-blank.
; Original, public-domain test code; assembles with DASM and the bundled vcs.h.

	processor 6502
	include "vcs.h"

	org $f000

Reset:
	sei
	cld
	ldx #0
	txa
Clear:
	sta 0,x			; zero RAM and TIA registers
	inx
	bne Clear

Frame:
	lda #2
	sta VSYNC		; begin vertical sync
	sta VBLANK
	sta WSYNC
	sta WSYNC
	sta WSYNC
	lda #0
	sta VSYNC		; end vertical sync

	ldx #37			; 37 lines of vertical blank
VBlank:
	sta WSYNC
	dex
	bne VBlank
	lda #0
	sta VBLANK		; enable the display

	ldx #192		; 192 visible lines, colour = line counter
Picture:
	stx COLUBK
	sta WSYNC
	dex
	bne Picture

	lda #2
	sta VBLANK		; blank the display
	ldx #30			; 30 lines of overscan
Overscan:
	sta WSYNC
	dex
	bne Overscan
	jmp Frame

	org $fffc
	.word Reset
	.word Reset
