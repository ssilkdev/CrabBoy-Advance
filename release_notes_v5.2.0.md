# 🦀 CrabBoy Advance 5.2.0 — Animated Skins, Android Touch Overhaul & Audio Engine Fidelity

This release brings animated skin packs with touch control customization, comprehensive audio engine fixes, and performance optimizations across the JIT interpreter.

---

## 🎨 Animated Skins & Touch Controls (Android)

* **Animated Skin Engine:** Full support for multi-frame sprite sheets, looping background grids, and pulsing touch controls with pressed flares.
* **New "Emerald Synthwave" Theme:** Blends classic 80s outrun synthwave (seamless rolling perspective grid, scanline retro sun, celestial delta crest, and cosmic starfield) with Pokémon Emerald's Rayquaza aesthetics. Available ready-to-use in `docs/skins/Emerald-Synthwave.zip`.
* **Touch Layout Editor:** Drag to reposition and resize any control on-screen with separate portrait and landscape layout persistence.
* **Skin Pack Import/Export:** Direct in-app `.zip` skin importer with automated validation and hot-reloading.

---

## 🔊 Audio Engine Fidelity & Mixing Overhaul

* **DirectSound PCM Restoration:** Fixed DirectSound Channels A & B being silenced during normal gameplay, restoring drums, brass, attack sound effects, and Pokémon cries.
* **Pristine Stereo Output:** Corrected the default audio spatialization mode from 3D Headphone crossfeed to pure, uncolored stereo. Eliminates inter-channel comb filtering, phase cancellation, and muddy artificial bass distortion on mobile speakers.
* **In-Game Audio Toggles (Android):** Added on-the-fly toggles in the pause menu for **Audio Engine** (`Native APU` ⇄ `HD Re-Synthesis`) and **Spatial Mode** (`Pure Stereo` ⇄ `3D Headphones` ⇄ `5.1 Surround`).
* **Zero-Crossing Glitch Fix:** Eliminated audio discontinuities in HD Music Re-Synthesis caused by per-sample zero-crossing amplitude thresholds.
* **Lock-Free Stereo Frame Synchronization:** Added atomic stereo frame popping (`pop_frame`) and even-aligned pushes in `SampleRing` to prevent 1-sample channel phase offsets and buffer underruns during OS scheduling jitter.

---

## ⚡ Performance: JIT Stage 2 & 3a

* **ARM Decode Table & Renderer Fast Paths:** Direct instruction lookup table and scanline rendering fast paths while preserving 100% bit-identical CPU/PPU accuracy.
* **Batch Peripheral Stepping:** Event-horizon deferred peripheral catch-up delivering +33–59% higher emulation throughput.

---

## 📦 Downloads & Verification

| Asset | Description |
|---|---|
| `crabboy-advance-v5.2.0-linux-x86_64.tar.gz` | Linux x86_64 portable desktop bundle |
| `crabboy-advance-release.apk` | Android release APK (`arm64-v8a` + `x86_64`) |
| `Emerald-Synthwave.zip` | Animated custom skin pack for Android |
| `SHA256SUMS.txt` | SHA-256 verification checksums for all release assets |
