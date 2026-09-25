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

### M8. Per-game widescreen [New for GBA] ✅

- ✅ **Widescreen Engine & Subpixel Viewport Expansion** (`src/gba/widescreen.rs`, `6276c57`):
  - `WidescreenMode`:
    - `Off`: Native 3:2 (240x160).
    - `Ratio16_9`: 284x160 (+22px left margin, +22px right margin, aspect error 0.15%).
    - `TileAligned16_9`: 288x160 (+24px left margin, +24px right margin, exact 36x20 8x8 tiles).
    - `Ratio16_10`: 256x160 (+8px left margin, +8px right margin, Steam Deck native aspect ratio).
    - `Custom { width, height }`: Arbitrary custom viewport dimensions.
  - `bg_expand: [bool; 4]`: Selective background layer expansion mask allowing scrolling gameplay backgrounds (BG1..3) to expand into widescreen margins while keeping HUD/UI layers (BG0) pinned or pillarboxed.
  - `hud_anchor: [HudAnchor; 4]`:
    - `Center`: Centered at native horizontal position ($X = \text{margin\_left} \dots \text{margin\_left} + 240$), preventing HUD repetition or corruption across margins.
    - `Left`: Anchored to the left margin border ($X = 0 \dots 240$).
    - `Right`: Anchored to the right margin border ($X = W - 240 \dots W$).
    - `Expand`: Layer expanded across entire width (identical to `bg_expand: true`).
    - `Pillarbox`: Layer clamped strictly to native 240px viewport with transparent/backdrop margins.
  - `obj_expand: bool`: Extended sprite scanning and rendering for horizontal coordinates $X \in [-\text{margin\_left}, 240 + \text{margin\_right}]$, seamlessly drawing characters and enemies moving beyond the 240px boundary.
  - `window_mode: WidescreenWindowMode`:
    - `ExtendFull`: Stretches screen-wide fade and color effects (WIN0/WIN1/WINOUT) across the entire widescreen viewport ($0 \dots W$), eliminating black side bars during screen fades and transitions.
    - `Clamp240`: Clamps window coordinates strictly to native 240px area.
    - `IgnoreInMargins`: Outside window effects applied within margins.
  - Continuous subpixel coordinate evaluation across widescreen margins:
    - Mode 1 BG2 and Mode 2 BG2/3 affine backgrounds evaluated continuously across negative and margin coordinates.
    - Full compatibility with HD Mode 7 (2x, 4x, 8x internal rendering) and SSAA downsampling to widescreen resolutions (`downsample_ssaa_wide`).
    - Full compatibility with M7 HD Replacement Packs (subpixel replacement injection and dynamic recoloring in widescreen margins).
- ✅ **Per-Game Widescreen Database & ROM/RAM Patching** (`src/gba/widescreen.rs`, `6276c57`):
  - `WidescreenDatabase` lookup table mapping game title codes to tuned safe profiles:
    1. **Mario Kart: Super Circuit** (`AMKE`, `AMKP`, `AMKJ`): Mode 2 affine track and sky horizon expanded to 16:9; HUD lap/timer/speedometer centered.
    2. **F-Zero: Maximum Velocity** (`AFZE`, `AFZP`, `AFZJ`): Mode 2 affine track and starfield expanded to 16:9; speedometer, mini-map, and boost gauges centered.
    3. **Metroid Fusion** (`AMFE`, `AMFP`, `AMFJ`): 512-wide room scenery layers expanded to 16:9; Samus HUD centered; dynamic ROM/RAM camera draw clip patch applied.
    4. **Metroid: Zero Mission** (`BMXE`, `BMXP`, `BMXJ`): Room layer expansion; centered HUD.
    5. **Sonic Advance** (`ASOE`, `ASOP`, `ASOJ`): High-speed parallax backgrounds expanded to 16:9; rings, timer, and score centered.
    6. **Castlevania: Aria of Sorrow** (`AANE`, `AANP`, `AANJ`): Castle corridors and background layers expanded; HP/MP/boss bars centered.
    7. **Pokémon Emerald** (`BPEE`, `BPEP`, `BPEJ`): Battle scenes and overworld layers expanded; battle HUD and text boxes centered.
  - Automatic detection and profile application on ROM load via `configure_widescreen_for_loaded_cartridge()`.
  - ROM and EWRAM memory patching support via `WidescreenMemoryPatchDef` and `apply_to_core()`.
- ✅ **Desktop & Android UI Integration** (`src/ui/mod.rs`, `src/ui/screen.rs`, `src/ui/config.rs`, `src/main.rs`, `6276c57`):
  - Desktop Video menu includes a dedicated "Per-Game Widescreen" section:
    - Checkbox toggle for widescreen mode.
    - Active profile indicator with game title and region information.
    - Radio buttons for 16:9 Standard (284x160), 16:9 Tile-Aligned (288x160), and 16:10 Steam Deck (256x160).
  - Main viewport render widget dynamically updates aspect ratio (`calculate_target_size_for_aspect`) to eliminate pillarbox bars when widescreen is enabled.
  - CLI flag `--widescreen` for headless/GUI launches and `--dump-frame`.
  - Settings persisted in `config.json` under `render.widescreen`.
- ✅ **Tests** (`tests/widescreen.rs`, `6276c57`):
  - 13 comprehensive integration tests covering:
    - `test_widescreen_database_profiles`: verifies database lookup across all 7 supported games and region codes.
    - `test_widescreen_memory_patch_application`: verifies ROM/EWRAM memory patch application.
    - `test_widescreen_modes_and_geometry`: verifies width, margin, and aspect ratio calculations for 16:9, TileAligned, and 16:10.
    - `test_game_verification_1_mario_kart_super_circuit`: verifies Mario Kart Mode 2 track expansion without visual glitches.
    - `test_game_verification_2_fzero_maximum_velocity`: verifies F-Zero Mode 2 track and starfield expansion.
    - `test_game_verification_3_metroid_fusion`: verifies Metroid Fusion room layer expansion and HUD anchoring.
    - `test_widescreen_text_bg_layer_expansion`: verifies text BG horizontal expansion across margins.
    - `test_widescreen_hud_anchoring_prevents_repeating`: verifies HUD anchoring prevents repeating/corrupted UI elements.
    - `test_widescreen_sprite_offscreen_expansion`: verifies sprites render in margins beyond the 240px coordinate boundary.
    - `test_widescreen_mode2_affine_track_evaluation`: verifies subpixel affine evaluation across margin bounds.
    - `test_widescreen_window_full_screen_fade`: verifies full-screen window fading across entire widescreen viewport.
    - `test_hd_mode7_widescreen_combined_rendering`: verifies HD Mode 7 (scale 2x/4x) combined with widescreen expansion.
    - `test_hd_pack_widescreen_replacement_rendering`: verifies M7 HD replacement art injection and recoloring in widescreen margins.
- **"Done when" check:** at least three games run in widescreen with no visual errors (Mario Kart, F-Zero, Metroid Fusion verified) ✅.

---

## Stage 3: Audio

### M9. HD music re-synthesis [New] ✅

- ✅ **M4A / "Sappy" Sound Engine Interception & Auto-Detection** (`src/gba/m4a/mod.rs`, `74a1b9c`):
  - Signature scanning for Nintendo's shared 48-byte `CLOCK_TABLE` (`[0x01, 0x02, ..., 0x60]`) across ROM bytes.
  - Candidate song table heuristic scanner identifying valid GBA ROM pointers and track counts.
  - Verified per-game database profiles with accurate song table offsets and song counts:
    - **Pokémon Emerald** (`BPEE`, `BPEP`, `BPEJ`, `BPED`, `BPEF`, `BPES`, `BPEI`): Song table at `0x6B49F0` (610 songs).
    - **Pokémon FireRed & LeafGreen** (`BPRE`, `BPGE`): Song table at `0x4A32CC` / `0x4A2A2C` (500 songs).
    - **Pokémon Ruby & Sapphire** (`AXVE`, `AXPE`): Song table at `0x4548E0` / `0x454508` (450 songs).
    - **The Legend of Zelda: The Minish Cap** (`BZME`, `BZMP`, `BZMJ`): Song table at `0xA11DBC` / `0xB1D414` (546 songs).
    - **Fire Emblem: The Sacred Stones** (`BE8E`, `BE8P`, `BE8J`): Song table at `0x224470` / `0x42FFB0` (1000 songs).
    - **Fire Emblem: The Blazing Blade** (`AE7E`, `AE7X`, `AE7J`): Song table at `0x69D6D8` / `0x6805F4` (1001 songs).
    - **Fire Emblem: The Binding Blade** (`AFEJ`): Song table at `0x3994D8` (621 songs).
    - **Metroid Fusion & Zero Mission** (`AMFE`, `BMXE`): Song tables at `0x71794C` / `0x794020` (300 songs).
    - **Castlevania: Aria of Sorrow** (`AANE`): Song table at `0x4FA908` (200 songs).
    - **Golden Sun & The Lost Age** (`AGSE`, `AGFE`): Song tables at `0x153A00` / `0x127A00` (300 songs).
  - Runtime WRAM scanner anchoring on active `MusicPlayerInfo` (`ident == 0x68736D53` / `'Smsh'`) in EWRAM and IWRAM to intercept live game playback without hardcoded RAM addresses.
- ✅ **WaveData & ToneData Decoders** (`src/gba/m4a/voice.rs`, `74a1b9c`):
  - 8-bit signed PCM `WaveData` parsing: format flag, loop flags, loop start offset, sample length, and frequency calculation (`freq / 4096.0` base rate).
  - 12-byte `ToneData` instrument headers: DirectSound PCM (type 0), CGB PSG channels 1-4, key splits (type 0x40), and rhythm/drum tables (type 0x80).
  - 48 kHz scaled ADSR envelope decoding: Attack rate, Decay rate, Sustain level, Release rate.
- ✅ **High-Quality 48 kHz Stereo Floating-Point Sampler** (`src/gba/m4a/sampler.rs`, `74a1b9c`):
  - Full 32-bit floating-point mixing pipeline eliminating 8-bit hardware quantization noise and harsh 60 Hz envelope stepping.
  - Three interpolation algorithms selectable at runtime:
    - `Linear`: Fast 2-point linear interpolation.
    - `CubicHermite`: 4-point Catmull-Rom cubic Hermite spline ensuring smooth $C^1$ continuity (recommended default).
    - `Sinc`: 8-point Hann-windowed band-limited Sinc interpolation for studio-grade reconstruction.
  - High-precision per-sample ADSR envelope generator with anti-click ramps.
  - CGB PSG synthesis for legacy square, wave, and noise channels blended into 48 kHz stream.
  - Equal-power stereo panning laws ($L = \cos \theta$, $R = \sin \theta$).
  - 4-comb stereo reverberation unit with stereo decorrelation and configurable wet/dry send level.
  - Oldest/lowest release voice stealing for configurable polyphony (up to 64 voices).
  - Smooth tanh soft limiting on master mix bus.
- ✅ **Standard MIDI File (.mid) Type 1 Binary Serializer** (`src/gba/m4a/midi_export.rs`, `74a1b9c`):
  - SMF Type 1 generator with `MThd` header and multiple `MTrk` chunks.
  - Conductor track with tempo changes (`0xFF 0x51 0x03`), time signature, and song title meta-events.
  - Music tracks with delta-time VLQ encoding, Note On/Off, Pitch Bend, Pan (CC 10), Expression/Volume (CC 7 / CC 11), Program Change, and End of Track (`0xFF 0x2F 0x00`).
- ✅ **Multi-Track 48 kHz WAV Stem Renderer** (`src/gba/m4a/stem_export.rs`, `74a1b9c`):
  - Renders isolated per-instrument stereo WAV files alongside a time-aligned master mix file.
  - Standard 44-byte RIFF/WAVE header writer supporting 16-bit PCM stereo at 48000 Hz.
- ✅ **Audio Mixer, Save State Sync, & Seamless Fallback** (`src/gba/apu/mod.rs`, `src/gba/mod.rs`, `74a1b9c`):
  - `AudioEngineMode` toggle: `HdReSynthesis` vs `HardwareOnly`.
  - When enabled, `mix_and_push_sample()` dynamically blends high-resolution 48 kHz re-synthesized music with native DMG PSG sound effects.
  - Games without M4A automatically fall back to native APU DirectSound audio with zero overhead.
  - Full save-state & rewind synchronization: WRAM state captures music sequence position, and `load_state()` resets the 48 kHz buffer for immediate jitter-free resynchronization.
- ✅ **UI Controls & CLI Tools** (`src/ui/audio_mixer_dialog.rs`, `src/ui/config.rs`, `src/main.rs`, `74a1b9c`):
  - Audio Mixer dialog features:
    - HD Audio status badge and mode switcher.
    - Radio buttons for Linear, Cubic Hermite spline, and Band-limited Sinc interpolation.
    - Stereo reverb toggle and send level slider.
    - Integrated Jukebox with song selector, Play, Stop, and active profile details.
    - File dialogs for MIDI (.mid) and multi-track WAV stem export.
  - CLI flags:
    - `--hd-audio` and `--hardware-audio`: toggle audio engine mode on startup.
    - `--export-midi <ROM> [--song N] [--output P]`: headless MIDI exporter.
    - `--export-stems <ROM> [--song N] [--output D] [--seconds S]`: headless 48 kHz multi-track stem renderer.
  - Settings persisted in `config.json` under `audio`.
- ✅ **Tests** (`tests/m4a_audio.rs`, `74a1b9c`):
  - 11 comprehensive integration tests covering:
    - `test_m4a_database_lookups_and_profiles`: verifies profiles across Emerald, Minish Cap, Fire Emblem (FE6/FE7/FE8), Metroid, Castlevania, and Golden Sun.
    - `test_m4a_universal_signature_auto_detection`: verifies `CLOCK_TABLE` pattern scanning and candidate table detection.
    - `test_m4a_voice_and_sample_parsing`: verifies WaveData (8-bit PCM, loop points, sample rates) and ToneData parsing.
    - `test_m4a_sampler_interpolation_and_adsr`: verifies Linear, Cubic Hermite, and Sinc interpolation, ADSR transitions, and decay.
    - `test_m4a_polyphony_voice_stealing`: verifies voice allocation limits and oldest-voice stealing.
    - `test_m4a_sequencer_bytecode_dispatch`: verifies Sappy bytecode processing (TEMPO, VOICE, VOL, PAN, BEND, NoteOn/Off, FINE).
    - `test_m4a_midi_export`: verifies SMF format 1 generation, track headers, events, and termination.
    - `test_m4a_stem_export_and_wav_writer`: verifies multi-track stem generation and RIFF/WAVE 48 kHz file serialization.
    - `test_m4a_non_m4a_graceful_fallback_and_mode_toggle`: verifies fallback to native hardware audio for non-M4A titles.
    - `test_m4a_pokemon_emerald_playback_and_export`: live test on real Pokémon Emerald ROM verifying BGM playback, 48 kHz sample generation, MIDI export, and stem export.
    - `test_m4a_save_state_synchronization`: verifies save-state save/restore roundtrip with zero phase desync or audio glitching.
- **"Done when" check:** Emerald, Minish Cap and Fire Emblem play re-synthesized music with correct tempo, looping and sound effects, and the audio mixer can switch between HD and hardware audio ✅.

---

## Stage 4: Accessibility and understanding the game

### M10. Accessibility pack ✅

- ✅ **Shared core** (`src/gba/accessibility.rs`, `6eedb8a`). Lives beside
  the emulator but holds no emulation state, so determinism, replays and
  save states are untouched.
  - Colorblind filters: Daltonization (Fidaner) on top of Viénot/Brettel LMS
    dichromat simulation for protanopia, deuteranopia and tritanopia, plus
    grayscale and high contrast, with a strength slider. Neutral grays pass
    through unchanged; a unit test checks that red/green pairs a deuteranope
    confuses end up ≥1.3× further apart after correction.
  - Toggle instead of hold for any of the 10 buttons (latch flips on each
    press edge; latch state is never saved).
  - `Handedness` (two hands / left / right) for keyboard and touch; UI scale
    0.75–2.5×.
  - `AccessibilityStore`: global default + per-game overrides keyed by game
    code (GB: header title). `AccessibilityManager::commit` saves edits for
    the loaded game automatically, doesn't create an override that just
    matches the default, and edits with no game loaded set the default.
- ✅ **Slow motion audio** (`src/gba/apu/slowmo_stretch.rs`,
  `apu/audio_output.rs`, `apu/resample.rs`). 10–100% speed, three sound
  modes:
  - *Stretch*: WSOLA time stretcher (1024-frame Hann grains, 50% overlap,
    ±256-frame waveform-similarity search). Pitch is unchanged.
  - *Tape*: the resampler reads the input at `speed`, so pitch drops.
  - *Mute*: silence of the right length.
  - At speed `s` the core delivers audio in one burst every 1/(60·s) s, so
    the output queue band scales with speed (`slowmo_band`) and is pre-filled
    with silence on entry. After slow motion, whole crossfaded grains are
    dropped until the queue is back to normal latency (~0.5 s instead of the
    minute the ±0.5% rate control would need). Queue capacity grew to 64 K
    samples; normal-speed latency is unchanged (same 1000–3000 band).
- ✅ **Desktop** (`src/ui/accessibility_dialog.rs`, `ui/mod.rs`,
  `ui/controls.rs`, `ui/screen.rs`):
  - Accessibility dialog (Ctrl+U or Emulation menu) with a live palette
    preview; slow-motion presets also in Emulation › Speed.
  - One-handed keyboard presets (left: WASD/Q/E/Tab/R/Z/X; right:
    IJKL/U/O/Y/P/M/Enter) are an overlay: "Two hands" is always the user's
    own remapped bindings. A test checks the presets have no duplicate keys
    and don't collide with the fixed hotkeys (0–9, F1/F3/F6/F7, N, .).
  - The filter runs on the game's colors before frame blending, shaders,
    xBRZ and sharpening, on every render path (native, HD Mode 7 with and
    without SSAA, widescreen).
  - UI scale uses egui's zoom factor, applied once the mouse is released so
    the slider doesn't jump under the pointer.
  - Settings persist in `config.json` under `accessibility`; configs from
    before M10 still load.
  - CLI: `--dump-frame <ROM> --colorblind <mode>`.
- ✅ **Android** (`android/src/app.rs`, `android/src/touch.rs`):
  - "Accessibility..." in the in-game menu opens a touch-sized sheet (colors,
    strength, slow motion, slow-motion sound, touch layout, interface size,
    toggle buttons, use for all games / reset). Emulation pauses while it's
    open. Saved per game to `files/accessibility.json`.
  - One-handed touch layouts: every control stacked in one column on the
    chosen side (portrait) or a side column with the game filling the rest
    (landscape). Host test checks, at four phone sizes × both hands, that
    every control is on screen, off the game image, within the side 60% of
    the width in portrait, and hit-tests to exactly its own button.
- ✅ **Tests**:
  - `tests/accessibility.rs` (8): slow motion against a simulated real-time
    48 kHz device at 75/50/25/10% with **zero underruns** and pitch within
    3% (pitch-preserved), halved pitch (tape), gap-free silence (mute);
    switching speeds mid-play with no warm-up and no underruns; returning to
    normal latency after slow motion; a control test showing that without
    the stretcher 50% speed starves the device; the filter on a real
    emulator frame; sticky B driving the keypad over 30+ frames; per-game
    persistence round-trip through `AppConfig` JSON and the Android store.
  - Unit tests: filter neutrality and deutan separation, sticky edges,
    per-game commit rules, latch not persisted, WSOLA length/pitch/no clicks,
    tape resampling, keyboard preset clashes; Android host tests 8/8.
  - Full `cargo test --release`: **562 passed, 0 failed** (19 ignored), incl.
    jsmolka, mGBA suite ratchet, Blargg, determinism, replay golden,
    run-ahead and M4A.
- ✅ **Verified live**:
  - Emerald frames dumped through each filter look clean (no banding or
    clipping).
  - Android emulator (x86_64 debug APK): opened the sheet on Emerald, set
    Deuteranopia, 75%, 125% UI, left-hand layout and toggle-B. After a
    reinstall + restart, the library stayed at the global 100% while Emerald
    came back with its settings, filtered picture and one-handed layout.
    Found and fixed on device: the sheet overflowed the screen at 125%.
  - Release APK (arm64 + x86_64) builds.
- **"Done when" check:** every option works on desktop and Android and is
  saved per game ✅.
- Not done: the desktop filter runs on the CPU per pixel (cheap: frames are
  240×160 and runs of equal colors reuse the last result); the desktop GUI
  was checked by build + tests, not by clicking through it in a live window.

### M11. Automatic game-variable discovery ✅

- ✅ **Memory maps** (`src/gba/memmap/mod.rs`, `653d11d`). A per-game list of
  named variables: numbers, flags, player X/Y and text buffers. A variable's
  location is either absolute or `[pointer]+offset`, and a value can be
  XOR-encoded with a key that is also relocatable (Gen 3 money). Maps are
  saved as JSON under `<config dir>/memmaps/<GAMECODE>.json` and load
  automatically with the game.
- ✅ **Guided search** (`memmap/search.rs`):
  - Relation filters (equals, changed, increased, changed by N...).
  - XOR pairs found in linear time by indexing every RAM word once.
  - Each hit is turned into every IWRAM-pointer-relative form that reaches
    it, and only forms that still read correctly in other sessions are kept.
- ✅ **Discovery** (`memmap/discover.rs`). The player records snapshots
  labelled with what the screen shows; they never give an address.
  - **Sessions:** each reset starts a new session. Emerald moves its save
    blocks and money key to new addresses on each boot (checked by
    booting with different idle times). Requiring a hit to hold in every
    session is what removes coincidental matches.
  - **Numbers:** search in session 1, then confirm the survivors in the
    other sessions.
  - **Position:** built from "moved dx, dy" steps; a candidate has to follow
    every step, both moves and standing still.
  - **Flags (the "light machine learning"):** for every byte, learn a
    decision stump (`== v`, `!= v`, `in {..}`) from session 1's labelled
    snapshots, then keep it only if it classifies every held-out session
    correctly. Simpler rules rank higher.
  - **Text:** a buffer that holds the same bytes while one message is up
    and different bytes for another message, whose opening bytes read like
    text (not a repeating tile pattern) and appear verbatim in the ROM.
  - **Ranking:** the simplest location wins (absolute before pointer, u16
    before u32 before u8). Forms that reach the same bytes are merged.
  - **"Ask me"** (`PokeRefiner`): when a number still has several
    candidates, write a test value into half of them, ask what the game
    shows, and restore the save state. Each answer halves the list.
- ✅ **Desktop UI** (`src/ui/memmap_dialog.rs`; Debug menu or Ctrl+J):
  - Record numbers, steps, yes/no facts and messages (same or new).
  - Sessions advance automatically on reset.
  - Discover shows a per-variable report; ambiguous numbers go to the
    "Ask me" section.
  - Each variable shows its live value and has "Watch" (adds it to the hex
    editor's watch list) and delete. Save the map from the dialog.
  - The Gen 3 companion shows the discovered variables live.
- ✅ **Tests**:
  - `tests/memmap_discovery.rs` (needs the Emerald ROM and save): three
    boots with relocated save blocks. The scripted player walks, reads a
    sign, opens the start menu and save prompt, and fights until it takes
    damage. It labels only what's visible on screen. Known Gen 3 addresses
    are used just to label and to grade the result; discovery never sees
    them. Graded against those sessions plus a fresh fourth boot:
    - **HP** `0x02024542` u16: exact; only 2 candidates after session 1.
    - **Money** `[0x03005D90]+0xAC u32 xor [0x03005D8C]+0x490`: the real
      encrypted, pointer-relative field. Money never changes during the
      tour, so about 790 constant words also fit; "Ask me" narrowed them to
      1 in 10 questions by reading the trainer card.
    - **Player X/Y** `0x02037360/2`: the player's object-event coordinates.
      They track movement exactly in every session with a constant +7
      offset (the engine's map border).
    - **Text box open** `0x02001BC0 != 0`: correct while walking, and
      correct with a sign open in the fresh session. It is also off in the
      start menu.
    - **Text box text** `0x02021FC4`: holds the sign's string in the fresh
      session; its first 12 bytes are found in the ROM.
    - The map is saved, reloaded, and reads the same values.
    - Discovery takes about 0.27 s for 46 snapshots.
  - `tests/memmap_ui.rs`: the real dialog runs in a headless egui context
    and is driven by clicks and typing, finding widgets by label through
    AccessKit. It records two sessions, runs Discover (finds HP as
    `[0x03000100]+0x20` despite decoys), uses Watch, saves, and reloads the
    map on the next load. A second test runs "Ask me" to a single answer
    and checks every poked value was restored.
  - Unit tests: toy game fully discovered, stump learner, XOR relocation,
    JSON round-trip.
  - Full `cargo test --release`: **579 passed, 0 failed**. Android still
    builds.
- **"Done when" check:** for Emerald it finds HP, position, money and
  text-box state without being told where they are, and the map is saved
  and reused ✅.
- Notes:
  - X/Y came out as the object-event copy (+7) rather than the save-block
    copy. Both track the player exactly. The reported offset is constant,
    so the map is right for relative use, but a consumer that needs
    absolute map coordinates should subtract 7 for Gen 3.
  - "Text box open" was trained on signs, a save prompt and the start
    menu; battle text wasn't in the training set.
  - Android has no discovery UI (a desktop tool), but maps are plain JSON
    and the core API builds for Android.

---

## Performance Track: Mobile Thermal & Battery Optimization

### Stage 4a. Idle-Loop Detection & Busy-Wait Skipping ✅

- ✅ **Idle-loop skip core** (`src/gba/idle.rs`, `src/gba/mod.rs`, `src/gba/mmu/mod.rs`):
  - Detects backward branch loops (<= 64 bytes) that make no memory writes,
    no IO register reads, and keep CPU registers and condition flags identical.
  - Skips whole loop iterations directly to the next peripheral event horizon
    (scanline boundary, timer overflow, APU sample, or pending IRQ).
  - Automatically catches up batched peripherals and synchronizes timing clock
    when skipping, preserving exact event order and peripheral states.
  - Only skips RAM-polling loops (e.g. Pokémon Emerald VBlank wait at `0x080008c6`),
    guaranteeing 100% bit-identical frame and audio execution.
- ✅ **Tests & Verification** (`tests/idle_skip.rs`, `tests/halt_skip.rs`):
  - `emerald_idle_skip_reduces_cpu_work_in_gameplay`: **51.5% cycle reduction**
    (8,670,000 / 16,850,000 cycles skipped per second) in active Emerald gameplay,
    slashing CPU load and battery consumption on mobile.
  - `idle_skip_is_bit_identical`: 100% bit-identical hashes across 500 frames for
    Pokémon Emerald, Dragon Ball, Pokémon Sapphire, Harry Potter, and GBA test suite.
  - `batched_peripherals_are_bit_identical`: Confirmed bit-identical with full
    batched peripheral catchup over 2,000 frames.

---

## Stage 6: Android Platform Excellence (Tier-1 Primary Target)

### M18. Android release & core UI ✅
- Multi-ABI release APK (arm64-v8a + x86_64) with native static linking and high-precision GLES shaders.
- Custom animated skin packs, on-screen touch layout editor, in-game modal menu,
  fast forward, quick save/load, auto-save rotations, gyro/tilt sensor.

### M18a. Scoped Storage ROM Library Scanner (SAF Auto-Scan) ✅
- ✅ **SAF Folder Integration** (`android/java/.../MainActivity.java`, `android/src/platform.rs`):
  - Uses `Intent.ACTION_OPEN_DOCUMENT_TREE` with `FLAG_GRANT_PERSISTABLE_URI_PERMISSION`.
  - Persists tree URI permissions in `SharedPreferences` across app restarts.
  - Iterative breadth-first subdirectory scanner using Android's standard `DocumentsContract`
    (supports `.gba`, `.gb`, `.gbc`, and `.nds` ROMs up to 3 subdirectories deep).
  - Incremental sync: only copies new or changed ROMs, avoiding redundant disk I/O.
  - Two-way battery save (`.sav`) synchronization: auto-imports existing `.sav` files on scan,
    and automatically pushes updated `.sav` files back to the external SAF tree on flush.
- ✅ **UI & Workflow** (`android/src/app.rs`):
  - Primary "Select ROMs Folder (SAF Auto-Scan)" onboarding CTA in empty library.
  - Library toolbar shows connected folder pill with quick `🔄 Rescan` trigger and status indicators.
  - Folder management dialog supporting instant rescan, folder switching, and unlinking.

### M18b. Audio-Driven Haptic Rumble & Physical Tilt ✅
- ✅ **Authentic Cartridge Gyro/Tilt** (`android/src/app.rs`, `android/src/tilt.rs`):
  - Automatically enables phone motion sensors when a `SensorType::GyroTilt` cartridge is detected
    (*WarioWare Twisted!*, *Yoshi Topsy-Turvy*).
  - Rotates phone gravity/accelerometer vectors into screen-space coordinates and feeds
    normalized tilt angles directly to `cart.sensors.set_tilt(tx, ty)`.
- ✅ **Dynamic LRA Force Feedback & Audio Bass Haptics** (`MainActivity.java`, `android/src/app.rs`):
  - Hardware Cartridge Rumble: Drives phone vibrator with amplitude-controlled force feedback
    when GBA games trigger the rumble motor (*Drill Dozer*, *Pokémon Pinball*).
  - Audio-Driven Bass Haptics: Analyzes real-time low-frequency energy (single-pole low-pass filter
    on APU scope buffer) to trigger tactile LRA feedback during explosions, impacts, and heavy attacks.
  - Added user toggles in startup settings and in-game menu.


