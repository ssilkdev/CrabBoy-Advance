# Pokémon Emerald grass-battle crash — investigation notes

**Status: RESOLVED.** Root cause found and fixed: `MSR SPSR_<fields>, Rm` was
never decoded, so IRQ returns lost the saved THUMB bit. Black frames across the
battle transition dropped from **20/80 to 2/80**, and the remaining two are the
legitimate mid-transition blank. The battle screen now renders.

Read "Root cause" for the answer; the earlier sections are kept as the trail
that led there.

---

## The symptom

Entering a wild battle in tall grass (Route 101) breaks the display. The
transition animates correctly for ~2 frames, then the screen goes black and
stays black, with intermittent frames of torn green scanline garbage.

The user's diagnostic report (`~/Documents/crabboy_diagnostic_report.json`)
graded the run "Critical" and flagged an APU anomaly:

```
PSG Ch3 (Wave): Stuck Note Anomaly! Continuous static playback for 1135 frames
```

**The audio anomaly is a consequence, not the cause.** Nothing was updating the
sound channel because the game had stopped making progress.

---

## Reproduction (deterministic, no GUI needed)

`tests/grass_battle_live.rs` boots the real ROM with its adjacent `.sav`,
mashes through the title/continue screens, then walks up/down in grass until
the encounter fires, dumping a PNG per step to `/tmp/grass/`.

```bash
GBA_TEST_ROM="/home/ssilk/Downloads/Pokemon - Emerald Version (USA, Europe).gba" \
WALK_STEPS=80 \
cargo test --release --test grass_battle_live -- --ignored --nocapture
```

The transition begins at **step 052** every time.

Checking frames for corruption:

```bash
cd /tmp/grass && python3 -c "
from PIL import Image; import glob
for f in sorted(glob.glob('step_*.png')):
    n=len(Image.open(f).convert('RGB').getcolors(maxcolors=10**6))
    print(f, n, 'BLACK' if n==1 else '')
"
```

| | black frames (of 80) | first |
|---|---|---|
| baseline | 20 | step 052 |
| after Fixes 1–3 | 20 | step 052 |
| **after the MSR SPSR fix** | **2** | step 055 |

---

## Root cause

### `MSR SPSR_<fields>, Rm` was never decoded

`src/gba/cpu/arm.rs` gated MSR on:

```rust
if (instr & 0x0D60_F000) == 0x0120_F000 && ...
```

The MSR encoding is `cond 00 I 10 R 10 field_mask 1111 <operand>`. Bit 22 is
**R** (0 = CPSR, 1 = SPSR) and is an *operand*, so it must be don't-care. The
mask `0x0D60_F000` constrained it to **0**, so every `msr spsr_*` form failed
the test, fell through to the data-processing decoder, and decoded as `CMN` with
S=0 — **a silent no-op**. MSR SPSR was effectively unimplemented; nothing logged
or faulted.

Correct mask (`0x0DB0_F000`) checks the genuinely fixed bits 27..26, 24, 23, 21,
20 and 15..12, leaving bits 25 (I) and 22 (R) free.

### The T-bit was also masked out of SPSR writes

The control-byte write mask was `0xDF` for both CPSR and SPSR — i.e. bit 5 (T)
dropped. That is right for CPSR (writing T via MSR is UNPREDICTABLE on ARMv4T
and would switch execution state behind the pipeline) but **wrong for SPSR**,
which here is an ordinary data register and must accept all 32 bits.

Now: `mask |= if r { 0x0000_00FF } else { 0x0000_00DF };`

### How that breaks the battle

Emerald's `IntrMain` (in IWRAM at `0x03002750`) does, on every interrupt:

```
mrs  r0, spsr          ; save the interrupted state
...                    ; drop to System mode, re-enable IRQs, run the
                       ; game's handler -- this CLOBBERS spsr_irq
msr  spsr_fc, r0       ; 0xE169F000 -- restore it
bx   lr                ; back to the BIOS stub -> subs pc, lr, #4
```

The save (`mrs`) worked. The restore (`msr spsr_fc, r0`) did nothing. So the
BIOS epilogue `subs pc, lr, #4` restored a **stale** SPSR_irq — the System-mode
CPSR left over from the handler, with **T=0**.

The interrupted code was THUMB at `0x080008CE`. It resumed as **ARM** at
`0x080008CC`. From the derail trace:

```
ROM 080008CC A op=D0FA2800 cpsr=6000001F  <- two THUMB halfwords read as one ARM word
ROM 080008D0 A op=4700BC01
ROM 080008D4 A op=030022C0                <- literal pool, executed as code
...
ROM 080008FC A op=FF32F2DF                <- THUMB `bl` pair; as ARM this is
                                          ;   cond=0xF, bits 27..24 = 0xF
```

`0xFF32F2DF` passed `arm.rs`'s SWI test `(instr & 0x0F000000) == 0x0F000000`,
yielding SWI **0x32** — outside the HLE range (`<= 0x2A`) — so `handle_swi`
called `cpu.trigger_swi()`. That enters Supervisor mode and sets `PC = 0x08`.
The HLE BIOS leaves `0x08..0x18` as zeros (`andeq r0,r0,r0`), so execution fell
through into the **IRQ dispatcher at `0x18` while in SVC mode**, on `sp_svc`,
and from there into uninitialised ROM at `0x08C00008` — the "stuck loop".

Every downstream symptom follows: the game's code stopped running, so nothing
refreshed the WIN0H table, so `WIN0H=0` / `WINOUT=0` blanked the screen, so the
APU held one note for 1135 frames.

### Evidence chain

| Probe | Finding |
|---|---|
| `tests/derail_probe.rs` | PC first enters `0x08C00008` from `0x080004BC`; ring buffer shows the IRQ dispatcher at `0x18` entered in **mode 0x13 (SVC)**, not 0x12 (IRQ) |
| `tests/swi_vector_probe.rs` | Only SWIs 0x0B/0x0C/0x0F are ever executed legitimately (all HLE-handled); PC reaches the SWI vector `0x08` **exactly once**; IRQ dispatcher entered once in SVC mode |
| `tests/swi_trip_probe.rs` | Culprit isolated: `pc=0x080008FC op=0xFF32F2DF` → decodes as SWI 0x32 → `trigger_swi` → `PC=0x08`. Trace above it shows THUMB code being run as ARM |
| `tests/spsr_tbit_probe.rs` | **1** IRQ return where SPSR had T=0 but the return target `0x080008CE` is halfword-aligned (THUMB-only). After the fix: **0** |

### After the fix

```
derail_probe      : no derail observed in 169,417,129 instructions
spsr_tbit_probe   : T-bit mismatches: 0
grass_battle_live : 2/80 black frames (was 20/80), exception-mode frames: 0
```

Frames 052–054 shrink the window, 055–056 are black, 057+ fade the battle scene
in with the wild Pokémon, trainer and text box — a coherent transition, visually
confirmed.

### Regression tests

`tests/msr_spsr_tests.rs`, all four verified to have teeth by reverting each
fix independently:

| Test | Guards |
|---|---|
| `msr_spsr_register_form_is_decoded_at_all` | the decode mask (fails on the 0x0D60_F000 revert) |
| `msr_spsr_preserves_the_thumb_bit` | the SPSR control-byte mask (fails on the 0xDF revert) |
| `msr_cpsr_still_masks_the_thumb_bit` | that the fix did **not** make CPSR writable in T |
| `irq_return_resumes_thumb_after_spsr_restore` | end-to-end IntrMain epilogue shape; fails on either revert |

Reverting the decode mask alone fails 3 of 4; reverting only the T-bit mask
fails 2 of 4.

---

## The trail (what was proven before the root cause)

Kept because the reasoning is reusable, and because two of the three conclusions
here were **wrong turns** worth remembering.

### The game stalls, it does not crash

`tests/stuck_probe.rs` samples the PC over 200k instructions once the screen is
black:

```
distinct PCs in 200k instructions: 140
  0x08C0000C   17372 (8.7%)
  0x08C00028   17357 (8.7%)
  ... (9 addresses, ~8.6% each)
```

A tight 9-instruction loop at `0x08C0000C..0x08C0002C`, 12.00 MB into a 16 MB
ROM. `tests/loop_disasm.rs` then showed those words are **not valid ARM** — data
being executed:

```
0x08C00020: 0x03007903
0x08C00024: 0x79911177
cpsr=0x6000001F  -> mode 0x1F = SYSTEM, ARM state
r14 = 0x080004BF -> return address of whatever derailed
r0  = 0xE25EF004 <- the `subs pc, lr, #4` opcode, held as data
r6  = 0xE92D500F <- BIOS IRQ stub word
```

The BIOS stub words in r0/r6/r8/r12 looked like evidence that something walked
the BIOS as a data table. **That reading was wrong** — they are simply what the
BIOS epilogue and the runaway loads left behind. The real anchor was `r14`.

### The window registers are what blanks the screen — a symptom, not the bug

`tests/ppu_transition_probe.rs`:

```
step 51: win0h=0x00FF winout=0x0101  nonblack=38400
step 52: win0h=0x0000 winout=0x0000  nonblack=0
```

`tests/win0h_probe.rs` showed frames 12–13 animating the shrinking circle
correctly, then all zeros from frame 14 forever. `tests/loop_disasm.rs` proved
the **DMA source table at `0x020394E8` was intact** (`0x02F1`/`0x00EE`
alternating). That was read as "the fault is in the DMA path" — **also wrong**.
The table was stale-but-valid; nothing was consuming it because the CPU was
gone.

> Caveat on `win0h_probe`: the flight recorder holds only 512 entries
> (`FLIGHT_RECORDER_CAPACITY`), so "63/64 writes" is capped by buffer size, not
> a true per-frame count. The reliable signal is the *values*, not the counts.

### The lesson

Three probes pointed at the PPU/DMA and all three were downstream of the actual
fault. What broke it open was tracing **backwards from the derail** with a ring
buffer (`derail_probe.rs`) instead of forwards from the symptom. When PC is in
the weeds, `r14` at the moment of derail is worth more than any amount of
peripheral state.

---

## Fixes made along the way (real bugs, none of them this one)

Two genuine defects were found and fixed while hunting, with regression tests
proven to catch them. **Neither changed the black-frame count.**

### Fix 1 — `save_state()` dropped all hardware controller state

`Gba::save_state()` serialized CPU registers, EWRAM, IWRAM, VRAM, palette, OAM
and the raw `io_regs` array, then stopped. The `Mmu`'s live controller structs
live outside `io_regs` and were never written:

- `ime`, `ie`, `if_reg`, `waitcnt`
- all four DMA channels (including `internal_sad`/`internal_dad`/`internal_count`)
- all four timers

Measured round-trip loss on a real save:

```
IME       : true  -> false
IE        : 0x0005 -> 0x0000
DMA1 sad  : 0x030066D0 -> 0x00000000   cnt_h 0xB600 -> 0x0000
TM0 count : 64335 ->     0
```

A restored state resumed with interrupts globally disabled and DMA cleared.

**Fix:** versioned `CBA2` tail appended to the state blob (`STATE_V2_MAGIC` in
`src/gba/mod.rs`). v1 states still load — they simply have nothing to restore.

Tests: `tests/save_state_roundtrip.rs`. Verified to have teeth by temporarily
disabling the writer.

> Note: the second test in that file, `restored_state_keeps_executing_normally`,
> **passed even with the bug reintroduced**. Only the round-trip assertion
> actually guards this.

### Fix 2 — undefined-instruction vector fell through into I/O space

`arm.rs` raises the UND exception and sets `PC = 0x00000004`, but the HLE BIOS
only populated its IRQ dispatcher at `0x18`. The rest of the 16 KiB BIOS was
zeros, and `0x00000000` decodes as `andeq r0,r0,r0` — a fall-through. The CPU
would walk `0x04, 0x08, 0x0C…` up through empty BIOS and into I/O space.

**Fix:** `0x04` now contains `subs pc, lr, #4`.

Tests: `tests/exception_vector_tests.rs`.

This matched the user's captured save state (`PC=0x040002EC`, `CPSR` mode UND),
but a probe over 400 frames of normal play recorded **zero** UND exceptions. The
user's `.state` was captured after some *other* derailment — almost certainly a
later stage of the same MSR SPSR failure.

> **Related, still open:** the same fall-through hazard exists at the **SWI
> vector `0x08`**, and this investigation proved it is reachable in practice.
> See "Remaining work".

### Fix 3 (partial) — PPU could skip scanlines

`Ppu::step(cycles)` used a single `if cycle_in_scanline >= SCANLINE_CYCLES`,
advancing `vcount` at most once per call. A long DMA burst handing it multiple
scanlines' worth of cycles silently dropped lines — measured **46 of 160 visible
scanlines never rendered** at a 2464-cycle chunk.

**Fix:** `step()` consumes its budget in a loop, chunked to the next
HDraw/HBlank boundary. Also moved `render_scanline()` from the *end* of the
scanline to the **start of HBlank**, before the HBlank IRQ handler runs, since
games set up line N+1's registers from that handler.

Tests: `tests/ppu_scanline_tests.rs`, both verified to have teeth.

---

## Remaining work

1. **Stub the SWI vector at `0x08`.** This investigation proved the CPU *can*
   reach it: `handle_swi` only HLE-services `swi_num <= 0x2A` and otherwise
   calls `cpu.trigger_swi()`, which sets `PC = 0x08`. Zeros there fall through
   into the IRQ dispatcher at `0x18` **in SVC mode**, which is how a single bad
   opcode became a total derail. The comment in `mmu/mod.rs` claiming "the CPU
   never vectors there" is **false** and should be corrected. A `movs pc, lr`
   stub (or routing out-of-range SWIs to the UND path) would contain it. Note
   this is defence-in-depth, not the root cause — with MSR SPSR fixed, the
   garbage SWI 0x32 is never executed.

2. **Audit the other ARM decode masks.** The MSR bug was a don't-care bit
   wrongly constrained, failing *silently* into a different instruction. Worth
   checking MRS, MSR-immediate, and the multiply/halfword-transfer group for the
   same class of error. A decoder that logged unmatched-but-plausible encodings
   would have caught this in minutes.

3. **`IE=0x0006` with `IF=0x0001`** (VBlank pending but not enabled) was noted
   while stuck and never explained. Probably just a stale flag from the derailed
   run — re-check on a healthy trace before spending time on it.

4. **Re-verify audio.** The original "stuck note" anomaly should be gone now
   that the game keeps running. Re-run diagnostics through a battle to confirm.

---

## Files

### Added (tests)

| File | Purpose | Keep? |
|---|---|---|
| `tests/msr_spsr_tests.rs` | **Root-cause regression tests** | Yes |
| `tests/grass_battle_live.rs` | Drives a real grass battle, dumps PNGs to `/tmp/grass/` | Yes — the repro |
| `tests/save_state_roundtrip.rs` | Fix 1 regression tests | Yes |
| `tests/exception_vector_tests.rs` | Fix 2 regression tests | Yes |
| `tests/ppu_scanline_tests.rs` | Fix 3 regression tests | Yes |
| `tests/derail_probe.rs` | Ring-buffer trace, trips when PC leaves valid code | Yes — this is what cracked it |
| `tests/spsr_tbit_probe.rs` | Counts IRQ returns whose SPSR T-bit contradicts the return alignment | Yes — cheap invariant check |
| `tests/swi_vector_probe.rs` | Tallies SWI numbers; flags IRQ dispatch in the wrong mode | Scratch |
| `tests/swi_trip_probe.rs` | Trips on PC reaching `0x08`, names the culprit opcode | Scratch |
| `tests/stuck_probe.rs` | PC histogram + machine state while stuck | Scratch (broadly useful) |
| `tests/ppu_transition_probe.rs` | Dumps PPU regs across the transition | Scratch |
| `tests/win0h_probe.rs` | Counts/decodes WIN0H writes per frame | Scratch |
| `tests/loop_disasm.rs` | Raw words at the stuck loop + DMA table | Scratch |

### Modified (fixes)

- `src/gba/cpu/arm.rs` — **MSR decode mask + SPSR T-bit** (the root cause)
- `src/gba/mod.rs` — `STATE_V2_MAGIC`, v2 save/load tail
- `src/gba/mmu/mod.rs` — UND vector stub at `0x04`
- `src/gba/ppu/mod.rs` — `step()` cycle loop; render at HBlank start

### Superseded

`docs/GRASS_BATTLE_CRASH_FIX.md` was written when Fixes 1 and 2 were believed to
resolve this. It overstates that outcome; it now carries a header pointing here.

---

## Housekeeping

- **Test suite: 153 passing, zero build warnings.**
- **`cargo clippy` and `rustfmt` are NOT installed** on this toolchain. An
  earlier claim of a "clean clippy run" was wrong — that invocation silently did
  nothing, and editor lint hooks report a spurious rustfmt error on every `.rs`
  edit. Only `cargo build` warnings have ever been verified.
- **Nothing is committed.** The working tree mixes the user's pre-existing Linux
  packaging work (`updater.rs`, `.cargo/config.toml`, `build.rs`, `.github/`,
  `packaging/`, `platform.rs`) with all of the above. `README.md`, `src/main.rs`
  and `src/ui/mod.rs` contain edits from both. Use `git add -p` to separate them.
- **Release artifacts** were built at v0.4.0 and verified
  (`target/crabboy-advance-v0.4.0-linux-x86_64.tar.gz`). They predate Fix 3 and
  the MSR fix — **rebuild before shipping**.
- `SHA256SUMS.txt` lists `icon.png` but the tarball ships `assets/icon_256.png`,
  so that entry cannot be verified. Pre-existing packaging-script bug; matters
  because `updater.rs` validates downloads against this file.
- **The user's `.state` file cannot be repaired.** It was captured after the
  derail. Recovered from Trash to
  `saves/Pokemon_-_Emerald_Version__USA__Europe__gba_slot0.state` for forensics
  only. Play from the in-game `.sav`.
