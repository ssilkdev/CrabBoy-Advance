#!/usr/bin/env python3
"""Generates a minimal, valid GBA ROM for smoke-testing the emulator.

The ROM contains real ARM7TDMI machine code that:
  1. sets DISPCNT to Mode 3 with BG2 enabled,
  2. writes 256 red pixels into VRAM, and
  3. spins in an infinite loop.

That exercises the CPU decoder, the MMU IO/VRAM paths and the PPU bitmap
renderer, so a rendered frame is real evidence the pipeline works -- not just
that the binary starts.

    python3 scripts/make_test_rom.py out.gba
"""

import struct
import sys

# GBA cartridge header offsets (see GBATEK).
TITLE_OFF = 0xA0
FIXED_VALUE_OFF = 0xB2   # must be 0x96
CHECKSUM_OFF = 0xBD      # header checksum byte

# ARM instruction stream placed at ROM offset 0xC0 (entry branch target).
CODE = [
    0xE3A00301,  # mov   r0, #0x04000000   ; IO base -> DISPCNT
    0xE3A01B01,  # mov   r1, #0x400
    0xE2811003,  # add   r1, r1, #3        ; 0x403 = Mode 3 | BG2 enable
    0xE1C010B0,  # strh  r1, [r0]
    0xE3A02406,  # mov   r2, #0x06000000   ; VRAM base
    0xE3A0301F,  # mov   r3, #31           ; BGR555 red
    0xE3A04C01,  # mov   r4, #0x100        ; 256 pixels
    0xE1C230B0,  # strh  r3, [r2]          ; loop: store pixel
    0xE2822002,  # add   r2, r2, #2
    0xE2544001,  # subs  r4, r4, #1
    0x1AFFFFFB,  # bne   loop
    0xEAFFFFFE,  # b     .                 ; halt
]


def build_rom() -> bytes:
    rom = bytearray(0x400)

    # Entry point: branch from 0x00 to the code at 0xC0.
    # ARM branch offset = (target - pc - 8) / 4 = (0xC0 - 0 - 8) / 4 = 0x2E.
    rom[0:4] = struct.pack("<I", 0xEA000000 | 0x2E)

    rom[TITLE_OFF:TITLE_OFF + 12] = b"CRABTEST\x00\x00\x00\x00"
    rom[0xAC:0xB0] = b"CBTE"   # game code
    rom[0xB0:0xB2] = b"01"     # maker code
    rom[FIXED_VALUE_OFF] = 0x96

    # Header checksum: -(sum of bytes 0xA0..0xBC) - 0x19, per GBATEK.
    chk = 0
    for b in rom[0xA0:0xBD]:
        chk = (chk - b) & 0xFF
    rom[CHECKSUM_OFF] = (chk - 0x19) & 0xFF

    for i, insn in enumerate(CODE):
        off = 0xC0 + i * 4
        rom[off:off + 4] = struct.pack("<I", insn)

    return bytes(rom)


def main() -> int:
    out = sys.argv[1] if len(sys.argv) > 1 else "test_rom.gba"
    data = build_rom()
    with open(out, "wb") as fh:
        fh.write(data)
    print(f"Wrote {len(data)} byte test ROM to {out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
