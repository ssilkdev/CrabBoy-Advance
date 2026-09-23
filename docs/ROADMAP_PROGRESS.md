# Roadmap progress log

Tracks implementation of `docs/ROADMAP.md`. Each entry says what landed,
the commit, and how it was verified, so work can resume from here after an
interruption. Newest entries at the bottom of each milestone.

Status key: ✅ done · 🚧 in progress · ⏳ not started

---

## Stage 1: Foundations

### M1. Accuracy baseline 🚧

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
- Scoreboard (after `2584182`):
  - jsmolka gba-tests: **10/10** (arm, thumb, memory, nes, bios, unsafe,
    save×3, ppu)
  - mGBA suite: **3984/6998** (baseline was 3185)
    - Memory 1081/1552 · I/O read 124/130 · Timing 482/2020
    - Timer count-up 345/936 · Timer IRQ 0/90 · Shifter 140/140
    - Carry 93/93 · Multiply long 52/72 · BIOS math 609/615
    - DMA 1032/1244 · SIO R/W 25/90 · SIO timing 0/4 · Misc 1/12
  - `cargo test --release`: 409 passed, 0 failed.
  - Emerald headless (scratch `emu-probe`, 1860 frames): 250 fps, boots to
    NEW GAME. (170 fps before the timing work, because the CPU was doing
    too much work per frame.)
- 🚧 Next in M1: timers (count-up and IRQ latency), remaining Timing
  cases, multiply-long flags, DMA edge cases, SIO registers.
- ⏳ AGS aging cart: proprietary, so it can't be fetched; harness will
  accept an optional path once the above is in.
- ⏳ Remaining open items from `docs/AI_AGENT_FIX_DESIGN.md`.
