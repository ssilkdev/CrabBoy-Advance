//! ROADMAP M3: run-ahead.
//!
//! "Done when: measured input-to-screen latency drops by the configured
//! number of frames with no audio artifacts." Both parts are measured here
//! on the self-contained replay program (always runs, including CI) and on
//! Pokemon Emerald when that ROM is available.

use gba_simulator::gba::keypad::Key;
use gba_simulator::gba::replay::{boot, fnv, movie, parse_movie};
use gba_simulator::gba::run_ahead::RunAhead;
use gba_simulator::gba::Gba;
use std::path::PathBuf;

const KEYS: [Key; 10] = [
    Key::A, Key::B, Key::Select, Key::Start, Key::Right,
    Key::Left, Key::Up, Key::Down, Key::R, Key::L,
];

fn frame_hash(g: &Gba) -> u64 {
    fnv(g.get_framebuffer().iter().flat_map(|p| p.to_le_bytes()))
}

/// Play `inputs` with run-ahead; return displayed-frame hashes, the audio
/// that reached the output (the capture buffer) and the final state.
fn play(mut g: Gba, ra: &mut RunAhead, inputs: &[[bool; 10]]) -> (Vec<u64>, Vec<f32>, Vec<u8>) {
    g.mmu.apu.capture = Some(Vec::new());
    let mut shown = Vec::new();
    for keys in inputs {
        for (i, k) in KEYS.iter().enumerate() {
            g.mmu.keypad.set_key_state(*k, keys[i]);
        }
        ra.run_frame(&mut g);
        shown.push(frame_hash(&g));
    }
    let audio = g.mmu.apu.capture.take().unwrap();
    (shown, audio, g.save_state())
}

fn modes() -> Vec<(&'static str, RunAhead)> {
    vec![
        ("1 frame, single instance", RunAhead::new(1, false)),
        ("2 frames, single instance", RunAhead::new(2, false)),
        ("2 frames, second instance", RunAhead::new(2, true)),
    ]
}

#[test]
fn run_ahead_keeps_audio_and_timeline_bit_identical() {
    let mv = parse_movie(&movie());
    let (_, base_audio, base_state) = play(boot(), &mut RunAhead::new(0, false), &mv);
    assert!(!base_audio.is_empty());
    for (name, mut ra) in modes() {
        let (_, audio, state) = play(boot(), &mut ra, &mv);
        assert!(ra.fallback_reason.is_none(), "{name}: {:?}", ra.fallback_reason);
        assert_eq!(audio.len(), base_audio.len(), "{name}: audio length differs");
        if let Some(i) = audio.iter().zip(&base_audio).position(|(a, b)| a.to_bits() != b.to_bits()) {
            panic!("{name}: audio differs at sample {i}");
        }
        assert!(state == base_state, "{name}: the real timeline diverged from a run without run-ahead");
    }
}

/// Frames from the first frame the key is held until the displayed picture
/// differs from a run where it never is.
fn latency(make: impl Fn() -> Gba, frames_ahead: u32, second: bool, key: usize, press_at: usize, total: usize) -> Option<usize> {
    let idle = vec![[false; 10]; total];
    let mut pressed = idle.clone();
    for f in &mut pressed[press_at..] {
        f[key] = true;
    }
    let (a, _, _) = play(make(), &mut RunAhead::new(frames_ahead, second), &idle);
    let (b, _, _) = play(make(), &mut RunAhead::new(frames_ahead, second), &pressed);
    (press_at..total).find(|&t| a[t] != b[t]).map(|t| t - press_at)
}

#[test]
fn run_ahead_reduces_latency_by_configured_frames_homebrew() {
    // Right (index 4) moves the pixel; press at frame 100.
    let base = latency(boot, 0, false, 4, 100, 130).expect("program never reacted");
    eprintln!("homebrew: native latency {base} frame(s)");
    assert!(base >= 1, "expected at least one frame of built-in lag");
    for n in 1..=base as u32 {
        for second in [false, true] {
            let l = latency(boot, n, second, 4, 100, 130).unwrap();
            eprintln!("  run-ahead {n} (second instance: {second}): {l}");
            assert_eq!(l, base - n as usize, "run-ahead {n} should cut exactly {n} frame(s)");
        }
    }
}

fn emerald() -> Option<PathBuf> {
    let p = std::env::var("CRABBOY_DETERMINISM_ROM").map(PathBuf::from).unwrap_or_else(|_| {
        PathBuf::from(std::env::var("HOME").unwrap_or_default())
            .join("Downloads/Pokemon - Emerald Version (USA, Europe).gba")
    });
    p.is_file().then_some(p)
}

#[test]
fn run_ahead_reduces_latency_by_configured_frames_emerald() {
    let Some(rom) = emerald() else {
        eprintln!("skipping: Emerald ROM not found");
        return;
    };
    let rom = std::fs::read(rom).unwrap();
    // Boot to the title screen, then measure how long Start takes to
    // change the picture.
    let make = || {
        let mut g = Gba::new_headless();
        g.load_rom_bytes(rom.clone());
        g.set_deterministic_clock(Some(1_788_000_000));
        g
    };
    let (press, total) = (1000, 1060);
    let base = latency(make, 0, false, 3, press, total).expect("Emerald never reacted to Start");
    eprintln!("Emerald title screen: native latency {base} frame(s)");
    for n in 1..=(base as u32).min(2) {
        let l = latency(make, n, true, 3, press, total).unwrap();
        eprintln!("  run-ahead {n}: {l}");
        assert_eq!(l, base - n as usize);
    }
}
