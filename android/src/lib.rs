//! CrabBoy Advance for Android.
//!
//! A touch-first front-end over the same emulator cores the desktop build
//! uses (`gba_simulator::gba` / `gba_simulator::dmg`). Rendering and touch
//! input go through eframe/egui on a `NativeActivity`; the small Java
//! activity in `java/` adds the system ROM picker, fullscreen, and safe-area
//! insets. Controllers arrive through the patched winit gamepad hook.
//!
//! `touch`, `tilt` and `keymap` are plain logic and build (and test) on any host:
//! `cargo test --manifest-path android/Cargo.toml`.

pub mod autosave_settings;
pub mod keymap;
pub mod orientation;
pub mod skin;
pub mod tilt;
pub mod touch;

#[cfg(target_os = "android")]
mod app;
#[cfg(target_os = "android")]
mod gamepad;
#[cfg(target_os = "android")]
mod platform;

/// The egui_glow patch (see Cargo.toml) must keep asking for highp floats:
/// with mediump, phone GPUs draw a thin line beside letters.
#[cfg(test)]
mod shader_patch_tests {
    #[test]
    fn egui_shaders_use_high_precision_on_gles() {
        for src in [
            include_str!("../patches/egui_glow/src/shader/fragment.glsl"),
            include_str!("../patches/egui_glow/src/shader/vertex.glsl"),
        ] {
            assert!(src.contains("precision highp float;"), "shader lost its highp patch");
        }
    }
}
