# 🦀 CrabBoy Advance

A cycle-accurate, high-performance Game Boy Advance (GBA) emulator written in pure Rust with 5.1 Surround & 3D Spatial Audio, xBRZ / NVIDIA Image Sharpening, Live Rewind, Cheat Engine, Link Cable SIO Networking, Hardware Sensor Emulation, and a modern Windows 11 Fluent dark UI.

---

## 🌟 Key Features

- **ARM7TDMI CPU Core**:
  - Full 32-bit ARM and 16-bit THUMB instruction set decoders.
  - 37 banked registers across all operating modes (User, System, FIQ, IRQ, Supervisor, Abort, Undefined).
  - Fast barrel shifter (LSL, LSR, ASR, ROR, RRX) and condition code evaluation.
  - Cycle-stepped interrupt handling (VBlank, HBlank, VCounter, Timers, DMA).
- **PPU (Picture Processing Unit) & Display Pipeline**:
  - Modes 0, 1, 2, 3, 4, and 5 with background affine rotations, scaling, and priority compositing.
  - 128 hardware OAM sprites with affine rot/scale and double-size modes.
  - Windowing (WIN0, WIN1, OBJWIN) and alpha blending / special color effects.
  - **xBRZ High-Definition Upscaling**: 2x, 3x, 4x, 5x, and 6x pixel-art scaling.
  - **NVIDIA Adaptive Image Sharpening (NIS)** with directional contrast enhancement.
  - **4K Ultrawide & Auto-Integer Scaling** presets with ambient glow backdrops and GBA LCD color correction.
- **Audio Processing Unit (APU) & 5.1 Spatial Audio**:
  - DirectSound FIFO channels A & B with signed 8-bit circular buffers and timer-synchronized DMA streaming.
  - **ITU-R BS.775 5.1 Surround Sound**: Discrete 6-channel downmix or discrete multichannel output with dedicated LFE resonant sub-bass crossover.
  - **3D Headphone Spatializer**: Bauer binaural crossfeed with natural interaural time delay (ITD) to eliminate acoustic fatigue.
  - **6-Channel Audio Mixer**: Solo/mute matrix for DirectSound A/B and PSG Square 1/2, Wave, and Noise channels.
  - **Pitch-Preserved Fast-Forward DSP**: Smart mute and pitch-corrected audio playback when fast-forwarding.
- **Power Tools & Emulation Engine**:
  - **Live Rewind / Time Travel**: Real-time rollback ring buffer (hold `Backspace`/`Tab` or gamepad `L3`).
  - **Persistent 10-Slot Save State Manager** with GUI slot metadata inspector (`Ctrl+S`).
  - **Cheat Engine & RAM Searcher**: GameShark, Action Replay v3, CodeBreaker, and raw memory freeze (`Ctrl+C`).
  - **Multi-Player Link Cable (SIO)**: Hardware SIO registers and non-blocking UDP socket loopback for multi-instance multiplayer (`Ctrl+L`).
  - **Hardware Cartridge Sensors**: Solar photodiode counter, 12-bit SPI tilt/gyro with visual bubble level, and cartridge rumble pak (`Ctrl+Y`).
  - **Gen 3 RPG Companion**: Automatic SaveBlock memory inspection and cryptographic substruct decryption (`Ctrl+P`).
  - **Pure-Rust Animated GIF Recorder**: Lossless GIF89a gameplay video recorder with Netscape loop extension (`Ctrl+F12`).
  - **Retro GBA Shell Bezels**: Classic Indigo, Glacier Blue, GBA SP Flame Red, and Game Boy Player overlays with interactive button indicators.
  - **TAS Engine**: Frame-accurate macro recording, `.tas` script export/import, and single-frame stepping (`F6`, `.` / `N`).
  - **Memory Hex Editor & PPU Inspector**: Live byte delta heatmap highlighting, watchpoints, and 128-sprite OAM gallery.

---

## Controls

### GBA Gamepad
| GBA Button | PC Keyboard Key |
| :--- | :--- |
| **D-Pad** | Arrow Keys (Up, Down, Left, Right) |
| **A Button** | `Z` |
| **B Button** | `X` |
| **L Shoulder** | `A` |
| **R Shoulder** | `S` |
| **Start** | `Enter` |
| **Select** | `Backspace` |

### Emulation Hotkeys
| Key | Action |
| :--- | :--- |
| **Space (Hold)** | Turbo / Fast-Forward (4x Speed) |
| **P** | Pause / Resume Emulation |
| **F** | Single Frame Step (when paused) |
| **Ctrl + R** | Reset Emulation |
| **F5** | Quick Save State |
| **F8** | Quick Load State |

---

## Building and Running

### Prerequisites
- [Rust](https://www.rust-lang.org/) (1.75+ recommended)
- Windows 10/11 x64

### Build Release
```powershell
cargo build --release
```

The optimized executable will be located at:
```powershell
target/release/crabboy-advance.exe
```

### Run
```powershell
cargo run --release
```
Or launch directly with a ROM:
```powershell
.\target\release\crabboy-advance.exe "path\to\game.gba"
```

---

## Testing

Run the automated test suite:
```powershell
cargo test
```
