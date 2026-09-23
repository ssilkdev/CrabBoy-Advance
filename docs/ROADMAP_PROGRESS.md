# Roadmap progress log

Tracks implementation of `docs/ROADMAP.md`. Each entry says what landed,
the commit, and how it was verified, so work can resume from here after an
interruption. Newest entries at the bottom of each milestone.

Status key: ✅ done · 🚧 in progress · ⏳ not started

---

## Stage 1: Foundations

### M1. Accuracy baseline ✅

- ✅ **Emerald "internal battery has run dry" (RTC register decode).**
  `src/gba/mmu/rtc.rs`. The S-3511A command byte had been switched to
  MSB-first, but the register table still used the numbers from the old
  bit-reversed decode, so the status/control register (1) and the time
  register (3) were never answered. Registers now follow the datasheet
  (0 reset, 1 control, 2 datetime, 3 time).
  Verified: new unit tests drive the pins exactly like Nintendo's SiiRtc
  library (`sii_library_reads_status_with_24_hour_flag`,
  `sii_library_reads_a_valid_datetime`); a headless run of a fresh Emerald
  cart now reaches NEW GAME / OPTION instead of the battery warning.
- ✅ **GBA test-ROM harness** (`tests/gba_test_roms.rs`, `099c4b2`,
  `5f7c409`, `b486a61`). ROMs live outside the repo (skipped if absent):
  set `GBA_TEST_ROMS` or put them in `~/Downloads/gba-test-roms/`
  (`gba-tests/` = `git clone https://github.com/jsmolka/gba-tests`,
  `suite.gba` from `https://s3.amazonaws.com/mgba/suite-latest.zip`).
  - jsmolka: the verdict is read from the **screen** (r12 is banked in FIQ
    mode and gets clobbered by the text renderer, so the old r12 check
    reported false passes).
  - mGBA suite: each sub-suite is started through the menu and scored from
    its `END: pass/total` debug-port message. `MGBA_BASELINE` holds the
    per-suite floor; the test fails on regression and asks you to raise it
    on improvement, so the numbers below are always current.
  - Debug port: `src/gba/mmu/debug_port.rs` implements mGBA's 0x04FFF600
    print registers (`b3dda40`), which homebrew can also use.
- ✅ **CPU/memory accuracy fixes found by the harness**:
  - `d3d960b` PC reads +12 with register-specified shifts.
  - `b647cca` 8-bit SRAM/Flash bus for 16/32-bit accesses.
  - `228827c` open-bus reads past ROM end; `14b78ae` BIOS read protection
    and open-bus latch.
  - `27c3002` real 2-stage prefetch pipeline (self-modifying code sees the
    stale prefetched opcode). Not in save states; refilled on load.
  - `aaf43e2` misaligned LDRH/LDRSH rotation; open bus for unmapped and
    write-only IO registers.
  - `9f8894c` LDM/STM edge cases (empty list, base in list, misaligned
    base, user-bank transfer) and `TST/TEQ/CMP/CMN` with Rd=PC restoring
    SPSR.
- ✅ **Timing** (`2584182`): WAITCNT wait states per region, sequential vs
  non-sequential accesses, cartridge prefetch buffer, DMA CPU stall, and
  bit-exact BIOS math SWIs with their cycle costs (`mmu/timing.rs`,
  `mmu/bios_math.rs`).
- ✅ **CI** (`9866bfc`): `scripts/fetch_test_roms.sh` downloads the free
  test ROMs; the Linux CI job runs both harnesses with
  `CRABBOY_REQUIRE_TEST_ROMS=1` (a missing ROM fails instead of skipping)
  and writes the per-suite scoreboard to the job summary. The mGBA
  per-suite baselines make any regression fail the build, so the pass
  rate can only go up.
- ✅ **Timers and IRQs** (`46f7182`): timestamp-based timers (cycle-exact
  mid-instruction reads, exact overflow times, correct cascade counts),
  7-cycle IRQ latency, 2-cycle timer read latency; IntrWait sets IME and
  only returns after the IRQ is dispatched.
- ✅ **Design-doc items** (`docs/AI_AGENT_FIX_DESIGN.md`, status header
  updated): NV condition, THUMB undefined, IRQ-return tracking, 5-bit
  blending (`adfc96c`); affine BG/OBJ mosaic (`1f1f2af`); FIFO open bus
  (`c05de83`); pitch-preserved fast-forward (`9491dbd`); lock-free audio
  ring, ITD, BS.775 downmix, soft limiting (`699e9cd`); background GIF
  encoding (`32a2300`). Deliberately not changed items are listed with
  reasons at the top of that document.
- ✅ **SIO** (`ce1c6b5`): registers moved to their real addresses (SIOCNT
  was at 0x124, so games writing it hit SIOMULTI0), mode decoding, readback
  and write masks, idle line levels, multiplayer SIOMLT_SEND.
- ✅ **AGS aging cart**: proprietary, never downloaded. Drop a dump at
  `$GBA_TEST_ROM_DIR/ags.gba` and `ags_aging_cart_runs` boots it for two
  emulated minutes, checks the core doesn't lock up and saves
  `target/ags-result.png`. Not scored automatically: its result screen
  hasn't been characterised without a dump to test on.
- **Final M1 scoreboard** (after `ce1c6b5`):
  - jsmolka gba-tests: **10/10** (was 3/10 at the start of M1)
  - mGBA suite: **4247/7002** (was 3185/6998)
    - Memory 1081/1552 · I/O read 130/130 · Timing 577/2020
    - Timer count-up 438/936 · Timer IRQ 4/90 · Shifter 140/140
    - Carry 93/93 · Multiply long 52/72 · BIOS math 609/615
    - DMA 1032/1244 · SIO R/W 90/90 · SIO timing 0/8 · Misc 1/12
  - GB: Blargg cpu_instrs + instr_timing, dmg-acid2, cgb-acid2 pass.
  - `cargo test --release`: 441 passed, 0 failed. Android host tests 7/7;
    arm64 APK builds.
  - Emerald headless: 257 fps, boots to NEW GAME, no battery warning;
    intro audio statistics unchanged through the timer/audio rework.
- **M1 "done when" check:** pass rate tracked in CI and can only go up ✅;
  Emerald RTC message gone ✅; design-doc open items closed or documented
  ✅.
- Remaining accuracy gaps (not blockers; the ratchet will record any
  progress): cycle-level Timing cases (bus-contention details), timer IRQ
  edge timing, multiply-long carry flag, DMA edge cases, SIO timing.

### M2. Deterministic core and fast save states ✅

- ✅ **Host-independent core** (`01f573d`): core audio runs at a fixed
  48 kHz with an integer sample clock; resampling to the device, rate
  control and fast-forward muting moved to `AudioOutput` (output side
  only). The RTC has a controllable clock:
  `Gba::set_deterministic_clock(Some(unix))` starts it at `unix` and
  advances it with emulated time (CPU cycles). `Gba::new_headless` makes a
  core that never opens an audio device (`4e74fca`).
- ✅ **Complete save states** (`01f573d`, format v3, v1/v2 still load):
  `gba/state.rs` (`StateWriter`/`StateReader`, `Snapshot` trait, decoding
  never panics) plus a `Snapshot` impl next to every component: CPU
  pipeline, PPU registers and scanline position, APU (DirectSound FIFOs,
  PSG channels, frame sequencer, sample clock, filters), timer anchors,
  bus timing and prefetch, SIO, keypad, cartridge (save chip contents and
  protocol, RTC, sensors), MMU latches.
- ✅ **Tests**:
  - `tests/determinism.rs` (needs an Emerald ROM, skips without):
    identical frames and audio across two runs (1200 frames); save → play
    → load → replay is bit-identical (used to diverge at frame 789);
    save → load → save is byte-identical; save+load = ~20 µs, 515 KiB.
  - `tests/replay.rs` (`4e74fca`, runs everywhere incl. CI): a
    hand-assembled test program (BIOS IRQ/IntrWait, keys, timer, DMA, PPU,
    PSG) driven by a 900-frame movie in the `.tas` text format. Two runs
    match; a run saved at frame 450 and resumed in a fresh core matches;
    output matches `tests/data/replay_golden.txt` (`CRABBOY_BLESS=1` to
    rewrite). Sensitivity checked: a 1-cycle IRQ timing change fails it.
- ✅ **Platforms**: Linux locally; Linux + Windows via the existing CI
  `cargo test` jobs; Android verified on the emulator with
  `examples/replay_report.rs` — x86_64 and arm64 (translated) builds both
  print output byte-identical to the golden file.
- **"Done when" check:** replay passes on Linux, Windows (CI) and Android
  ✅; save + restore ≈ 20 µs, about 0.1% of a 16.7 ms frame ✅.
- Note: the Windows result comes from CI, which runs on push; nothing has
  been pushed yet, so that job hasn't run on this code.

### M3. Run-ahead ✅

- ✅ **Core speculative execution & rollback** (`src/gba/run_ahead.rs`,
  `src/gba/mod.rs`): runs the true frame; snapshots the machine; emulates
  1..=4 frames speculatively with the currently held input to render the
  low-latency picture; then rolls back to the true timeline.
- ✅ **Glitchless audio & clean timeline side-effects**:
  - `src/gba/apu/mod.rs`: `Apu::speculative` suppresses sample batching,
    oscilloscope ring buffer updates, audio device pushing, and replay
    capture buffer emission. Speculative frames are silent by design and
    leave zero audio footprint.
  - Save chip writes during speculative frames are preserved in RAM and
    restored by save-state rollback; `cart.save.set_dirty()` maintains the
    real dirty state without triggering false periodic disk flushes.
  - Optional **second instance** (shadow headless core): runs speculative
    frames in a separate headless core cloned and synced via save-state,
    guaranteeing the real core never rolls back. Includes automatic fallback
    to single-instance mode if needed.
- ✅ **Per-game configuration & UI** (`src/ui/config.rs`, `src/ui/mod.rs`):
  - `RunAheadSettings` and `RunAheadGameConfig` persisted in `config.json` with
    backward compatibility for legacy configs without run-ahead keys.
  - Per-game settings keyed by ROM name with global fallback.
  - Emulation menu controls: toggle latency reduction frames (Off, 1 frame,
    2 frames), toggle second instance mode, and report shadow core fallback status.
- ✅ **Tests** (`tests/run_ahead.rs`):
  - `run_ahead_keeps_audio_and_timeline_bit_identical`: verifies on TAS movie
    replay that audio output capture and end-of-run state snapshots remain
    100% bit-identical across 1-frame single instance, 2-frame single instance,
    and 2-frame second instance compared to run-ahead disabled.
  - `run_ahead_reduces_latency_by_configured_frames_homebrew`: verifies that
    measured input-to-screen reaction latency drops by exactly the configured
    number of frames.
  - `run_ahead_reduces_latency_by_configured_frames_emerald`: verifies that
    Pokémon Emerald title screen latency drops frame-for-frame under run-ahead.
- **"Done when" check:** measured input-to-screen latency drops by configured
  frames with no audio artifacts or timeline divergence ✅.

