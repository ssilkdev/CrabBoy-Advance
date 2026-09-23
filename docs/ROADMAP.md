# CrabBoy Advance — Feature Roadmap

An ordered sequence of milestones. There are no dates: each milestone starts
once the ones it depends on are done. Milestones in the same stage that don't
depend on each other can run in parallel.

Tags:
- **[New]**: no mainstream GBA emulator does this, as far as we know.
- **[Catch-up]**: mGBA, NanoBoyAdvance or RetroArch already have it.

Multiplayer work (rollback netplay, Wireless Adapter emulation) is left out of
this roadmap on purpose.

---

## Stage 1: Foundations

Almost everything later depends on the emulator being accurate and
deterministic, so this comes first.

### M1. Accuracy baseline [Catch-up]
- Run the mGBA test suite, the AGS aging cartridge and the existing GB test
  ROMs in CI, and publish the pass rate.
- Fix the RTC bug that makes Pokémon Emerald report "The internal battery has
  run dry" on a fresh save (`src/gba/mmu/rtc.rs`).
- Close the remaining open items in `docs/AI_AGENT_FIX_DESIGN.md`.
- **Done when:** the pass rate is tracked in CI, can only go up, and the
  Emerald RTC message is gone.

### M2. Deterministic core and fast save states [Catch-up]
- The same ROM, save and inputs always produce identical frames and audio.
  Remove any dependence on wall-clock time or host timing inside the core.
  The RTC gets a controllable clock.
- Save states round-trip byte-for-byte, and are small and fast enough to take
  every frame.
- CI replays a recorded movie (the existing `.tas` format) and compares a hash
  of every frame.
- **Depends on:** M1.
- **Done when:** the replay test passes on Linux, Windows and Android, and a
  save state plus restore fits comfortably inside one frame's time budget.

### M3. Run-ahead [Catch-up]
- Hide one or two frames of the game's built-in input lag by emulating ahead
  and rolling back. Configurable per game, with an optional second instance so
  audio doesn't glitch.
- **Depends on:** M2.
- **Done when:** measured input-to-screen latency drops by the configured
  number of frames with no audio artifacts.

### M4. Save sync between desktop and Android [Catch-up]
- Battery saves and save states sync through a folder the user picks
  (Syncthing, Google Drive, Dropbox). Newest wins, and the older copy is kept
  as a backup.
- The same save-state format on both platforms.
- **Depends on:** M2 (portable save states).
- **Done when:** a game started on desktop continues on Android and back
  again with nothing lost.

---

## Stage 2: Rendering

### M5. Rendering pipeline groundwork: frame blending and shaders [Catch-up] ✅
- LCD ghosting / frame blending, which some games rely on for transparency
  (flickering sprites).
- A shader pipeline alongside xBRZ and NIS: built-in CRT and LCD-grid presets,
  plus shaders the user can load.
- Restructure the PPU so it can emit layers and draw commands, not only a
  finished 240×160 buffer. M6–M8 need this.
- **Depends on:** M1.
- **Done when:** the presets work on desktop and Android, and the PPU exposes
  per-layer output without changing the native-resolution image.

### M6. HD Mode 7 [New] ✅
- Render affine backgrounds and affine sprites at 4–8× internal resolution,
  the way bsnes-HD does for the SNES. Rotation and scaling become smooth
  instead of blocky and aliased.
- Targets: F-Zero, Mario Kart: Super Circuit, Golden Sun's world map, and the
  Pokémon battle intro swirls.
- **Depends on:** M5.
- **Done when:** these games render correctly at every scale, with no seams
  where HD layers meet native-resolution layers.

### M7. HD sprite and tile replacement packs [New for GBA] ✅
- Swap tiles and sprites for high-resolution art, keyed by a hash of the
  tile's pixels and palette. This is the approach Mesen uses for the NES.
- Includes a tile dump tool and a pack format with a manifest, so artists
  can make packs.
- **Depends on:** M5.
- **Done when:** a sample pack replaces a game's sprites with animation and
  palette changes still working.

### M8. Per-game widescreen [New for GBA] ✅
- For games whose engines already keep content outside the 240-pixel view,
  draw that extra area to fill 16:9. Controlled by a per-game database of
  safe settings and patches.
- **Depends on:** M5, and M6 for affine-heavy games.
- **Done when:** at least three games run in widescreen with no visual
  errors.

---

## Stage 3: Audio

### M9. HD music re-synthesis [New] ✅
- Most GBA games use Nintendo's shared M4A ("Sappy") music engine. Detect it
  and intercept its note and instrument data, then play the music through a
  high-quality sampler at 48 kHz instead of the hardware's 8-bit mixer.
- Export per-instrument stems and MIDI files from the emulator.
- Falls back to normal hardware audio for games that don't use M4A.
- **Depends on:** M2 (audio must stay in sync through save states and
  rewind).
- **Done when:** Emerald, Minish Cap and Fire Emblem play re-synthesized
  music with correct tempo, looping and sound effects, and the audio mixer
  can switch between HD and hardware audio.

---

## Stage 4: Accessibility and understanding the game

### M10. Accessibility pack [Catch-up] ✅
- Colorblind filters, a global slow-motion setting, toggle-instead-of-hold
  for any button, one-handed control layouts (desktop and Android), and
  interface scaling.
- **Depends on:** M5 (filters run through the shader pipeline).
- **Done when:** every option works on both desktop and Android and is saved
  per game.

### M11. Automatic game-variable discovery [New]
- A guided RAM search with light machine learning that finds variables like
  HP, player position, money, menu state and the text-box buffer, then saves
  them as a per-game "memory map".
- Builds on the existing memory hex editor and the Gen 3 companion.
- **Depends on:** M2 (search needs repeatable runs).
- **Done when:** for Emerald it finds HP, position, money and text-box state
  without being told where they are, and the map is saved and reused.

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

### M14. AI hint mode [New]
- The AI agent stops playing and advises instead: on request it looks at the
  screen and the M11 memory map and gives a hint, with a spoiler-level
  setting.
- **Depends on:** M11.
- **Done when:** hints are relevant to what's on screen in a set of scripted
  "stuck" scenarios, and the spoiler setting is respected.

---

## Stage 5: Tools

### M15. Time-travel debugger [New for GBA]
- Step backwards one instruction or one frame at a time, and ask "who last
  wrote this byte?" to jump back to that exact instruction.
- Combines rewind, the disassembler and the memory viewer.
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

## Stage 6: More platforms and extras

These don't depend on each other. Pick them in any order after Stage 5, or
earlier when there's spare capacity.

- **M18. Android release:** test on real arm64 phones, then a Play Store /
  F-Droid build with a stable signing key. (Can start any time; it only needs
  the existing `android/` front-end.)
- **M19. Web (WASM) build:** try CrabBoy in a browser. Depends on M2.
- **M20. libretro core:** reach RetroArch users. Depends on M2.
- **M21. Super Game Boy borders** for GB games.
- **M22. e-Reader support.**

---

## Out of scope

- **Multiplayer**: rollback netplay and Wireless Adapter emulation.
- **Nintendo DS support**: roughly a second emulator's worth of work; see
  `docs/DS_FEASIBILITY.md`.
- **JIT recompiler**: the interpreter already runs at full speed with plenty
  of headroom, including on Android.

## Dependency summary

```
M1 ─┬─ M2 ─┬─ M3
    │      ├─ M4 ── M16
    │      ├─ M9
    │      ├─ M11 ─┬─ M12 ── M13
    │      │       ├─ M14
    │      │       └─ M17 (also M2)
    │      ├─ M15
    │      └─ M19, M20
    └─ M5 ─┬─ M6 ── M8
           ├─ M7
           └─ M10
```
