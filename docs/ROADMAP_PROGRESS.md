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

### M4. Save sync between desktop and Android ✅

- ✅ **Cross-platform sync coordinator** (`src/gba/save_sync.rs`):
  - User-selected sync directory (Syncthing, Google Drive, Dropbox, Nextcloud).
  - Handles battery saves (`.sav`) and save states (`.state`).
  - **Newest wins**: evaluates file modification timestamps so the device with
    latest progress takes precedence.
  - **Zero data loss automatic backups**: when an older file is overwritten, it is
    archived into a timestamped `.bak.<timestamp>_<seq>` file, with configurable
    retention limit rotation (`max_backups`, default 3).
  - **Byte-level identity check**: identical files skip copying and disk writes
    (`SyncStatus::UpToDate`), conserving disk lifetime and sync bandwidth.
  - **Timestamp preservation**: `copy_preserving_mtime` maintains original file
    mtimes to eliminate ping-pong sync cycles.
  - **Smart directory routing**: `target_dir_for_file` routes incoming files to
    proper subdirectories (`states/` vs `roms/` vs `saves/`) based on file extension
    and existing ROM presence.
- ✅ **Desktop Slot 0 & Android Primary State Linking** (`src/ui/save_manager.rs`,
  `src/gba/save_sync.rs`):
  - Desktop convention (`saves/{stem}_slot0.state`) and Android convention
    (`states/{stem}.state`) are transparently mapped.
  - Saving slot 0 on desktop mirrors to `{stem}.state`, and loading slot 0 falls
    back to `{stem}.state` when slot 0 file is absent.
  - Bidirectional alias synchronization mirrors states between the two naming
    schemes during cloud sync.
- ✅ **Desktop UI & Automatic Triggers** (`src/ui/save_sync_dialog.rs`, `src/ui/config.rs`, `src/ui/mod.rs`):
  - `SaveSyncDialog`: graphical sync management with folder picker (`rfd`), auto-sync
    checkbox, backup retention slider (1..=10), manual "Sync All" and "Sync Current Game"
    triggers, and real-time status reporting with color-coded badges and backup logs.
  - Auto-sync triggers hooked into ROM loading, save state save/load, and battery
    save flushing to disk.
  - Settings persisted in `config.json` under `save_sync`.
- ✅ **Tests** (`tests/save_sync.rs`):
  - `save_sync_bidirectional_and_alias_mapping`: verifies roundtrip sync where desktop
    starts Emerald, Android receives states and plays forward 1 hour, and desktop syncs
    back with older desktop saves archived into `.bak` files.
  - `save_sync_newest_wins_and_backup_rotation`: verifies newest-wins overwrite and
    monotonic backup rotation enforcing the maximum backup retention limit.
  - `save_sync_identical_files_detected_and_skipped`: verifies that byte-identical files
    are detected and skipped with zero writes.
  - `save_sync_real_v3_state_roundtrip`: boots real GBA cores, saves v3 state on desktop,
    syncs through cloud to Android, loads state on a fresh Android core, and verifies
    framebuffers and CPU state continue running bit-identically.
- **"Done when" check:** a game started on desktop continues on Android and back again
  with nothing lost ✅.

---

## Stage 2: Rendering

### M5. Rendering pipeline groundwork: frame blending and shaders ✅

- ✅ **LCD Ghosting and Frame Blending** (`src/gba/frame_blend.rs`, `da92d38`):
  - `FrameBlendMode`: `Off`, `Simple50`, `SmartDeFlicker`, and `LcdGhosting { decay }`.
  - `blend_50_50`: fast bitwise parallel 50/50 blend in 7 operations with no channel bleed.
  - `SmartDeFlicker`: detects 30 Hz flicker (pixel $t == t-2 \ne t-1$) used by games for transparency and shadows (F-Zero, shields, reflections), blending flickering pixels while keeping moving objects and backgrounds pin-sharp.
  - `LcdGhosting`: authentic liquid crystal response persistence modeling exponential decay across frames.
- ✅ **Cross-Platform Shader Pipeline & Custom Profiles** (`src/gba/shader.rs`, `da92d38`):
  - Built-in presets: `Crisp`, `Linear`, `LcdGrid` (cell borders and gap dimming), `LcdSubpixel` (AGB-001/AGS-101 vertical RGB subpixel triads), `CrtScanlines`, `CrtGeom` (scanlines + aperture grille + bloom), alongside existing `NvidiaSharpen` and `Xbrz`.
  - `CustomShaderParams`: parser supporting both structured JSON and flexible INI key=value files with `#` / `//` comments. Configurable scanline intensity, LCD grid intensity, aperture grille, brightness boost, color temperature, and bloom.
- ✅ **PPU Per-Layer Buffers & Draw Commands** (`src/gba/ppu/layers.rs`, `src/gba/ppu/mod.rs`, `da92d38`):
  - `PpuLayer` (`Backdrop`, `Bg0`..`Bg3`, `Obj`), `PpuLayerBuffers`, `LayerKind`, and `LayerDrawCommand`.
  - `set_layer_capture(true)` records isolated 240x160 RGBA surfaces per layer (transparent pixels have alpha 0) and draw command metadata (layer kind, priority, scroll offsets, affine transformation matrices, blending mode, and window enable flags).
  - Native composited 240x160 framebuffer output remains completely bit-identical whether layer capture is enabled or disabled.
  - Zero performance overhead when layer capture is disabled (buffers unallocated).
- ✅ **UI & Android Integration** (`src/ui/mod.rs`, `src/ui/screen.rs`, `src/ui/config.rs`, `android/src/app.rs`):
  - Video menu on desktop allows switching frame blend modes and shader presets with real-time preview, plus "Load Custom Shader..." file dialog.
  - Persistence in `config.json` under `render`.
  - Android port (`CrabBoyApp`) integrates `FrameBlender` and shader presets directly into frame upload and menu UI.
- ✅ **Tests** (`tests/rendering_pipeline.rs`, `da92d38`):
  - 8 integration tests covering:
    - Layer capture isolation with bit-identical native framebuffer verification.
    - Forced blank layer capture behavior.
    - Simple 50/50 frame blending.
    - Smart de-flickering (motion vs 30 Hz oscillation).
    - Authentic LCD ghosting exponential decay.
    - Preset shader geometry and channel biasing (LCD grid, LCD subpixel, CRT scanlines, CRT geom).
    - Custom shader JSON and key=value parsing and execution (warm vs cool color temperature, brightness boost).
- **"Done when" check:** presets work on desktop and Android ✅; PPU exposes per-layer output without changing the native-resolution image ✅.

### M6. HD Mode 7 ✅

- ✅ **HD Mode 7 Affine Rendering Engine** (`src/gba/ppu/hd_mode7.rs`, `9b44c59`):
  - `HdScale`: `Off`, `X2` (480x320), `X4` (960x640), `X8` (1920x1280).
  - `HdMode7Config`: scale factor, perspective scanline interpolation toggle, and SSAA downsampling toggle.
  - Continuous subpixel coordinate evaluation $(u, v)$ for affine backgrounds (Mode 1 BG2, Mode 2 BG2/3) and affine bitmaps (Mode 3 15-bit color, Mode 4 8-bit palette with page flipping, Mode 5 15-bit color with page flipping).
  - Scanline perspective interpolation: interpolates affine parameters ($PA, PB, PC, PD, X, Y$) across sub-scanlines between scanline $y$ and $y+1$, eliminating stairstepping on 3D perspective tracks (Mario Kart: Super Circuit, F-Zero: Maximum Velocity).
  - Subpixel affine sprite evaluation: renders rotscale OBJs at high resolution with subpixel transformation coordinate mapping.
  - Seamless layer compositing: composites HD affine surfaces with native resolution layers (Backdrop, non-affine text BGs BG0/1, non-affine sprites) using hardware priority sorting `(priority ASC, is_bg ASC, layer_idx ASC)` and GBA color special effects (alpha blending, brightness increase/decrease via `apply_color_effects`).
  - Supersample Anti-Aliasing (SSAA): box-filtered downsampling from HD internal resolution back to native 240x160 for high-fidelity anti-aliasing without requiring a high-resolution display window.
  - Zero overhead and bit-identical output when HD Mode 7 is disabled.
- ✅ **PPU Architecture & Draw Command Integration** (`src/gba/ppu/layers.rs`, `src/gba/ppu/mod.rs`, `9b44c59`):
  - `LayerDrawCommand` updated with `bgcnt`, `bldcnt`, `bldalpha`, and `bldy`.
  - Affine origin snapshotting (`scanline_affine_origin`) captures internal affine registers $(X, Y)$ *before* per-scanline increments to guarantee exact subpixel geometric alignment.
  - `hd_config`, `set_hd_mode7_config`, and `render_hd_frame` exposed on `Ppu` and `Gba`.
- ✅ **Desktop & Android UI Integration** (`src/ui/mod.rs`, `src/ui/screen.rs`, `src/ui/config.rs`, `android/src/app.rs`, `9b44c59`):
  - Video menu UI on desktop with radio buttons for HD scale (Off, 2x, 4x, 8x), checkboxes for perspective interpolation and SSAA.
  - Dynamic texture resizing in `ScreenRenderer` matching the HD resolution `[240 * scale, 160 * scale]`.
  - Android port (`CrabBoyApp`) configures `HdMode7Config`, uploads HD frames, and exposes configuration in UI.
- ✅ **Tests** (`tests/hd_mode7.rs`, `9b44c59`):
  - 8 integration tests covering:
    - Scale factors and HD frame dimension computation.
    - Subpixel affine rendering in Mode 1 with rotated/scaled tilemaps.
    - Perspective scanline interpolation removing stairstepping between adjacent scanlines.
    - Seamless compositing between high-resolution affine tracks and native UI text layers.
    - Box-filtered SSAA downsampling from 4x HD to native 240x160.
    - High-resolution Mode 3 bitmap affine rendering.
    - High-resolution affine sprite transformation and transparency.
    - SSAA preservation of native pixel boundaries for non-affine text and UI.
- **"Done when" check:** affine backgrounds and sprites render smoothly at 2x, 4x, 8x resolution without stairstepping or seams against native UI layers ✅.

### M7. HD sprite and tile replacement packs [New for GBA] ✅

- ✅ **Hash-Keyed Extraction & Identification Engine** (`src/gba/hd_pack.rs`, `1bc78d8`):
  - FNV-1a 64-bit deterministic hash implementation in pure Rust (`fnv1a_64`, `hash_to_hex`, `hex_to_hash`) with zero external hashing dependencies.
  - VRAM tile pixel extraction and hashing for both 4bpp and 8bpp tiles (`extract_tile_pixels`, `hash_tile`).
  - Palette extraction and hashing for both BG palettes and OBJ palettes (`extract_palette`, `hash_palette`).
  - Canonical un-flipped composite sprite extraction across 1D and 2D tile-to-OBJ mapping modes (`extract_sprite`, `sprite_to_rgba`), guaranteeing the same canonical hash regardless of in-game horizontal or vertical orientation.
- ✅ **Pack Format, Manifest & In-Memory Asset Storage** (`src/gba/hd_pack.rs`, `1bc78d8`):
  - `manifest.json` schema (`HdPackManifest`, `HdReplacementEntry`) supporting:
    - High-res PNG assets at integer scales (e.g. 2x, 4x, 8x).
    - `palette_hash` optional matching: allows artists to supply alternate high-res art for distinct character variants (e.g., Shiny Pokémon, Fire Mario, player vs enemy colors).
    - `recolor: true`: dynamic palette recoloring that maps base palette entries to current runtime palette entries, dynamically preserving anti-aliased shading and alpha across in-game damage flashes and screen transitions.
    - Animation frame swapping: distinct high-resolution art mapped to each unique sprite state/frame automatically keyed by tile hash.
    - `hflip` / `vflip` manifest overrides.
  - `HdPack::load_from_dir`: loads directory of PNG assets and manifest into in-memory `HdImage` and `HdReplacement` buffers with $O(1)$ fast lookup tables (`find_sprite_replacement`, `find_tile_replacement`).
- ✅ **Runtime Replacement & Subpixel Compositing** (`src/gba/ppu/hd_mode7.rs`, `src/gba/ppu/mod.rs`, `src/gba/mod.rs`, `1bc78d8`):
  - Subpixel replacement injection during high-resolution rendering: replaces native tiles and sprites with high-resolution textures.
  - Full support for sprite bounding box wrapping, affine/non-affine rendering, horizontal and vertical flipping, and semi-transparency.
  - High-resolution text BGs (BG0..3) tile replacement.
  - Box-filtered SSAA downsampling: high-res assets downsampled to native 240x160 with subpixel fidelity when SSAA is enabled or running on native displays.
  - Zero overhead and bit-identical output when HD packs are disabled.
- ✅ **Asset Dumping Tool & CLI Commands** (`src/gba/hd_pack.rs`, `src/main.rs`, `1bc78d8`):
  - `dump_tiles_and_sprites`: extracts active on-screen sprites and VRAM background tiles, writes numbered PNG assets, and generates a ready-to-edit `manifest.json` template.
  - Headless CLI command `--dump-tiles <ROM> [--output <DIR>] [--frame <N>]`.
  - Headless/CLI loading flag `--hd-pack <DIR>`.
- ✅ **Desktop & Android UI Integration** (`src/ui/mod.rs`, `src/ui/config.rs`, `android/src/app.rs`, `1bc78d8`):
  - Video menu UI on desktop: toggle HD pack checkbox, active pack info banner (pack name, scale, sprite/tile replacement count), "Load HD Pack Folder..." directory picker (`rfd`), and "Dump Tiles & Sprites to Folder..." button.
  - Persistence in `config.json` under `render.hd_pack_path` and `render.hd_pack_enabled`.
  - Android port (`CrabBoyApp`) automatically uploads or downsamples HD frames when HD pack is enabled.
- ✅ **Sample HD Pack** (`assets/sample_hd_pack/`, `1bc78d8`):
  - Includes sample hero sprites (idle, walk frames, fire palette variant), `manifest.json`, and documentation explaining pack authoring and dynamic recoloring.
- ✅ **Tests** (`tests/hd_pack.rs`, `1bc78d8`):
  - 11 unit and integration tests covering:
    - FNV-1a 64-bit deterministic hashing and hex parsing.
    - Tile pixel extraction and palette hashing.
    - Sprite extraction and horizontal/vertical flip canonical invariance.
    - Manifest JSON serialization and deserialization roundtrip.
    - Pack loading from directory and PNG decoding.
    - Sample HD pack verification and animation frame matching.
    - Tile and sprite dumping tool verification.
    - High-resolution sprite replacement rendering.
    - Dynamic palette recoloring and palette variant selection.
    - Sprite animation frame swapping across action states.
    - Box-filtered SSAA downsampling preserving subpixel fidelity.
- **"Done when" check:** a sample pack replaces a game's sprites with animation and palette changes still working ✅.




