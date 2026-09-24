# JIT plan

A staged path from the interpreter to a recompiler. Each stage has to be
**bit-identical** to the one before: same pictures, sound, CPU state and
cycle counts, frame for frame. Save states, replays, run-ahead and the
determinism tests all depend on that. A stage that isn't identical doesn't
ship.

## How identity is checked

- `tests/halt_skip.rs`, `tests/determinism.rs` and `tests/run_ahead.rs`,
  plus the mGBA and jsmolka suites.
- A golden-trace harness outside the repo. It fingerprints every frame of
  5 ROMs for 3000 frames each, with scripted input. Each frame's
  fingerprint covers:
  - both framebuffers and the captured audio
  - the registers, CPSR and cycle count
  - the timers, EWRAM and IWRAM

  The build under test has to produce the same 15,000 fingerprints as the
  pre-JIT commit.
- Unit tests for each fast path against the slow path it replaces, over
  every input (the Thumb decode table: all 65,536 opcodes) or random fuzz
  (memory reads: 80,000 addresses including the special cases).

## Where the time goes

Measured with a sampling profiler on Emerald and Dragon Ball, 3000 frames.

| Area | Share | Notes |
|---|---|---|
| CPU fetch/decode/dispatch | ~35% | per-instruction re-decode, pipeline, open bus |
| PPU scanline rendering | ~25% | not helped by a JIT |
| Frame CRC in the video linter | 12-16% | bit-at-a-time CRC32, every frame |
| APU / timers / SIO per-instruction stepping | ~20% | called after every instruction |

A JIT only speeds up the first row. The PPU and the per-instruction
peripheral stepping cap what it can achieve. So the plan attacks the cheap,
wide wins first.

## Stages

1. **Hot-path fixes** ✅ (bit-identical)
   - Table-driven CRC32 (slicing-by-4) for the linter.
   - A fast path for plain memory reads: EWRAM, IWRAM and ROM, with the
     GPIO window and past-end ROM reads kept on the slow path.
   - The Thumb format decided by a 1024-entry table instead of a 20-test
     if-chain.

   Result: +18% to +53% fps (see the table below).
2. **Batch peripheral stepping** ✅ (bit-identical)
   - The PPU, timers, APU and serial port now run behind the CPU until
     the next scheduled event, instead of being stepped after every
     instruction. The event horizon covers the PPU boundary, timer
     overflow, audio sample, DMG frame sequencer and serial transfer end.
   - They catch up first before anything that could observe them:
     - CPU IO reads and writes (`Mmu::cpu_read*`/`cpu_write*`)
     - BIOS calls (SWI)
     - the end of each frame
   - The optimisation doesn't apply while any of these is true, so it
     can't change IRQ timing:
     - the CPU is sleeping (HALT/IntrWait)
     - an IRQ is raised or pending
     - a DMA stall is queued
     - a keypad IRQ condition holds
   - Only active inside `run_frame`, so single-stepping (debugger, probes)
     is unchanged.
   - `tests/halt_skip.rs` compares batching on vs off over 2000 frames of
     5 ROMs. Removing one catch-up makes it fail at frame 2.

   Result: a further +33% to +59% fps (see the table below).
3. **Block cache.**
   - Pre-decode straight-line runs of Thumb and ARM code into op lists,
     keyed by address and mode.
   - Invalidate on any write to a page that holds cached code
     (self-modifying code, and code copied into IWRAM).
   - Blocks end at branches, SWIs, writes to PC and mode switches.

   Finding: every instruction fetch also advances the bus-timing model
   (wait states, ROM prefetch buffer). A cache therefore can't skip the
   fetches without changing cycle counts. The per-block timing has to be
   precomputed per prefetch state, which is why this comes after stage 2.
4. **Native code for hot blocks.** Emit x86-64 (desktop) and AArch64
   (Android) for blocks that ran more than N times, falling back to the
   cached interpreter for anything unusual. iOS forbids W^X pages, so it
   keeps stage 3.

## Results so far

Headless, 3000 frames after a 600-frame warm-up, i9-12900K:

| Game | Before | Stage 1 | Stage 2 | Total |
|---|---|---|---|---|
| Pokémon Emerald | 287 fps | 371 fps | 517 fps | +80% |
| Dragon Ball Advanced Adventure | 406 fps | 488 fps | 648 fps | +60% |
| Pokémon Sapphire | 443 fps | 557 fps | 717 fps | +62% |
| Harry Potter | 529 fps | 811 fps | 1211 fps | +129% |
