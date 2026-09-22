# 🦀 CrabBoy Advance

Created fully with AI using Google Antigravity with Gemini Flash 3.8 Flash and Claude Opus 4.6.

A cycle-accurate, high-performance Game Boy Advance (GBA) emulator written in pure Rust with 5.1 Surround & 3D Spatial Audio, xBRZ / NVIDIA Image Sharpening, Live Rewind, Cheat Engine, Link Cable SIO Networking, Hardware Sensor Emulation, and a modern Fluent dark UI. Runs natively on **Linux** and **Windows**.

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
  - **🤖 AI Agent Player**: Hand the controller to a vision LLM and watch it play (`Ctrl+A`). The agent screenshots the live GBA framebuffer, reasons about what's on screen, and drives the emulated keypad, while a spectator side panel streams its observation, current goal, held buttons, and input trace. Works with any OpenAI-compatible vision endpoint (llama.cpp `--mmproj`, Ollama, vLLM, OpenAI), plus an offline heuristic autopilot that needs no server at all. Inference runs on a worker thread, so the emulator never stalls.
  - **Embedded Strategy Guide & Manual**: 8-page illustrated Pokémon field guide baked into the binary with interactive chapter navigation, zoom, and PDF export (`F1`).
  - **Mascot Logo & Application Icon**: Custom Rust crab mascot, shipped as a multi-resolution Windows PE resource icon (`assets/icon.ico`) and as a freedesktop hicolor theme icon on Linux.
  - **Auto-Updater & GitHub Release Checker**: Asynchronous startup update detection with one-click in-place executable download and restart (`Help ➔ Check for Updates` or top menu `Update` tab).
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
| **F1** | Open Illustrated Trainer's Strategy Guide & Manual |
| **Ctrl + A** | 🤖 Open the AI Agent Player (watch an AI play) |
| **F5** | Quick Save State |
| **F8** | Quick Load State |

---

## Building and Running

CrabBoy Advance runs on **Linux** and **Windows** (x64). The emulator core is
pure portable Rust; only packaging, the self-updater and desktop integration
differ per platform.

> **Note:** the repository intentionally does **not** pin a default target in
> `.cargo/config.toml`. A default target would force every build to
> cross-compile and break native builds. Build natively with
> `cargo build --release`; pass `--target` only when cross-compiling.

### Linux

#### Prerequisites
- [Rust](https://www.rust-lang.org/) (1.75+ recommended)
- A C toolchain (`gcc`) and `pkg-config`
- Development headers for audio, input and the windowing stack. The GUI
  (`eframe`/`winit`/`glow`) links against X11, Wayland and OpenGL, `cpal` needs
  ALSA, and `gilrs` needs udev for gamepad support:

```bash
# Debian / Ubuntu
sudo apt install build-essential pkg-config \
    libasound2-dev libudev-dev libgl1-mesa-dev libxkbcommon-dev \
    libwayland-dev libx11-dev libxrandr-dev libxi-dev libxcursor-dev

# Fedora
sudo dnf install gcc pkgconf-pkg-config alsa-lib-devel systemd-devel \
    mesa-libGL-devel libxkbcommon-devel wayland-devel \
    libX11-devel libXrandr-devel libXi-devel libXcursor-devel

# Arch
sudo pacman -S base-devel pkgconf alsa-lib systemd-libs mesa \
    libxkbcommon wayland libx11 libxrandr libxi libxcursor
```

#### Build and run
```bash
cargo build --release
./target/release/crabboy-advance "path/to/game.gba"
```

#### Install (desktop integration)
Installs the binary, icon, `.desktop` entry and the `.gba` MIME association, so
the emulator appears in your application menu and ROMs open with a double-click:

```bash
./packaging/linux/install.sh              # per-user, into ~/.local
sudo ./packaging/linux/install.sh --system  # system-wide, into /usr/local
./packaging/linux/install.sh --uninstall
```

A per-user install requires `~/.local/bin` on your `PATH` (the script warns if
it is missing).

#### Package a release tarball
```bash
./scripts/package_release.sh 5.1.0
```
Produces a reproducible `target/crabboy-advance-v5.1.0-linux-x86_64.tar.gz`
plus `SHA256SUMS.txt`.

### Windows

#### Prerequisites
- [Rust](https://www.rust-lang.org/) (1.75+ recommended)
- Windows 10/11 x64
- **LLVM MinGW (MSVCRT)**: `winget install MartinStorsjo.LLVM-MinGW.MSVCRT`. This project builds for the `x86_64-pc-windows-gnullvm` Rust target (see `.cargo/config.toml`) rather than plain `x86_64-pc-windows-gnu`, because the GUI/windowing stack (`eframe`/`winit`/`glow`) needs the ABI this toolchain provides — building with a plain MinGW-w64 GCC toolchain compiles and links successfully but crashes at runtime.
  - If `cargo build` fails with a missing linker, the installed toolchain's version-dated folder name in `.cargo/config.toml` may not match your install; check `%LOCALAPPDATA%\Microsoft\WinGet\Packages\MartinStorsjo.LLVM-MinGW.MSVCRT_*\` and update the two paths there.

#### Build Release
```powershell
cargo build --release --target x86_64-pc-windows-gnullvm
```

The optimized executable will be located at:
```powershell
target\x86_64-pc-windows-gnullvm\release\crabboy-advance.exe
```

(With an MSVC host toolchain, a plain `cargo build --release` produces
`target\release\crabboy-advance.exe` instead.)

#### Run
```powershell
.\target\x86_64-pc-windows-gnullvm\release\crabboy-advance.exe "path\to\game.gba"
```

### Headless / CLI (all platforms)

The emulator runs without a display for diagnostics and frame capture:

```bash
crabboy-advance --diagnose   game.gba --frames 300 --output report.json
crabboy-advance --dump-frame game.gba --frame 60   --output frame.png
crabboy-advance --audit-audio game.gba --frames 300
crabboy-advance --help
```

---

## Testing

Run the automated test suite:
```bash
cargo test
```
