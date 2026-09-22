# Nintendo DS Support — Feasibility Assessment

**Repo:** `/home/ssilk/CrabBoy-Advance` @ `11ac569` (+ staged GB/GBC core)
**Question:** what would it take to run DS games in CrabBoy Advance?
**Date:** 2026-09-21

---

## 1. Verdict

**Technically feasible, but this is not a feature — it is a second emulator living in the same binary.**

The GB/GBC core landed in one session because the hard parts were already
built: the PSG channels already existed (`src/gba/apu/dmg.rs`), the display
pipeline already took a `u32` framebuffer, and the SM83 is a 500-opcode 8-bit
CPU. Roughly **80% of that feature was reuse.**

DS inverts that ratio. Realistic reuse is **~15%**, and it is concentrated in
the shell (audio sink, window, file dialog, save-slot plumbing) rather than in
anything that emulates hardware. Everything that makes a DS a DS — two CPUs of
different architectures, a programmable VRAM bank matrix, a hardware 3D
pipeline, a 16-channel ADPCM mixer, an encrypted cartridge bus — has **zero**
precedent in this codebase.

Reference scale: a DS-only melonDS core (excluding DSi, WiFi, JIT, OpenGL
renderers) is **~25,400 lines of C++**. CrabBoy Advance's entire GBA core is
**5,487 lines**. The DS core alone would be roughly **4x the size of
everything currently in `src/gba/`.**

The single largest risk is **not** the ARM9. It is the **3D geometry engine +
rasterizer** (~4,800 lines in melonDS), which has no analogue anywhere in this
project and cannot be partially implemented — a game either gets correct
polygons or it renders garbage.

---

## 2. What is genuinely reusable

| Capability | Location | What it gives DS |
|---|---|---|
| Audio output sink | `src/gba/apu/audio_output.rs:318` `push_sample_batch`, `:244` `sample_rate` | Fully reusable. Resampling, surround, mute mask, fast-forward — DS SPU just feeds it a stereo batch. |
| Multi-core UI dispatch | `src/ui/emu_core.rs:9` `SnapshotCore`, `:37` `ConsoleKind` | Already generic. Adding `ConsoleKind::Ds` + `impl SnapshotCore for Nds` is a ~20-line change. |
| Save slots / rewind | `src/ui/save_manager.rs:50`, `src/ui/rewind.rs:30` | Already generic over `SnapshotCore` (done for GB/GBC). Works unchanged — *if* DS state size is tolerable (see risks). |
| HLE BIOS precedent | `src/gba/mmu/bios.rs:6` `execute_hle_swi` | Pattern, not code. Proves the project is willing to HLE a BIOS; DS needs the same trick twice over. |
| Core coordinator shape | `src/gba/mod.rs:90`/`:217`, `src/dmg/mod.rs:122`/`:168` | `step_instruction`/`run_frame`/`save_state` is a proven shape the UI already drives. DS fits the same interface. |
| Window / input / file dialog | `src/ui/mod.rs`, `src/ui/controls.rs` | Reusable shell. |

**Not reusable, despite appearances:**

- `src/gba/cpu/arm.rs` — ARMv4T only. The one "long multiply" hit at `:253` is
  `UMULL/SMLAL`, which ARMv4T already has. There is **no** CLZ, BLX, saturating
  arithmetic, DSP multiply, LDRD/STRD, or coprocessor support. More
  structurally, `step_arm(cpu, mmu: &mut Mmu)` (`arm.rs:7`, `thumb.rs:7`) is
  hard-wired to the concrete GBA `Mmu`. DS needs the same decoder driving two
  different buses.
- `src/gba/ppu/` — a useful *mental* template for BG/OBJ compositing, but DS
  VRAM is 9 programmable banks, not `Box<[u8; 96*1024]>` (`ppu/mod.rs:19`), and
  there are two independent engines plus a 3D layer.
- `src/gba/apu/` — 2 DMA FIFOs + 4 PSG channels. DS is 16 channels with ADPCM.
  Only the *sink* survives.

---

## 3. Integration points

| # | Change | Anchor | Nature |
|---|---|---|---|
| 1 | Abstract the CPU decoders over a bus trait | `src/gba/cpu/arm.rs:7`, `thumb.rs:7` | **Refactor of existing code** — the only change that touches working GBA paths |
| 2 | Add an event scheduler | *does not exist* (verified: no `Scheduler`/`BinaryHeap` anywhere in `src/`) | New subsystem |
| 3 | ARMv5TE decoder + CP15 + ITCM/DTCM | new `src/nds/cpu/` | New module |
| 4 | DS memory map, 9-bank VRAM matrix, IPC/FIFO, 4+4 DMA | new `src/nds/mmu/` | New module |
| 5 | 2D engines A & B | new `src/nds/gpu2d/` | New module |
| 6 | 3D geometry engine + software rasterizer | new `src/nds/gpu3d/` | New module — **the dominant cost** |
| 7 | 16-channel SPU → existing sink | new `src/nds/spu.rs` → `audio_output.rs:318` | New module, one-line tap |
| 8 | NDS cart (KEY1/KEY2 Blowfish), SPI bus, firmware, save auto-detect | new `src/nds/cart/`, `src/nds/spi/` | New module |
| 9 | Dual-screen display + touch input | `src/ui/screen.rs:201`, `:126`, `src/ui/mod.rs:276` | **Breaks a shared assumption** (see §5) |
| 10 | `ConsoleKind::Ds`, `.nds` extension, `impl SnapshotCore` | `src/ui/emu_core.rs:37`, `:43` | One-line taps |

---

## 4. Decision: 3D rendering approach

| Option | Accuracy ceiling | Effort | Notes |
|---|---|---|---|
| **Software rasterizer** | Highest — DS 3D is a fixed-function pipeline with quirks (W-buffer, edge marking, 4x4 texel compression, toon/highlight) that GPUs do not have | ~8–12 wk | Deterministic; works headless; keeps `--diagnose`/`--dump-frame`/TAS/AI-agent honest |
| OpenGL via `glow` | Medium — requires shader emulation of the same quirks anyway | ~6–9 wk + ongoing | `glow 0.16` and `egui_glow 0.31.1` are already in the tree via eframe |
| Hybrid (SW default, GL optional) | Highest | sum of both | Only worth it after software works |

**Recommendation: software rasterizer.**

The deciding factor is not performance — it is that **every differentiating
feature this project already has depends on headless determinism**:
`--diagnose`, `--dump-frame`, `--audit-audio`, the flight recorder
(`src/gba/diagnostics/flight_recorder.rs`), the TAS engine, and the AI agent
(`src/ui/ai_agent.rs:455` reads the framebuffer directly). A GL path would
require a live GPU context in CI and in every headless invocation, and would
make frame output driver-dependent. Software-first preserves the project's
existing character; GL can be added later as an opt-in speed path.

---

## 5. Risks and hard parts (honest)

**A. There is no scheduler, and DS cannot work without one.**
Both existing cores use "execute one instruction, then advance every peripheral
by that many cycles" (`src/gba/mod.rs:90-213`, `src/dmg/mod.rs:122-160`). That
works for one CPU. The DS has an ARM9 at 67 MHz and an ARM7 at 33 MHz that must
stay in lockstep across the IPC FIFO, shared main RAM, and the GXFIFO. Retrofitting
a proper event scheduler is **phase-0 work that must land before any DS code**, and
it is the one item that touches the working GBA/GB cores.

**B. The dual-screen change breaks a codebase-wide assumption.**
`&[u32; 240 * 160]` is hard-coded across `src/ui/screen.rs:201`,
`src/ui/gif_recorder.rs:68`, `src/ui/screenshot.rs:66`, and
`src/ui/ai_agent.rs:455`. The GB/GBC core dodged this by letterboxing into a
240x160 buffer (`src/dmg/mod.rs` `framebuffer_gba_sized`). **DS cannot dodge
it** — 2x(256x192) does not fit, and stacking gives 256x384, a portrait aspect
that `AspectRatio::calculate_target_size` (`src/ui/screen.rs:126`) was never
written for. This forces a real generalisation to `(width, height, &[u32])`
plus screen-layout modes (stacked / side-by-side / single).

**C. Touch input has no home in the existing input model.**
The keypad is a 10-bit word and the TAS format is `[bool; 10]`
(`src/ui/tas.rs:51`). Touch is `(x, y, pressed)`. This changes the **TAS file
format** — a breaking change to a shipped feature — and the AI agent's action
space (`src/ui/ai_agent.rs:374` `apply_inputs`).

**D. DS realistically requires user-supplied BIOS + firmware.**
The GBA core HLEs its BIOS (`src/gba/mmu/bios.rs`) and needs no external files —
a genuine selling point. DS needs an ARM9 BIOS, an ARM7 BIOS, and a firmware
image (touchscreen calibration, user profile). HLE is possible but is itself a
large sub-project, and firmware-dependent behaviour is a common source of
"works in melonDS, broken here" bugs. **This is a product-level change**: the
emulator stops being zero-setup.

**E. Save-state size explodes.**
DS state is ~8 MB (4 MB main RAM + 656 KB VRAM + TCM + registers) vs the GBA's
~400 KB. The rewind buffer defaults to 120 snapshots (`src/ui/rewind.rs:15`) —
that is **~960 MB of RAM** for DS. Rewind needs compression or a
different capture policy before it can be enabled for DS.

**F. Cartridge encryption is a hard gate.**
KEY1/KEY2 Blowfish encryption sits between the emulator and the ROM's secure
area. It is well-documented and bounded work, but it is a *blocking* item:
until it is right, commercial ROMs do not boot at all.

---

## 6. Phased plan

Every phase ends in something verifiable, mirroring how the GB/GBC core was
proven against Blargg's ROMs.

| Phase | Scope | Effort | Exit criterion (verifiable) |
|---|---|---|---|
| **0** | Event scheduler; abstract `step_arm`/`step_thumb` over a `Bus` trait | 2–3 wk | **GBA + GB test suites still pass** (`cargo test --release`, `--gb-serial cpu_instrs.gb`). No DS code yet. |
| **1** | ARMv5TE decoder, CP15, ITCM/DTCM, dual-CPU lockstep, main RAM, IPC FIFO, DMA | 3–4 wk | ARM CPU test ROMs (`armwrestler`, `FeOS` arm9 tests) pass via a `--nds-serial`-style harness |
| **2** | VRAM bank matrix, 2D engines A & B, display capture, master brightness | 4–6 wk | 2D homebrew renders correctly; `--dump-frame` PNGs match reference |
| **3** | NDS cart (KEY1/KEY2), SPI bus, firmware boot, save-type auto-detect | 3–4 wk | A commercial 2D game (e.g. *Pokémon HeartGold*) boots to its title screen and saves |
| **4** | 16-channel SPU (PCM8/16, ADPCM, PSG, capture) → existing sink | 2–3 wk | `--audit-audio` equivalent gives clean output on a known-good title |
| **5** | 3D geometry engine + software rasterizer | **8–12 wk** | *Mario Kart DS* / *Nintendogs* render correctly |
| **6** | Dual-screen UI, screen-layout modes, touch input, TAS format v2 | 2–3 wk | Touch-driven game playable; TAS record/replay round-trips |

**Total: ~27–39 weeks of focused engineering.**

For calibration: GB/GBC was **one session**. DS is **~20–30x that**, and unlike
GB/GBC there is no shortcut hiding in the existing code.

---

## 7. Recommendation

**Do not treat this as the next feature.** Three options, in the order I'd rank
them:

1. **Scope it down to a deliberate milestone.** Phases 0–3 (~12–17 weeks) yield
   a *2D-only DS core* that boots homebrew and 2D commercial games. That is a
   genuinely shippable, demonstrable result, and Phase 0 alone improves the
   existing GBA/GB cores by giving them a real scheduler. 3D becomes a separate
   decision made with real information.

2. **Do Phase 0 as a standalone spike first.** The scheduler + bus-trait
   refactor is valuable on its own merits, is low-risk, and is the honest test
   of whether the architecture tolerates a third core. If Phase 0 is painful,
   that is the cheapest possible signal to stop.

3. **Decline, and deepen instead.** `mem_timing.gb` still fails (no M-cycle bus
   timing), and GBA sub-instruction timing has the same gap. Fixing both would
   make CrabBoy Advance genuinely accurate on two consoles rather than
   approximate on three.

My pick: **option 2, then reassess.** It is two to three weeks, it cannot
produce a dead end, and it pays for itself even if DS is never built.
