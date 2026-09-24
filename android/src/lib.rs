//! CrabBoy Advance for Android.
//!
//! A touch-first front-end over the same emulator cores the desktop build
//! uses (`gba_simulator::gba` / `gba_simulator::dmg`). Rendering and touch
//! input go through eframe/egui on a `NativeActivity`; the small Java
//! activity in `java/` adds the system ROM picker, fullscreen, and safe-area
//! insets. Controllers arrive through the patched winit gamepad hook.
//!
//! `touch` and `keymap` are plain logic and build (and test) on any host:
//! `cargo test --manifest-path android/Cargo.toml`.

pub mod keymap;
pub mod orientation;
pub mod touch;

#[cfg(target_os = "android")]
mod app;
#[cfg(target_os = "android")]
mod gamepad;
#[cfg(target_os = "android")]
mod platform;
