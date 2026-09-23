# CrabBoy Advance for Android

A touch-first Android front-end over the same GBA / GB / GBC emulator cores as
the desktop app. The cores (`src/gba`, `src/dmg`) are shared unchanged; this
directory only adds the Android shell.

## What it does

- **Game library**: import `.gba`, `.gb` and `.gbc` ROMs through the system
  file picker. They are copied into app-private storage, so no storage
  permission is needed.
- **Touch controls**: multi-touch D-pad (with diagonals), A/B, L/R,
  Start/Select, menu and fast-forward. Separate portrait and landscape
  layouts, clear of camera cutouts.
- **Controllers**: Bluetooth or USB gamepads work without setup. The buttons
  map to where they sit on a GBA: the right face button is A and the bottom
  one is B. L1/L2 are L, R1/R2 are R. The D-pad or left stick moves.
  Select+Start or the Home/Mode button opens the menu. The touch overlay hides
  while a controller is in use.
- **Saves**: battery saves (`.sav`) are written every second, when a game
  closes, and when the app goes to the background. To bring in a desktop
  save, import its `.sav` with the same base name as the ROM. The menu also
  has one save-state slot per game.
- **Audio**: cpal → Oboe (AAudio/OpenSL ES).
- **Back button**: opens and closes the in-game menu.

Left out on purpose (desktop only): the updater, AI agent, web/game guides,
GIF recorder, TAS, link cable, debug windows, cheats UI, xBRZ filters and the
5.1 audio mixer UI.

## Build

Requirements: JDK 17+, the Android SDK (`platforms;android-35`,
`build-tools;35.0.1`), NDK r27+, `cargo install cargo-ndk`, and
`rustup target add aarch64-linux-android x86_64-linux-android`.

```sh
export ANDROID_HOME=~/android/sdk JAVA_HOME=~/android/jdk-17...
android/build-apk.sh                          # arm64 phones -> android/build/crabboy-advance-release.apk
android/build-apk.sh --abi arm64-v8a,x86_64   # + x86_64 for the Android emulator
```

The script uses the SDK tools directly (javac, d8, aapt2, zipalign,
apksigner), with no Gradle. It signs with `$CRABBOY_KEYSTORE` /
`$CRABBOY_KEYSTORE_PASS` if they are set. Otherwise it creates a local
`android/debug.keystore`, which is fine for sideloading. Keep the same
keystore between builds, or Android refuses to install an update over an
existing copy.

Host unit tests for the touch layout and controller mapping:

```sh
cargo test --manifest-path android/Cargo.toml
```

## Layout

- `src/app.rs`: library, emulation loop (paced to 59.73 Hz), in-game menu, saves
- `src/touch.rs`: on-screen control layout, hit testing and drawing (host-tested)
- `src/keymap.rs`: controller key/axis → GBA button mapping (host-tested)
- `src/gamepad.rs`: receives controller events from the winit hook
- `src/platform.rs`: JNI calls into `MainActivity`
- `java/.../MainActivity.java`: `NativeActivity` subclass for the file picker, ROM/save import, fullscreen and cutout insets
- `build.rs`: links the NDK C++ runtime statically (needed by Oboe)
- `patches/winit`: winit 0.30.13 plus a small gamepad hook (see below)

The root crate's GUI is behind the default `desktop` feature. This crate
depends on it with `default-features = false`, so eframe/rfd/gilrs and the
other desktop dependencies are never built for Android.

### The winit patch

Stock winit on Android marks every key event as handled, so controller
buttons (`KEYCODE_BUTTON_*`) come through as unidentified keys that egui
drops, and joystick motion is treated as touch input. The patch adds
`winit::platform::android::gamepad::set_gamepad_hook`. Once a hook is
installed, key and motion events from gamepad, joystick or D-pad sources, and
the system Back key, go to the hook instead. The change is limited to
`src/platform/android.rs` (new module) and one `match` in
`src/platform_impl/android/mod.rs`, both marked `CrabBoy Advance patch`.
