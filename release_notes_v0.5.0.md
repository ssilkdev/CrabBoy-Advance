# 🦀 Crabboy-Advance-0.5.0 — Linux Support & CPU Correctness Release

This release adds a native Linux build and fixes an ARM instruction-decode bug
that had been silently corrupting interrupt returns.

---

## 🐧 Linux Support

CrabBoy Advance now builds and ships for Linux x86_64 alongside Windows.

- Portable `.tar.gz` with the binary, manual, README and LICENSE
- Desktop integration: `.desktop` entry, MIME type registration for `.gba`
  files, and a per-user `install.sh`
- Auto-updater works on Linux, resolving the correct per-OS release asset
- Reproducible packaging (`--sort=name`, fixed mtime/owner), so an identical
  source tree yields a byte-identical tarball and a stable published checksum

## 🕹️ Critical CPU Fix — `MSR SPSR` was never decoded

**Symptom:** entering a wild battle in tall grass in Pokémon Emerald froze the
game. The transition animated for about two frames, then the screen went black
permanently, with occasional torn scanline garbage and a stuck audio note.

**Cause:** two defects in the ARM `MSR` path.

1. `MSR` was gated on `(instr & 0x0D60_F000) == 0x0120_F000`. In the encoding
   `cond 00 I 10 R 10 field_mask 1111 <operand>`, bit 22 is **R**
   (0 = CPSR, 1 = SPSR) and is an *operand*, so it must be don't-care. That mask
   constrained it to 0, so every `msr spsr_*` form failed the test, fell through
   to the data-processing decoder, and decoded as `CMN` with S=0 — **a silent
   no-op**. `MSR SPSR` was effectively unimplemented, and nothing logged or
   faulted to say so.

2. The control-byte write mask dropped bit 5 (the T/Thumb bit) for **both** CPSR
   and SPSR. That is correct for CPSR (writing T via `MSR` is UNPREDICTABLE on
   ARMv4T) but wrong for SPSR, which is an ordinary data register there and must
   accept all 32 bits.

**Why it broke battles:** Emerald's interrupt handler saves SPSR on entry, runs
the game's handler in System mode (clobbering `spsr_irq`), then restores it with
`msr spsr_fc, r0` before returning. The save worked; the restore did nothing. So
the return restored a stale System-mode SPSR with T=0, and interrupted **THUMB**
code resumed in **ARM** state — after which the CPU decoded a THUMB branch pair
as a bogus `SWI 0x32`, vectored into unpopulated BIOS, and ran off into
uninitialised ROM.

Every visible symptom was downstream of this: the window registers went to zero
because the game's code had stopped running, and the "stuck note" audio anomaly
was simply nothing updating the sound channel.

**Impact:** this affects any game whose interrupt handler saves and restores
SPSR — which is the standard pattern, not an Emerald quirk. Other titles were
likely hitting the same corruption in less obvious ways.

**Measured:** black frames across the battle transition dropped from **20 of 80
to 2 of 80**, and the remaining two are the legitimate mid-transition blank. No
derail in 169,417,129 instructions.

## 💾 Save States — Hardware State Now Persisted

`save_state()` serialized CPU registers and memory but stopped short of the
MMU's live controller structs, which sit outside the `io_regs` array. **IME, IE,
IF, WAITCNT, all four DMA channels and all four timers were silently dropped.**

A restored state therefore resumed with interrupts globally disabled and every
DMA channel cleared. Measured on a real save: `IME true → false`,
`IE 0x0005 → 0x0000`, `DMA1 sad 0x030066D0 → 0x00000000`, `TM0 count 64335 → 0`.
Since Emerald drives its audio mixer from DMA1/DMA2 on a timer IRQ, loading a
state broke sound outright.

Fixed with a versioned `CBA2` tail. **Existing v1 save states still load** —
they simply have no extra hardware state to restore.

## 🖥️ PPU — No More Skipped Scanlines

`Ppu::step()` advanced `vcount` at most once per call, so a long DMA burst
handing it several scanlines' worth of cycles silently dropped lines — measured
**46 of 160 visible scanlines never rendered** at a 2464-cycle chunk. It now
consumes its full cycle budget in a loop.

Scanline rendering also moved from the end of the scanline to the **start of
HBlank**, before the HBlank IRQ handler runs, since games program the next
line's registers from that handler.

## 🛡️ Hardening

- The undefined-instruction vector at `0x04` now contains `subs pc, lr, #4`
  instead of zeros. Previously a bad opcode would walk up through empty BIOS and
  into I/O space, turning one bad instruction into a dead machine.
- `SHA256SUMS.txt` listed `icon.png` while the tarball ships
  `assets/icon_256.png`. Since the auto-updater validates downloads against this
  file, that entry could never be verified. It now names the file actually
  shipped.

## 🤖 AI Agent Player

New optional mode that drives the emulator from a local OpenAI-compatible vision
LLM — the agent observes frames and issues key presses. Ships with a mock vision
server so it can be exercised offline. See `docs/AI_AGENT_PLAYER.md`.

---

## Verification

**155 automated tests pass** (up from 149), with zero build warnings.

The `MSR SPSR` regression tests were each verified to have teeth by reverting
the fixes independently: reverting the decode mask fails 3 of 4 tests, reverting
the T-bit mask fails 2 of 4. One test specifically guards that the fix did *not*
make CPSR writable in T.

Full evidence chain, including the two wrong turns taken along the way, is in
[`docs/GRASS_BATTLE_INVESTIGATION.md`](docs/GRASS_BATTLE_INVESTIGATION.md).

### Known gap

The SWI vector at `0x08` is still unstubbed. `handle_swi` only HLE-services
`swi_num <= 0x2A` and otherwise vectors there, where zeros fall through into the
IRQ dispatcher in the wrong CPU mode. With the `MSR SPSR` fix in place the
garbage SWI is never executed, so this is defence-in-depth rather than a live
bug — documented rather than quietly left.

## 💾 Downloads & Verification

| Asset | Platform |
|---|---|
| `crabboy-advance-v0.5.0-linux-x86_64.tar.gz` | Linux x86_64 (portable) |
| `crabboy-advance-linux-x86_64` | Linux x86_64 (bare binary, used by auto-update) |
| `SHA256SUMS.txt` | Checksums for every asset |

Verify a download with:

```bash
sha256sum -c SHA256SUMS.txt
```

*CrabBoy Advance is an open-source Game Boy Advance emulation project developed
in pure Rust.*
