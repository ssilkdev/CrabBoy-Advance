# CrabBoy Advance — Feature Roadmap

An ordered sequence of milestones. There are no dates: each milestone starts
once the ones it depends on are done. Milestones in the same stage that don't
depend on each other can run in parallel.

> **Platform Focus**: CrabBoy Advance is developed with an **Android-First**
> philosophy. Android is our primary tier-1 target, driving our performance
> budgets, battery/thermal efficiency, touch ergonomics, and presentation.
> Desktop functions as our reference testing platform, developer toolchain, and
> CI foundation.

Tags:
- **[New]**: no mainstream handheld emulator does this, as far as we know.
- **[Catch-up]**: mGBA, melonDS, NanoBoyAdvance or RetroArch already have it.

Detailed implementation logs and test verification records for completed
milestones are kept in `docs/ROADMAP_PROGRESS.md`.

---

## Stage 1: Foundations

Almost everything later depends on the emulator being accurate and
deterministic, so this comes first.

### M1. Accuracy baseline [Catch-up] ✅
- Run the mGBA test suite, the AGS aging cartridge and the existing GB test
  ROMs in CI, and publish the pass rate.
- Fix the RTC bug that makes Pokémon Emerald report "The internal battery has
  run dry" on a fresh save (`src/gba/mmu/rtc.rs`).
- Close the remaining open items in `docs/AI_AGENT_FIX_DESIGN.md`.
- **Done when:** the pass rate is tracked in CI, can only go up, and the
  Emerald RTC message is gone.
- **Completed:** 10/10 jsmolka, 4,247/7,002 mGBA baseline locked in CI, S-3511A
  RTC fixed, bus wait states and open bus latches implemented.

### M2. Deterministic core and fast save states [Catch-up] ✅
- The same ROM, save and inputs always produce identical frames and audio.
  Remove any dependence on wall-clock time or host timing inside the core.
  The RTC gets a controllable clock.
- Save states round-trip byte-for-byte, and are small and fast enough to take
  every frame (~20 µs).
- CI replays a recorded movie (the existing `.tas` format) and compares a hash
  of every frame.
- **Depends on:** M1.
- **Done when:** the replay test passes on Linux, Windows and Android, and a
  save state plus restore fits comfortably inside one frame's time budget.
- **Completed:** Format v3 states, controllable RTC clock, `tests/replay.rs`
  golden verification passing on all platforms.

### M3. Run-ahead [Catch-up] ✅
- Hide one or two frames of the game's built-in input lag by emulating ahead
  and rolling back. Configurable per game, with an optional second instance so
  audio doesn't glitch.
- **Depends on:** M2.
- **Done when:** measured input-to-screen latency drops by the configured
  number of frames with no audio artifacts.
- **Completed:** 1–4 frames speculative execution, silent speculative APU,
  shadow second-instance core, per-game UI settings in `config.json`.

### M4. Save sync between desktop and Android [Catch-up] ✅
- Battery saves and save states sync through a folder the user picks
  (Syncthing, Google Drive, Dropbox, Nextcloud). Newest wins, and the older
  copy is kept as a backup with automated rotation.
- The same save-state format on both platforms with alias mapping between
  desktop slots (`_slot0.state`) and mobile primary states.
- **Depends on:** M2 (portable save states).
- **Done when:** a game started on desktop continues on Android and back
  again with nothing lost.
- **Completed:** `src/gba/save_sync.rs`, desktop GUI dialog, atomic writes,
  automatic triggers on save/load/flush.

---

## Stage 2: Rendering

### M5. Rendering pipeline groundwork: frame blending and shaders [Catch-up] ✅
- LCD ghosting / frame blending, which some games rely on for transparency
  (flickering sprites).
- A shader pipeline alongside xBRZ and NIS: built-in CRT and LCD-grid presets,
  plus custom shaders (JSON/INI) the user can load.
- Restructure the PPU so it can emit layers and draw commands, not only a
  finished 240×160 buffer. M6–M8 need this.
- **Depends on:** M1.
- **Done when:** the presets work on desktop and Android, and the PPU exposes
  per-layer output without changing the native-resolution image.
- **Completed:** Smart de-flicker, authentic LCD decay, 6 shader presets,
  `PpuLayerBuffers` draw commands.

### M6. HD Mode 7 [New] ✅
- Render affine backgrounds and affine sprites at 2–8× internal resolution,
  the way bsnes-HD does for the SNES. Rotation and scaling become smooth
  instead of blocky and aliased.
- Perspective scanline interpolation removing stairstepping on 3D perspective
  tracks (Mario Kart, F-Zero).
- Seamless layer compositing with native UI and SSAA box downsampling.
- **Depends on:** M5.
- **Done when:** target games render correctly at every scale, with no seams
  where HD layers meet native-resolution layers.
- **Completed:** `src/gba/ppu/hd_mode7.rs`, 2x/4x/8x scaling, SSAA, verified on
  Mario Kart and F-Zero.

### M7. HD sprite and tile replacement packs [New for GBA] ✅
- Swap tiles and sprites for high-resolution art, keyed by an FNV-1a hash of
  the tile's pixels and palette.
- Dynamic palette recoloring preserving anti-aliased shading and alpha across
  damage flashes and palette swaps.
- Tile/sprite dumping tool and `manifest.json` schema.
- **Depends on:** M5.
- **Done when:** a sample pack replaces a game's sprites with animation and
  palette changes still working.
- **Completed:** `src/gba/hd_pack.rs`, CLI dumping tool (`--dump-tiles`),
  sample pack in `assets/sample_hd_pack/`.

### M8. Per-game widescreen [New for GBA] ✅
- For games whose engines already keep content outside the 240-pixel view,
  draw that extra area to fill 16:9 (284x160) or 16:10 (256x160). Controlled
  by a per-game database of safe settings, HUD anchoring, and memory patches.
- **Depends on:** M5, and M6 for affine-heavy games.
- **Done when:** at least three games run in widescreen with no visual
  errors.
- **Completed:** 7 verified game profiles (Mario Kart, F-Zero, Metroid Fusion,
  Zero Mission, Sonic Advance, Aria of Sorrow, Pokémon Emerald).

---

## Stage 3: Audio

### M9. HD music re-synthesis [New] ✅
- Most GBA games use Nintendo's shared M4A ("Sappy") music engine. Detect it
  and intercept note and instrument data, playing music through a 48 kHz
  32-bit floating-point sampler instead of the hardware's 8-bit mixer.
- Cubic Hermite spline and band-limited Sinc interpolation, 4-comb stereo reverb.
- Export per-instrument stems (WAV) and Standard MIDI Files (Type 1 .mid).
- Falls back to normal hardware audio for games that don't use M4A.
- **Depends on:** M2 (audio must stay in sync through save states and rewind).
- **Done when:** Emerald, Minish Cap and Fire Emblem play re-synthesized
  music with correct tempo, looping and sound effects, and the audio mixer
  can switch between HD and hardware audio.
- **Completed:** `src/gba/m4a/`, Jukebox UI, CLI stem/MIDI exporters, verified
  on 10+ major titles.

---

## Stage 4: Accessibility and understanding the game

### M10. Accessibility pack [Catch-up] ✅
- Colorblind filters (Daltonization for protanopia, deuteranopia, tritanopia),
  slow motion (10–100%) with WSOLA pitch-preserved time-stretching,
  toggle-instead-of-hold for buttons, one-handed control layouts (desktop and
  Android), and interface scaling.
- **Depends on:** M5 (filters run through the shader pipeline).
- **Done when:** every option works on both desktop and Android and is saved
  per game.
- **Completed:** `src/gba/accessibility.rs`, slowmo audio stretcher, one-handed
  touch layouts, per-game persistence.

### M11. Automatic game-variable discovery [New] ✅
- A guided RAM search with decision-stump machine learning that finds
  variables like HP, player position, money (including XOR-encrypted fields),
  menu state and the text-box buffer, saving them as a per-game "memory map".
- "Ask me" binary search refiner for ambiguous candidates.
- **Depends on:** M2 (search needs repeatable runs).
- **Done when:** for Emerald it finds HP, position, money and text-box state
  without being told where they are, and the map is saved and reused.
- **Completed:** `src/gba/memmap/`, desktop discovery dialog (Ctrl+J),
  verified on relocated Gen 3 save blocks.

### M12. Dialogue reader [New]
- Take text-box text from game memory when a memory map exists, otherwise
  read the screen with OCR, and speak it with text-to-speech (using Android's
  native `TextToSpeech` service on mobile).
- Read at the game's pace, with manual repeat.
- **Depends on:** M11.
- **Done when:** an Emerald conversation is read out correctly on Android and
  desktop, both from memory and through the OCR fallback.

### M13. Live translation overlay [New as a built-in]
- OCR Japanese text and translate it with a local or remote LLM through the
  existing vision endpoint the AI agent uses. Draw the translation over the
  original text box.
- **Depends on:** M12 (text capture), plus the existing AI agent plumbing.
- **Done when:** a Japanese-only game can be played through its opening
  hour with readable translated dialogue.

### M14. AI hint mode & guide assistant [New]
- The AI agent acts as a companion and advisor: on request it inspects the
  screen, the M11 memory map, and indexed game guide manuals (PDF / Web
  guides) to provide context-aware hints with configurable spoiler levels.
- Mobile floating action button / split-pane companion on Android.
- **Depends on:** M11, and the existing guide import subsystem.
- **Done when:** hints are relevant to what's on screen in a set of scripted
  "stuck" scenarios, and the spoiler setting is respected.

---

## Stage 5: Tools & Visual History

### M15. Time-travel debugger [New for GBA]
- Step backwards one instruction or one frame at a time, and ask "who last
  wrote this byte?" to jump back to that exact instruction.
- Combines rewind, the disassembler, the flight recorder and the memory viewer.
- **Depends on:** M2.
- **Done when:** you can go from a corrupted value in the hex editor back to
  the instruction that wrote it in one click.

### M16. Visual filmstrip rewind & branching timeline [New]
- **Mobile Filmstrip Scrubber:** Swipe from the screen edge on Android to open
  a horizontal thumbnail filmstrip of the last 15–30 seconds. Slide your
  thumb to the exact jump or battle turn and release to resume.
- **Branching Save-State Tree:** Save states form a visual tree you can
  browse with thumbnails, branch names, and instant restore points.
- **Depends on:** M2 and M4.
- **Done when:** filmstrip scrub works smoothly on Android at 60 FPS and state
  trees survive cloud sync.

### M17. Plugin and scripting API [Catch-up; sandboxed WASM plugins would be new]
- A stable API for reading and writing memory, watching frames and inputs,
  drawing HUD overlays, and using the M11 memory maps. Scripting plus
  sandboxed WASM plugins.
- Move the Gen 3 companion onto it as the first plugin. Then add LiveSplit
  autosplitter support, item and route trackers, and overlay plugins.
- **Depends on:** M2 and M11. It works better after M15–M16, which it can
  expose to plugins.
- **Done when:** the companion runs as a plugin with the same features, and
  a LiveSplit autosplitter works for one game.

---

## Stage 6: Android Platform Excellence (Tier-1 Primary Target)

All mobile milestones designed to deliver the best handheld experience on
smartphones, foldables, tablets, and dedicated Android handhelds (Odin, Retroid).

### M18. Android release & core UI [Catch-up] ✅ (Core Release Live)
- Multi-ABI release APK (arm64-v8a + x86_64) with native C++ static runtime
  linking and high-precision GLES shaders.
- Custom animated skin packs, on-screen touch layout editor, in-game modal menu,
  fast forward, quick save/load, auto-save rotations, gyro/tilt sensor.

### M18a. Scoped Storage ROM Library Scanner (SAF Auto-Scan) ✅
- Use Android's Storage Access Framework (`ACTION_OPEN_DOCUMENT_TREE`) to let
  the user pick their `ROMs/` folder once.
- Persist folder URI permissions, automatically scan subdirectories for `.gba`,
  `.gb`, `.gbc`, and `.nds` games, and auto-sync battery saves without manual
  per-file imports.
- **Completed:** Non-blocking iterative tree scanner, persistent URI permissions,
  two-way `.sav` sync, library toolbar integration, verified in multi-ABI release APK.

### M18b. Audio-driven haptic rumble & physical tilt ✅
- **Dynamic LRA Force Feedback:** Convert low-frequency audio energy (bass
  impacts, explosions, boss roars) into nuanced tactile vibrations using
  Android's `Vibrator` / `VibrationEffect` API.
- **Authentic Gyro/Tilt:** Map the phone's physical hardware accelerometer and
  gyroscope to cartridge tilt sensors (*WarioWare Twisted!*, *Yoshi Topsy-Turvy*).
- **Completed:** Screen-aligned gravity/gyro cartridge tilt, APU scope buffer
  single-pole bass energy filter, amplitude-controlled LRA force feedback,
  in-game and startup UI toggles.

### M18c. Android TV & dedicated handheld console navigation
- Complete D-pad and analog stick focus navigation for the game library, search
  filters, and in-game modal menus.
- Zero-touch operation: navigate, launch, play, and configure entirely from
  a Bluetooth controller or built-in gamepad (Retroid Pocket, AYN Odin, Anbernic).
- Android TV Leanback launcher banner and intent declarations.

### M18d. Foldable & tablet clamshell ergonomics
- Detect foldable posture changes (`FoldingFeature` in Jetpack WindowManager).
- When half-folded (clamshell mode): display the game on the upright half and
  touch controls on the flat bottom half.
- Adaptive aspect ratio and UI scaling for large tablets (7"–13").

### M18e. Per-game touch layouts & auto-switching
- Automatically load custom touch control layouts based on the running game's
  genre:
  - *Pokémon / RPGs:* One-handed compact vertical thumb layout.
  - *Mario Kart / F-Zero:* Wide thumb grips with enlarged drift triggers.
  - *Action / Fighting:* Traditional D-pad with responsive diagonal zones.

### M19. Local Link Cable & Wireless Adapter (2-Player P2P)
- Two-player peer-to-peer serial synchronization over local Wi-Fi or Wi-Fi Direct.
- **Zero-Config Pairing:** Host displays an on-screen QR code; client scans with
  the phone's camera to connect instantly.
- Enables Pokémon trading/battling, Mario Kart GP, and Zelda Four Swords.
- **Depends on:** M2.

### M20. RetroAchievements integration (`rcheevos`)
- ROM hash matching, in-game achievement toast popups, leaderboards, and
  hardcore mode via the official RetroAchievements community API.
- **Depends on:** M11 (memory offsets).

### M21. Web (WASM) build & libretro core [Secondary targets]
- WASM browser build and libretro core for cross-platform portability.

---

## Stage 7: Nintendo DS Support (Touch-Native Mobile Focus)

A dual-screen expansion bringing full Nintendo DS (NDS) emulation alongside
GBA and Game Boy. Designed from the ground up for Android touchscreens.
Architectural details in `ds_support_plan.md` and `feature/nds-support`.

**Validation:** `tests/nds_test_roms.rs` drives the RockPolish/rockwrestler
test ROM (ARMv4/v5 instructions, IPC, DIV/SQRT, WRAMCNT/VRAMCNT/TCM) and
passes 23/23. The ROM is not vendored; put `rockwrestler.nds` in
`~/Downloads/nds-test-roms/` or set `NDS_TEST_ROM_DIR`. No commercial ROM has
been tested yet.

Status of M22 (done unless noted):
- ARMv5 DSP instructions (QADD/QDADD, SMULxy/SMLAxy/SMULWy/SMLAWy/SMLALxy),
  LDM/STM edge cases (empty list, base in list, S bit), ARMv5 interworking
  for LDR/LDM/POP into PC, and Thumb BLX label.
- CP15 TCM regions synced to the bus: ITCM/DTCM virtual size plus mirroring,
  movable DTCM, and TCM load mode.
- HLE BIOS stubs: IRQ vectors dispatch to the handler pointer at
  `0x03FF_FFFC` (ARM7) or DTCM+0x3FFC (ARM9).
- IPCSYNC IRQs and the full IPCFIFO: send/receive IRQs, error flag, last-word
  replay, and `0x0410_0000` receive port.
- DIV/SQRT unit (`src/nds/math.rs`), including the hardware quirks for
  divide-by-zero and overflow.
- WRAMCNT shared-WRAM banking, VRAMCNT MST/OFS mapping for banks A–I (CPU and
  PPU views), ARM7 VRAM and VRAMSTAT.
- *Open:* caches, cycle timing, wait states, real card command protocol, and
  BIOS SWIs beyond the HLE set.

### M22. DS dual-core architecture & memory matrix
- ARM946E-S (67 MHz) with instruction/data TCM and caches.
- ARM7TDMI (33 MHz) running subsystem tasks.
- Bidirectional IPC FIFO with send/receive interrupts.
- Programmable 9-bank VRAM matrix (Banks A–I, 656 KiB total) mapping to 2D
  backgrounds, sprites, textures, and LCD direct mode.

### M23. Dual 2D graphics engine (Engine A & Engine B)
- Engine A (top or bottom screen): Modes 0–5, text/affine/extended backgrounds,
  affine sprites, alpha blending, brightness effects, and display capture.
- Engine B (sub-screen): Modes 0–5 text and affine layers with independent
  palettes and scroll registers.
- Dual 256×192 native framebuffers (composited 256×384).

### M24. DS peripherals, touch digitizer & SPU
- SPI bus connecting the resistive touchscreen controller, firmware NVRAM,
  and power management chip (PMIC).
- Direct mobile touch-to-digitizer coordinate mapping (with stylus support).
- 16-channel SPU supporting 8-bit/16-bit PCM, 4-bit IMA-ADPCM, and PSG channels,
  interfacing with CrabBoy's 48 kHz audio sink.
- Cartridge SPI protocol with Blowfish key encryption and ROM stream DMA.

### M25. 3D geometry engine & rasterizer
- 3D Geometry Engine: fixed-point vector/matrix math coprocessor, 4×4 clip
  matrix stack, normal transformation, and vertex lighting.
- Polygon rasterizer: mobile-optimized scanline software rasterizer and
  hardware GLES / Vulkan rasterizer with texture mapping, alpha blending,
  edge antialiasing, and toon shading.

### M26. Mobile dual-screen UX & Slot-2 crossover
- Tailored mobile layouts:
  - *Portrait phone:* Top screen on top, Touch screen on bottom with ergonomic
    touch buttons flanking the lower screen.
  - *Foldable clamshell:* Top screen on the upper display, touch bottom screen
    on the lower display like a physical Nintendo DS.
  - *Landscape phone:* Side-by-side or Picture-in-Picture with quick-swap hotkey.
- GBA Slot-2 cartridge pass-through for dual-slot features (Gen 4 Pokémon
  Pal Park, Dual-Slot music/items).

---

## Performance Track: Mobile Thermal & Battery Optimization

Mobile handheld play requires ruthless optimization for low battery consumption
and zero thermal throttling:

1. **Stage 1 (Hot-path fixes) ✅**: Table CRC32, slice memory fast paths (+18% to +53%).
2. **Stage 2 (Batch peripheral stepping) ✅**: Catch-up event horizon for PPU, timers,
   and serial (+33% to +59%).
3. **Stage 3a (Decode tables & renderer fast paths) ✅**: ARM 4096-entry candidate
   tables, tile-by-tile text BG rendering (+18% to +36%). Total cumulative: **+110% to +144%**.
4. **Stage 4a: Idle-Loop Detection & Busy-Wait Skipping [Critical for Mobile] ✅**:
   - Games like Pokémon Emerald busy-wait in an idle loop at 60 FPS instead of
     sleeping (HALT).
   - Detect idle loops and advance the CPU clock directly to the next peripheral
     event horizon.
   - **Benefit:** Cuts CPU utilization and power draw by **30–50% on mobile**,
     keeping phones cool and extending battery life by hours.
   - **Verified:** 51.5% cycle reduction (8.67M / 16.85M cycles skipped) in Emerald
     gameplay; 100% bit-identical frame/audio hash across test suite and commercial ROMs (`tests/idle_skip.rs`).
5. **Stage 4b: Zero-Copy GLES Pixel Buffer Objects (PBO)**:
   - Stream the core's native framebuffer directly to GPU textures via PBOs,
     eliminating CPU-side Vec allocations and memory bandwidth overhead.
6. **Stage 4c: 59.73 Hz Variable Refresh Rate (VRR/LTPO) Frame Pacing**:
   - Synchronize with Android's `Choreographer` on modern 90Hz/120Hz/144Hz LTPO
     displays to eliminate judder and micro-stutter without screen tearing.

---

## Out of Scope

- **Global WAN Rollback Netplay**: high-latency cross-internet rollback netplay
  remains out of scope (local P2P link cable is covered in M19).
- **Nintendo DSi hardware**: cameras, SD card interface, and TWL-mode enhancements
  are deferred; focus is on the standard Nintendo DS (NDS Lite/Phat) library.
- **Native iOS App Store Release**: cancelled/deferred due to sideloading
  limitations and Linux cross-compilation tooling constraints.

---

## Dependency summary

```
M1 ─┬─ M2 ─┬─ M3
    │      ├─ M4 ── M16
    │      ├─ M9
    │      ├─ M11 ─┬─ M12 ── M13
    │      │       ├─ M14
    │      │       ├─ M17 (also M2)
    │      │       └─ M20 (RetroAchievements)
    │      ├─ M15
    │      ├─ M18 (Android Core) ── M18a ── M18b ── M18c ── M18d ── M18e
    │      ├─ M19 (Local P2P Link)
    │      └─ M22 ── M23 ── M24 ── M25 ── M26 (Stage 7 NDS)
    └─ M5 ─┬─ M6 ── M8
           ├─ M7
           └─ M10
```
