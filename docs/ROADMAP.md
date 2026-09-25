# CrabBoy Advance — Feature Roadmap

An ordered sequence of milestones. There are no dates: each milestone starts
once the ones it depends on are done. Milestones in the same stage that don't
depend on each other can run in parallel.

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
  read the screen with OCR, and speak it with text-to-speech. Read at the
  game's pace, with manual repeat.
- **Depends on:** M11.
- **Done when:** a whole Emerald conversation is read out correctly, both
  from memory and through the OCR fallback.

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
- **Depends on:** M11, and the existing guide import subsystem.
- **Done when:** hints are relevant to what's on screen in a set of scripted
  "stuck" scenarios, and the spoiler setting is respected.

---

## Stage 5: Tools

### M15. Time-travel debugger [New for GBA]
- Step backwards one instruction or one frame at a time, and ask "who last
  wrote this byte?" to jump back to that exact instruction.
- Combines rewind, the disassembler, the flight recorder and the memory viewer.
- **Depends on:** M2.
- **Done when:** you can go from a corrupted value in the hex editor back to
  the instruction that wrote it in one click.

### M16. Branching save-state timeline [New]
- Save states and rewind points form a tree you can browse, like git
  history, instead of 10 flat slots. Every branch has thumbnails, and any
  point can be named, compared and resumed.
- **Depends on:** M2 and M4 (the same format everywhere).
- **Done when:** the tree works on desktop and Android and survives a sync.

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

## Stage 6: Platforms and extras

These don't depend on each other. Pick them in any order after Stage 5, or
earlier when there's spare capacity.

### M18. Android release & polish [Catch-up] ✅ (Core Release Live)
- Multi-ABI release APK (arm64-v8a + x86_64) with native C++ static linking
  and high-precision GLES shaders.
- Custom animated skin packs, on-screen touch layout editor, in-game modal menu,
  fast forward, quick save/load, auto-save rotations, gyro/tilt sensor.
- **Remaining polish:**
  - Scoped Storage folder selector to auto-populate and monitor ROM collections.
  - Android TV / Leanback navigation (full D-pad UI focus navigation).
  - F-Droid and Play Store submission metadata.

### M19. Web (WASM) build [Catch-up]
- Pure Rust core compiled to `wasm32-unknown-unknown` running in modern
  browsers with WebGL/wgpu and WebAudio.
- **Depends on:** M2.

### M20. libretro core [Catch-up]
- Package CrabBoy as a `libretro` core for RetroArch and OS handhelds.
- **Depends on:** M2.

### M21. Super Game Boy borders & palettes [Catch-up]
- SGB border rendering and custom color palettes for monochrome GB titles.

### M22. e-Reader support [Catch-up]
- Emulate the GBA e-Reader accessory, dot-code payload decoding, and memory
  card transfer for E-Card minigames and Pokémon battle cards.

### M23. Local Link Cable & Wireless Adapter (2-player P2P) [Catch-up]
- Two-player peer-to-peer serial synchronization over local Wi-Fi or local IP
  sockets (no high-latency WAN rollback needed).
- Enables Pokémon trading/battling, Mario Kart GP multiplayer, and Zelda Four
  Swords.
- **Depends on:** M2.

### M24. RetroAchievements integration (`rcheevos`) [Catch-up]
- ROM hash matching, badge popups, leaderboards, and hardcore mode via the
  official RetroAchievements community API.
- **Depends on:** M11 (memory offsets).

---

## Stage 7: Nintendo DS Support [New Flagship Track]

A dual-screen expansion bringing full Nintendo DS (NDS) emulation alongside
GBA and Game Boy. Architectural details are documented in `ds_support_plan.md`
and implemented on `feature/nds-support`.

### M25. DS dual-core architecture & memory matrix
- ARM946E-S (67 MHz) with instruction/data TCM and caches.
- ARM7TDMI (33 MHz) running subsystem tasks.
- Bidirectional IPC FIFO with send/receive interrupts.
- Programmable 9-bank VRAM matrix (Banks A–I, 656 KiB total) mapping to 2D
  backgrounds, sprites, textures, and LCD direct mode.

### M26. Dual 2D graphics engine (Engine A & Engine B)
- Engine A (top or bottom screen): Modes 0–5, text/affine/extended backgrounds,
  affine sprites, alpha blending, brightness effects, and display capture.
- Engine B (sub-screen): Modes 0–5 text and affine layers with independent
  palettes and scroll registers.
- Dual 256×192 native framebuffers (composited 256×384).

### M27. DS peripherals, touch digitizer & SPU
- SPI bus connecting the resistive touchscreen controller, firmware NVRAM,
  and power management chip (PMIC).
- 16-channel SPU supporting 8-bit/16-bit PCM, 4-bit IMA-ADPCM, and PSG channels,
  interfacing with CrabBoy's 48 kHz audio sink.
- Cartridge SPI protocol with Blowfish key encryption and ROM stream DMA.

### M28. 3D geometry engine & rasterizer
- 3D Geometry Engine: fixed-point vector/matrix math coprocessor, 4×4 clip
  matrix stack, normal transformation, and vertex lighting.
- Polygon rasterizer: scanline-based software rasterizer and hardware-accelerated
  wgpu/OpenGL backend with texture mapping, alpha blending, edge antialiasing,
  and toon shading.

### M29. Dual-screen UX & Slot-2 crossover
- Desktop and Android dual-screen presentations: Stacked (vertical), Side-by-Side,
  Single Screen with swap hotkey, and Picture-in-Picture (PIP).
- Direct touchscreen input for mobile touch and desktop mouse pointer.
- GBA Slot-2 cartridge pass-through for dual-slot features (Gen 4 Pokémon
  Pal Park, Dual-Slot music/items).

---

## Performance Track

A staged path from the interpreter to optimal execution while guaranteeing
bit-identical emulation (`docs/JIT.md`):

1. **Stage 1 (Hot-path fixes) ✅**: Table CRC32, slice memory fast paths (+18% to +53%).
2. **Stage 2 (Batch peripheral stepping) ✅**: Catch-up event horizon for PPU, timers,
   and serial (+33% to +59%).
3. **Stage 3a (Decode tables & renderer fast paths) ✅**: ARM 4096-entry candidate
   tables, tile-by-tile text BG rendering (+18% to +36%). Total cumulative: **+110% to +144%**.
4. **Stage 4 alternatives (High ROI per effort)**:
   - *Idle-loop detection & skipping*: skip busy-wait cycles up to the next event
     horizon for games that don't HALT (e.g., Pokémon Emerald). Dramatically
     cuts CPU and battery drain on mobile.
   - *Multithreaded scanline rendering*: decouple PPU scanline compositing from
     the CPU loop for multi-core systems.

---

## Out of Scope

- **Global WAN Rollback Netplay**: high-latency cross-internet rollback netplay
  remains out of scope (local P2P link cable is covered in M23).
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
    │      │       └─ M24
    │      ├─ M15
    │      ├─ M19, M20, M23
    │      └─ M25 ── M26 ── M27 ── M28 ── M29 (Stage 7 DS)
    └─ M5 ─┬─ M6 ── M8
           ├─ M7
           └─ M10
```
