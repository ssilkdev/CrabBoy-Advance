# Grass-battle crash: root cause and fix

> **SUPERSEDED — this document's conclusion is wrong.**
>
> The two fixes below are real bugs and worth keeping, but **neither caused the
> grass-battle crash**. After both landed, the black-frame count across the
> transition was unchanged (20 of 80 frames).
>
> The actual root cause was `MSR SPSR_<fields>, Rm` never being decoded in
> `src/gba/cpu/arm.rs` (bit 22, the R bit, was wrongly constrained to 0 by the
> decode mask), plus the T-bit being masked out of SPSR writes. IRQ returns
> therefore lost the saved THUMB state and resumed THUMB code in ARM mode.
> Fixing it took black frames from 20/80 to 2/80.
>
> See **`docs/GRASS_BATTLE_INVESTIGATION.md`** for the full analysis, evidence
> chain and regression tests. Read this file only as background on Fixes 1 & 2.

**Symptom:** the game freezes when a wild battle starts in tall grass. A
diagnostic report captured at that moment shows `overall_grade: "Critical"`,
an APU "stuck note" anomaly, and — the real tell — a flight recorder full of
events whose `pc` values sit inside **I/O register space** (`0x04000214`,
`0x040002EC`, `0x040003F4`, ...).

The audio anomaly is a *consequence*, not the cause: the CPU stopped running
the game's code, so nothing ever updated the sound channel, and PSG Ch3 held
one note for 1135 frames.

## What was actually wrong

Two independent defects produce this same signature.

### 1. `save_state()` silently dropped all hardware controller state

`Gba::save_state()` serialized CPU registers, EWRAM, IWRAM, VRAM, palette, OAM
and the raw `io_regs` array — then stopped. The `Mmu`'s live hardware structs
were never written:

- `ime`, `ie`, `if_reg`, `waitcnt`
- all four DMA channels (including the internal working copies)
- all four timers

Because these live in their own structs rather than in `io_regs`, restoring a
state produced a machine with **interrupts globally disabled and every DMA
channel cleared**. Measured on a real Emerald save:

```
            before -> after
IME       : true  -> false
IE        : 0x0005 -> 0x0000
DMA1 sad  : 0x030066D0 -> 0x00000000   cnt_h 0xB600 -> 0x0000
DMA2 sad  : 0x03006D00 -> 0x00000000   cnt_h 0xB600 -> 0x0000
TM0 count : 64335 ->     0
```

Pokémon Emerald drives its audio mixer from DMA1/DMA2 (FIFO_A/FIFO_B) on a
timer IRQ. Restore a state and that entire arrangement is gone.

**Fix:** a versioned `CBA2` tail appended to the state blob carrying IME/IE/IF/
WAITCNT, all DMA channel registers plus internal counters, and all timer state.
Old (v1) state files still load — they simply have nothing to restore.

### 2. The undefined-instruction vector fell through into I/O space

`arm.rs` correctly raises an Undefined Instruction exception for unrecognized
encodings and sets `PC = 0x00000004`. But the HLE BIOS only populated its IRQ
dispatcher at `0x18`; the rest of the 16 KiB BIOS was zeros.

`0x00000000` decodes as `andeq r0, r0, r0` — a harmless fall-through. So the
CPU would execute nothing at `0x04`, then `0x08`, `0x0C` ... walking all the
way up through empty BIOS and straight into I/O space, exactly as the flight
recorder shows.

The captured save state is frozen mid-failure and proves it:

```
PC   = 0x040002EC  [IO-REGS (NOT EXECUTABLE)]
CPSR = 0x8000009B  -> mode 0x1B = UND (undefined-instruction mode)
r14  = 0x00000028  -> inside the BIOS IRQ stub
user IRQ handler ptr [0x03007FFC] = 0x04000200   <- garbage
```

**Fix:** `0x04` now contains `subs pc, lr, #4`, which is what real hardware
does — restore CPSR from SPSR_und and resume after the faulting instruction.
A bad opcode stays local instead of destroying the machine.

The SWI vector at `0x08` is deliberately left unpopulated: SWIs are intercepted
in `arm.rs`/`thumb.rs` and serviced by `handle_swi`, so the CPU never vectors
there. Stubbing it would be dead code implying otherwise.

## Regression tests

| Test | Guards |
|---|---|
| `save_state_roundtrip_preserves_hardware_state` | IME/IE/DMA/timers survive a round trip |
| `restored_state_keeps_executing_normally` | a restored state runs 300 frames without PC entering I/O space |
| `undefined_instruction_does_not_escape_into_io_space` | a bad opcode can't walk into I/O space |
| `bios_exception_vectors_are_populated` | reachable vectors are never zero |

The round-trip test was verified to genuinely catch the bug by temporarily
reverting the fix — it fails with
`save_state() silently drops: IME, IE, DMA source addresses, DMA control`.

## Honest caveats

- **Your specific save state cannot be repaired.** It was captured *after* the
  CPU had already derailed (PC already in I/O space, CPSR already in UND mode).
  The fixes prevent this from happening again; they can't reconstruct the
  register state that was lost before the snapshot. Start from your in-game
  `.sav`, not that `.state`.
- **The exact undefined instruction was not identified.** I proved the escape
  mechanism and closed it, but I did not trace which opcode the battle
  transition executed that the decoder rejects. If battles still misbehave
  after this fix, that decoder gap is the next thing to chase — it would mean
  a genuinely unimplemented instruction, not just bad recovery from one.
