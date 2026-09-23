//! ROADMAP M2: determinism, fast save states and movie replay.
//!
//! These tests need a commercial ROM, so they look for Pokemon Emerald at
//! `$CRABBOY_DETERMINISM_ROM` or `~/Downloads/Pokemon - Emerald Version
//! (USA, Europe).gba` and skip when it isn't there. The homebrew-ROM variant
//! (`replay_generated_rom_*`) always runs, including in CI.

use gba_simulator::gba::keypad::Key;
use gba_simulator::gba::Gba;
use std::path::PathBuf;

/// FNV-1a, 64-bit: tiny, stable and good enough to fingerprint frames.
fn fnv(bytes: impl IntoIterator<Item = u8>) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

fn frame_hash(gba: &Gba) -> u64 {
    fnv(gba.get_framebuffer().iter().flat_map(|p| p.to_le_bytes()))
}

fn audio_hash(samples: &[f32]) -> u64 {
    fnv(samples.iter().flat_map(|s| s.to_bits().to_le_bytes()))
}

const KEYS: [Key; 10] = [
    Key::A, Key::B, Key::Select, Key::Start, Key::Right,
    Key::Left, Key::Up, Key::Down, Key::R, Key::L,
];

/// A fixed, repeatable input script: mash through the intro.
fn scripted_input(frame: u64) -> u16 {
    match frame % 90 {
        0..=3 => 1 << 3,  // Start
        30..=33 => 1,     // A
        60..=62 => 1 << 7, // Down
        _ => 0,
    }
}

fn apply_input(gba: &mut Gba, mask: u16) {
    for (i, k) in KEYS.iter().enumerate() {
        gba.mmu.keypad.set_key_state(*k, mask & (1 << i) != 0);
    }
}

/// Run `frames` frames with the scripted input and return per-frame video
/// hashes plus one hash of all audio produced.
fn run(gba: &mut Gba, start: u64, frames: u64) -> (Vec<u64>, u64) {
    let mut video = Vec::with_capacity(frames as usize);
    let mut audio = Vec::new();
    for f in start..start + frames {
        apply_input(gba, scripted_input(f));
        gba.run_frame();
        gba.mmu.apu.flush_samples();
        audio.append(&mut gba.mmu.apu.pending_diagnostic_samples);
        video.push(frame_hash(gba));
    }
    (video, audio_hash(&audio))
}

fn emerald() -> Option<PathBuf> {
    let p = std::env::var("CRABBOY_DETERMINISM_ROM").map(PathBuf::from).unwrap_or_else(|_| {
        PathBuf::from(std::env::var("HOME").unwrap_or_default())
            .join("Downloads/Pokemon - Emerald Version (USA, Europe).gba")
    });
    p.is_file().then_some(p)
}

/// Load a ROM copy in a private temp dir (so runs don't share a .sav) and
/// pin the RTC so both runs see the same date.
fn boot(rom: &PathBuf, tag: &str) -> (Gba, PathBuf) {
    let dir = std::env::temp_dir().join(format!("crabboy-det-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let copy = dir.join("rom.gba");
    std::fs::copy(rom, &copy).unwrap();
    let mut gba = Gba::new();
    gba.load_rom(&copy).unwrap();
    gba.set_deterministic_clock(Some(1_788_000_000)); // fixed Unix time
    (gba, dir)
}

#[test]
fn two_runs_produce_identical_frames_and_audio() {
    let Some(rom) = emerald() else {
        eprintln!("skipping: Emerald ROM not found");
        return;
    };
    let (mut a, da) = boot(&rom, "a");
    let (mut b, db) = boot(&rom, "b");
    let (va, aa) = run(&mut a, 0, 1200);
    let (vb, ab) = run(&mut b, 0, 1200);
    if let Some(i) = va.iter().zip(&vb).position(|(x, y)| x != y) {
        panic!("frames diverge at frame {i}");
    }
    assert_eq!(aa, ab, "audio diverges");
    let _ = std::fs::remove_dir_all(da);
    let _ = std::fs::remove_dir_all(db);
}

#[test]
fn save_state_restore_replays_identically() {
    let Some(rom) = emerald() else {
        eprintln!("skipping: Emerald ROM not found");
        return;
    };
    let (mut gba, dir) = boot(&rom, "state");
    run(&mut gba, 0, 600);
    let state = gba.save_state();
    let (v1, a1) = run(&mut gba, 600, 600);
    assert!(gba.load_state(&state), "load_state failed");
    let (v2, a2) = run(&mut gba, 600, 600);
    if let Some(i) = v1.iter().zip(&v2).position(|(x, y)| x != y) {
        panic!("replay after load_state diverges at frame {}", 600 + i);
    }
    assert_eq!(a1, a2, "audio after load_state diverges");
    // Round trip: saving right after a load gives the same bytes.
    assert!(gba.load_state(&state));
    assert_eq!(gba.save_state(), state, "save_state is not byte-stable across a load");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn save_state_fits_in_a_frame_budget() {
    let Some(rom) = emerald() else {
        eprintln!("skipping: Emerald ROM not found");
        return;
    };
    let (mut gba, dir) = boot(&rom, "speed");
    run(&mut gba, 0, 300);
    let n = 200;
    let t = std::time::Instant::now();
    for _ in 0..n {
        let s = gba.save_state();
        assert!(gba.load_state(&s));
    }
    let per = t.elapsed() / n;
    let size = gba.save_state().len();
    eprintln!("save+load: {per:?} per round trip, {} KiB", size / 1024);
    // One 60 Hz frame is 16.7 ms; a save+restore must use well under it
    // (release build). Debug builds are ~10x slower, so only enforce this
    // when optimised.
    if !cfg!(debug_assertions) {
        assert!(per.as_micros() < 2000, "save+load took {per:?}");
    }
    let _ = std::fs::remove_dir_all(dir);
}
