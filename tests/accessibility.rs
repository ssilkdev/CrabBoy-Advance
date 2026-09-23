//! ROADMAP M10: accessibility pack.
//!
//! Runs everywhere (no ROMs needed). Covers what the unit tests can't: the
//! slow-motion audio path through a real `AudioOutput` against a simulated
//! real-time device, the colorblind filter on a real emulator frame, sticky
//! buttons driving the keypad, and per-game persistence through the desktop
//! config file format.

use gba_simulator::gba::accessibility::{
    apply_colorblind_filter_to_pixels, AccessibilityManager, AccessibilityStore, ColorblindMode, Handedness,
    SlowMotionAudio,
};
use gba_simulator::gba::apu::audio_output::{set_headless, AudioOutput};
use gba_simulator::gba::keypad::Key;
use gba_simulator::gba::Gba;

const CORE_RATE: usize = 48_000;
/// Core frames of audio per emulated video frame at ~60 fps.
const PER_FRAME: usize = CORE_RATE / 60;

/// `set_headless` is a process-wide switch that `Gba::new_headless` also
/// flips and restores. Tests run in parallel, so without this lock one of
/// them can build its `AudioOutput` in the window where another has switched
/// headless mode back off, and get a real audio device that drains the queue.
static HEADLESS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn headless_gba() -> Gba {
    let _g = HEADLESS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    Gba::new_headless()
}

fn headless_output() -> AudioOutput {
    let _g = HEADLESS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = set_headless(true);
    let out = AudioOutput::new();
    set_headless(prev);
    out.set_fast_forward_mode(1);
    out
}

/// A 440 Hz stereo tone, continuous across calls.
fn tone(frames: usize, phase: &mut usize) -> Vec<f32> {
    let mut v = Vec::with_capacity(frames * 2);
    for _ in 0..frames {
        let s = (*phase as f32 * std::f32::consts::TAU * 440.0 / CORE_RATE as f32).sin() * 0.4;
        v.extend_from_slice(&[s, s]);
        *phase += 1;
    }
    v
}

/// Emulate `speed` × real time for `seconds` against a device that pulls
/// 48 kHz in 10 ms callbacks. Returns (underrun callbacks, played samples)
/// after a half-second warm-up.
fn run_realtime(out: &AudioOutput, speed: f32, seconds: f32) -> (usize, Vec<f32>) {
    run_realtime_from(out, speed, seconds, 50)
}

/// As `run_realtime`, counting from callback `warmup` on. The emulated clock
/// starts one frame ahead, as a front-end that has just run a frame is.
fn run_realtime_from(out: &AudioOutput, speed: f32, seconds: f32, warmup: usize) -> (usize, Vec<f32>) {
    let mut phase = 0;
    let mut emu_time = if warmup == 0 { -1.0 / 60.0 } else { 0.0f32 };
    let callback = CORE_RATE / 100 * 2; // 10 ms of stereo samples
    let mut underruns = 0;
    let mut played = Vec::new();
    let ticks = (seconds * 100.0) as usize;
    for tick in 0..ticks {
        let real = (tick + 1) as f32 / 100.0;
        // The front-end runs as many emulated frames as the paced clock allows.
        while emu_time + 1.0 / 60.0 <= real * speed {
            out.push_sample_batch(&tone(PER_FRAME, &mut phase));
            emu_time += 1.0 / 60.0;
        }
        let got = out.drain_queued(callback);
        if tick >= warmup {
            if got.len() < callback {
                underruns += 1;
            }
            played.extend(got);
        }
    }
    (underruns, played)
}

fn zero_crossings_per_sec(stereo: &[f32]) -> f32 {
    let left: Vec<f32> = stereo.iter().step_by(2).copied().collect();
    let n = left.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
    n as f32 / (left.len() as f32 / CORE_RATE as f32)
}

#[test]
fn slow_motion_audio_never_underruns_and_keeps_pitch() {
    for speed in [0.75f32, 0.5, 0.25, 0.1] {
        let out = headless_output();
        out.set_slow_motion(speed, SlowMotionAudio::PitchPreserved);
        let (underruns, played) = run_realtime(&out, speed, 4.0);
        assert_eq!(underruns, 0, "speed {speed}: {underruns} underruns");
        // 440 Hz = 880 zero crossings per second.
        let zc = zero_crossings_per_sec(&played);
        assert!((zc - 880.0).abs() < 880.0 * 0.03, "speed {speed}: pitch moved ({zc} crossings/s)");
    }
}

#[test]
fn without_stretching_slow_motion_would_starve_the_device() {
    // The failure mode this milestone fixes: pacing at 50% while the
    // output thinks it runs at full speed.
    let out = headless_output();
    let (underruns, _) = run_realtime(&out, 0.5, 4.0);
    assert!(underruns > 100, "expected starvation, got {underruns} underruns");
}

#[test]
fn tape_mode_lowers_pitch_and_mute_mode_is_silent_without_gaps() {
    let out = headless_output();
    out.set_slow_motion(0.5, SlowMotionAudio::Tape);
    let (underruns, played) = run_realtime(&out, 0.5, 4.0);
    assert_eq!(underruns, 0);
    let zc = zero_crossings_per_sec(&played);
    assert!((zc - 440.0).abs() < 440.0 * 0.03, "tape at 50% should halve the pitch ({zc})");

    let out = headless_output();
    out.set_slow_motion(0.25, SlowMotionAudio::Mute);
    let (underruns, played) = run_realtime(&out, 0.25, 4.0);
    assert_eq!(underruns, 0);
    assert!(played.iter().all(|&s| s == 0.0));
}

#[test]
fn leaving_slow_motion_returns_to_normal_audio() {
    let out = headless_output();
    out.set_slow_motion(0.5, SlowMotionAudio::PitchPreserved);
    run_realtime(&out, 0.5, 1.0);
    let deep = out.buffer_len();
    out.set_slow_motion(1.0, SlowMotionAudio::PitchPreserved);
    let (underruns, played) = run_realtime(&out, 1.0, 3.0);
    assert_eq!(underruns, 0);
    // The extra slow-motion buffering is gone again (normal latency).
    assert!(out.buffer_len() <= 3000 + 1600, "queue still {} deep (was {deep})", out.buffer_len());
    let zc = zero_crossings_per_sec(&played);
    assert!((zc - 880.0).abs() < 880.0 * 0.03, "{zc}");
}

#[test]
fn switching_speed_mid_play_does_not_glitch() {
    // Play at full speed, then step through every slow-motion preset and
    // back, as a player flipping the menu would. After the initial warm-up
    // no callback may come up short at any switch.
    let out = headless_output();
    run_realtime(&out, 1.0, 1.0);
    for (speed, mode) in [
        (0.75, SlowMotionAudio::PitchPreserved),
        (0.25, SlowMotionAudio::PitchPreserved),
        (0.5, SlowMotionAudio::Tape),
        (0.1, SlowMotionAudio::PitchPreserved),
        (1.0, SlowMotionAudio::PitchPreserved),
    ] {
        out.set_slow_motion(speed, mode);
        let (underruns, _) = run_realtime_from(&out, speed, 2.0, 0);
        assert_eq!(underruns, 0, "switch to {speed} ({mode:?})");
    }
}

#[test]
fn colorblind_filter_on_a_real_frame_keeps_neutral_pixels_and_alpha() {
    let mut gba = headless_gba();
    // A tiny program-less cart: the backdrop color shows on every pixel.
    gba.load_rom_bytes(vec![0; 0x200]);
    gba.mmu.write16(0x0500_0000, 0x001F); // backdrop = pure red
    gba.run_frame();
    let frame = *gba.get_framebuffer();
    for mode in [ColorblindMode::Protanopia, ColorblindMode::Deuteranopia, ColorblindMode::Tritanopia] {
        let mut f = frame;
        apply_colorblind_filter_to_pixels(&mut f, mode, 1.0);
        assert_eq!(f[0] & 0xFF00_0000, frame[0] & 0xFF00_0000, "alpha kept");
        if frame[0] & 0xFFFFFF != 0 && frame[0] & 0xFF != (frame[0] >> 8) & 0xFF {
            assert_ne!(f[0], frame[0], "{mode:?} should change a saturated color");
        }
    }
    // Grayscale gives equal channels everywhere.
    let mut f = frame;
    apply_colorblind_filter_to_pixels(&mut f, ColorblindMode::Achromatopsia, 1.0);
    for px in f.iter().step_by(97) {
        let (r, g, b) = (px & 0xFF, (px >> 8) & 0xFF, (px >> 16) & 0xFF);
        assert!(r == g && g == b, "{px:08X}");
    }
}

#[test]
fn sticky_buttons_drive_the_keypad() {
    let mut gba = headless_gba();
    gba.load_rom_bytes(vec![0; 0x200]);
    let mut acc = AccessibilityManager::new();
    acc.load_for_game("BPEE", "");
    acc.active.sticky_buttons.set_key_toggle_enabled(Key::B, true);

    let feed = |acc: &mut AccessibilityManager, gba: &mut Gba, physical: bool| {
        let p = acc.process_key(Key::B, physical);
        gba.mmu.keypad.set_key_state(Key::B, p);
        gba.run_frame();
        gba.mmu.keypad.is_key_pressed(Key::B)
    };
    assert!(feed(&mut acc, &mut gba, true), "tap latches B");
    assert!(feed(&mut acc, &mut gba, false), "still held after release");
    for _ in 0..30 {
        assert!(feed(&mut acc, &mut gba, false), "held for as long as needed");
    }
    assert!(!feed(&mut acc, &mut gba, true), "second tap releases");
    assert!(!feed(&mut acc, &mut gba, false));
}

#[test]
fn settings_persist_per_game_through_the_config_file() {
    use gba_simulator::ui::config::AppConfig;

    let mut acc = AccessibilityManager::with_store(AccessibilityStore::default());
    acc.load_for_game("BPEE", "POKEMON EMER");
    acc.active.colorblind_mode = ColorblindMode::Deuteranopia;
    acc.active.colorblind_intensity = 0.8;
    acc.active.slow_motion.enabled = true;
    acc.active.slow_motion.speed_factor = 0.25;
    acc.active.slow_motion.audio = SlowMotionAudio::Tape;
    acc.active.sticky_buttons.set_key_toggle_enabled(Key::B, true);
    acc.active.one_handed_desktop = Handedness::LeftHand;
    acc.active.one_handed_touch = Handedness::RightHand;
    acc.active.ui_scale = 1.5;
    assert!(acc.commit());

    let cfg = AppConfig { accessibility: acc.store.clone(), ..Default::default() };
    let json = serde_json::to_string_pretty(&cfg).unwrap();
    let back: AppConfig = serde_json::from_str(&json).unwrap();

    let mut acc2 = AccessibilityManager::with_store(back.accessibility);
    acc2.load_for_game("AMKE", "");
    assert_eq!(acc2.active.colorblind_mode, ColorblindMode::None, "other games unaffected");
    acc2.load_for_game("BPEE", "");
    let p = &acc2.active;
    assert_eq!(p.colorblind_mode, ColorblindMode::Deuteranopia);
    assert!((p.colorblind_intensity - 0.8).abs() < 1e-6);
    assert!(p.slow_motion.enabled && (p.slow_motion.speed_factor - 0.25).abs() < 1e-6);
    assert_eq!(p.slow_motion.audio, SlowMotionAudio::Tape);
    assert!(p.sticky_buttons.is_key_toggle_enabled(Key::B));
    assert_eq!(p.one_handed_desktop, Handedness::LeftHand);
    assert_eq!(p.one_handed_touch, Handedness::RightHand);
    assert!((p.ui_scale - 1.5).abs() < 1e-6);

    // Android uses the same store serialized on its own.
    let android = AccessibilityStore::from_json(&acc.store.to_json());
    assert_eq!(android, acc.store);

    // Configs written before M10 have no `accessibility` key and still load.
    let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
    v.as_object_mut().unwrap().remove("accessibility");
    let old: AppConfig = serde_json::from_value(v).unwrap();
    assert_eq!(old.accessibility, AccessibilityStore::default());
}
