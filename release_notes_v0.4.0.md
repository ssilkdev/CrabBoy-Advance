# 🦀 CrabBoy Advance v0.4.0 — Bug Fix & Hardening Release

This release is a comprehensive correctness and security pass following an in-depth automated code review of the entire emulator core, memory system, and auto-updater. It implements fixes from [`docs/AI_AGENT_FIX_DESIGN.md`](docs/AI_AGENT_FIX_DESIGN.md), addressing every critical finding and most major findings identified by that review.

---

## 🔒 Security

- **Auto-updater now verifies downloads before installing them.** The updater previously accepted any downloaded `crabboy-advance.exe` based only on file size. It now fetches a published `SHA256SUMS.txt` for each release and verifies the download's SHA-256 digest before treating it as trusted, and re-verifies again immediately before installing (closing the window between download and install). Releases without a verified checksum are refused rather than silently trusted.
- `curl.exe` is now resolved via an absolute `%SystemRoot%\System32` path instead of a bare `PATH` lookup.
- Fixed a mutex-poisoning bug where an updater thread panic could cascade into crashing the whole app (the poisoned lock was polled every UI frame).

## 💾 Save Data — Critical Fixes

- **EEPROM save games are now supported.** Previously entirely unimplemented — any game using EEPROM (Golden Sun, Mario Kart: Super Circuit, Fire Emblem, and others) had its saves silently discarded.
- **Real save-type detection.** The emulator now scans each ROM for the standard SDK marker strings (`EEPROM_V`, `SRAM_V`, `FLASH_V`/`FLASH512_V`/`FLASH1M_V`) instead of always assuming 128KB Flash.
- **Plain SRAM games can now actually save.** The existing Flash emulation only stored a byte after a specific unlock-sequence was issued; a plain SRAM cartridge's direct byte writes never triggered that sequence, so saves were silently dropped for every SRAM-only game. There's now a real, direct SRAM backend.
- Flash chip size (64KB/128KB) and ID now match the detected save type instead of always reporting a 128KB Macronix chip.

## 🕹️ Core Emulation Fixes

- **Keypad IRQ implemented.** Games that sleep waiting for a button press via IRQ (a common low-power idle pattern) were hanging forever; this now works.
- **PPU HBlank fixed to fire on all 228 scanlines**, including during VBlank — it was previously suppressed there, breaking HBlank-DMA/IRQ-driven effects that are supposed to continue through VBlank.
- **RTC command decoding fixed.** The command byte was assembled with the wrong bit order, silently corrupting the RTC register-select field for every non-palindromic register number. This broke Pokémon Ruby/Sapphire/Emerald's boot-time RTC probe.
- **APU Channel 3 (wave) dual-bank playback fixed.** Wave RAM reads/writes now correctly target the inactive bank while the other plays, matching real hardware's bank-swap technique (previously a single shared buffer meant writing new waveform data while a note played corrupted the audio currently sounding).
- Channels 1/2/4 now silence immediately when their DAC is turned off via a live envelope register write, not only at the next trigger.
- **BG/OBJ Mosaic implemented** for text-mode backgrounds, bitmap backgrounds, and sprites (screen-transition and pixelation effects used by many commercial titles previously had no visual effect at all).
- Bitmap-mode (3/4/5) BG2 priority is now read from BGCNT instead of hardcoded.
- Fixed BG2X/Y and BG3X/Y affine reference-point sign extension (was sign-extending from the wrong bit).

## 🎮 Cheat Engine

- **Real GameShark/Action Replay decryption implemented.** Previously, encrypted cheat codes from public databases were treated as literal (undecrypted) addresses, so essentially no real-world GSA/AR code worked. The standard 32-round TEA decryption is now applied for the common single-write code types.

## 🛠️ Stability & Cleanup

- Fixed a crash when a connected gamepad reports a non-ASCII name.
- Removed a machine-specific, no-longer-valid linker override that could break `cargo build` outright on a fresh checkout.
- Removed dead/divergent duplicate DMA execution code.
- The live-network updater test is now opt-in so `cargo test` is hermetic and offline-safe by default.
- Consolidated a duplicated ~50-line image-sharpening implementation into a single shared function.

---

## Verification

66 automated unit/integration tests pass (up from 44 in v0.3.0), including new regression tests for every fix above. See [`docs/AI_AGENT_FIX_DESIGN.md`](docs/AI_AGENT_FIX_DESIGN.md) for the full technical writeup, remaining known limitations (CodeBreaker's master-seed encryption scheme and affine-mode mosaic remain unimplemented — documented, not silently guessed at), and implementation notes.

## 💾 Downloads & Verification

See `SHA256SUMS.txt` attached to this release for verifiable checksums of every asset.

*CrabBoy Advance is an open-source Game Boy Advance emulation project developed in pure Rust.*
