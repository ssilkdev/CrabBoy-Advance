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
3. **Stage 3a: decode tables and renderer fast paths** ✅ (bit-identical)
   - ARM decode: a 4096-entry candidate table plus each class's full test,
     first match in the old chain's order. Tested against the chain on
     every key with 81 fills plus 3M random words.
   - Renderer:
     - background layers merged in precomputed priority order
     - BGR555 to RGBA via a compile-time table
     - text backgrounds drawn a tile at a time (fuzz-tested against the
       per-pixel loop)
   - Video linter: repeated frames found by comparing with the last frame;
     the CRC is computed only when needed.

   Result: +18% to +36% over stage 2 on the Pokémon games and Dragon Ball.

   **Block cache (not done, low value now).** Every instruction fetch has
   to go through the bus-timing model anyway (wait states, prefetch
   buffer). Reading the opcode itself costs about 2 ns, so caching decoded
   blocks would save only a few percent.
4. **Native code for hot blocks** (not started: needs a go/no-go)
   - Emit x86-64 (desktop) and AArch64 (Android) for hot blocks, with the
     cached interpreter as fallback. iOS forbids W^X pages, so it would
     stay on the interpreter.
   - **Ceiling, measured after stage 3a (Emerald profile).** ARM and Thumb
     execution *including* bus timing is about 43% of frame time. The
     rest is scanline rendering, peripherals, audio and diagnostics, and
     a JIT doesn't touch any of it. Bit-identical timing means generated
     code still has to call the timing model for every fetch and access.
     So even an infinitely fast JIT would give at most ~1.6x, and a
     realistic one ~1.2-1.3x.
   - **Cost.**
     - A code generator per architecture (or Cranelift, which adds several
       MB to the APK).
     - Block invalidation for code in RAM.
     - Exact cycle accounting per block.
     - Months of accuracy work to stay identical.

## Stage 4 alternatives with better payoff per effort

- **Sprite rendering and HD-audio sync**: 3-4% each in the profile.
- **Idle-loop detection**: Emerald never uses HALT; it busy-waits. It
  could be made exact by skipping whole loop iterations up to the next
  event. That's the same event-horizon machinery as the halt skip.
- **Rendering on a second thread**: a scanline only depends on registers
  latched at HBlank. Bit-identical if the PPU is handed a copy of the
  line's state.

## Results so far

Headless, 3000 frames after a 600-frame warm-up, i9-12900K:

| Game | Before | Stage 1 | Stage 2 | Stage 3a | Total |
|---|---|---|---|---|---|
| Pokémon Emerald | 287 fps | 371 fps | 517 fps | 612-644 fps | +113-124% |
| Dragon Ball Advanced Adventure | 406 fps | 488 fps | 648 fps | 868 fps | +114% |
| Pokémon Sapphire | 443 fps | 557 fps | 717 fps | 929 fps | +110% |
| Harry Potter | 529 fps | 811 fps | 1211 fps | 1293 fps | +144% |
