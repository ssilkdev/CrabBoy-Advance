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
- ⏳ Test-ROM harness for GBA (mGBA suite, AGS aging cart) with a tracked
  pass rate.
- ⏳ Remaining open items from `docs/AI_AGENT_FIX_DESIGN.md`.
