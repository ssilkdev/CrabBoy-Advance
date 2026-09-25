//! Stage 4a: Idle-Loop Detection & Busy-Wait Skipping (ROADMAP M4a / docs/JIT.md).
//!
//! Idle-loop skipping must be bit-identical to running every cycle of the loop
//! manually: same pictures, same sound, same CPU state and memory, frame for frame.
//!
//! In games like Pokémon Emerald that never invoke HALT and busy-wait in an idle
//! loop, idle-loop skipping slashes instruction execution counts by 30-50%,
//! providing massive battery savings and preventing thermal throttling on mobile.

use gba_simulator::gba::Gba;
use std::path::PathBuf;

fn roms() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let d = PathBuf::from(home).join("Downloads");
    [
        "Pokemon - Emerald Version (USA, Europe).gba",
        "Dragon Ball - Advanced Adventure (USA).gba",
        "Pokemon_ Sapphire Version/Pokemon - Sapphire Version (USA, Europe) (Rev 2).gba",
        "Harry Potter and the Sorcerer's Stone/Harry Potter and the Sorcerer's Stone (USA, Europe) (En,Fr,De,Es,It,Nl,Pt,Sv,No,Da).gba",
        "gba-test-roms/suite.gba",
    ]
        .iter()
        .map(|n| d.join(n))
        .filter(|p| p.exists())
        .collect()
}

fn fnv(h: u64, bytes: impl IntoIterator<Item = u8>) -> u64 {
    bytes.into_iter().fold(h, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

fn run_with_idle_skip(rom: &PathBuf, idle_skip: bool, frames: u32) -> (Vec<u64>, u64) {
    let mut g = Gba::new_headless();
    g.load_rom(rom).unwrap();
    g.set_deterministic_clock(Some(1_700_000_000));
    g.halt_skip = true;
    g.idle_skip = idle_skip;
    g.batch_peripherals_enabled = true;
    g.mmu.apu.capture = Some(Vec::new());

    let mut out = Vec::new();
    let mut total_instructions = 0u64;

    for f in 0..frames {
        let press = (f / 30) % 4 == 1;
        g.mmu.keypad.set_key_state(gba_simulator::gba::keypad::Key::Start, press && f % 120 < 60);
        g.mmu.keypad.set_key_state(gba_simulator::gba::keypad::Key::A, press && f % 120 >= 60);

        let mut frame_cycles = 0;
        g.batch_peripherals_enabled = true;
        while frame_cycles < gba_simulator::gba::CYCLES_PER_FRAME {
            g.halt_budget = gba_simulator::gba::CYCLES_PER_FRAME - frame_cycles;
            let c = g.step_instruction();
            frame_cycles += c;
            total_instructions += 1;
        }
        g.catch_up_peripherals();

        g.mmu.apu.flush_samples();
        let audio = std::mem::take(g.mmu.apu.capture.as_mut().unwrap());

        let mut h = fnv(0xcbf29ce484222325, g.get_framebuffer().iter().flat_map(|p| p.to_le_bytes()));
        h = fnv(h, audio.iter().flat_map(|s| s.to_bits().to_le_bytes()));
        h = fnv(h, g.cpu.regs.iter().flat_map(|r| r.to_le_bytes()));
        h = fnv(h, g.cpu.cycles.to_le_bytes());
        h = fnv(h, g.live_framebuffer().iter().flat_map(|p| p.to_le_bytes()));
        h = fnv(h, g.mmu.ewram.iter().copied());
        h = fnv(h, g.mmu.iwram.iter().copied());
        out.push(h);
    }
    (out, total_instructions)
}

#[test]
fn idle_skip_is_bit_identical() {
    let roms = roms();
    if roms.is_empty() {
        eprintln!("skipping: no test ROMs in ~/Downloads");
        return;
    }
    for rom in &roms {
        println!("Testing bit-identical execution on {}...", rom.display());
        let (hashes_no_skip, _) = run_with_idle_skip(rom, false, 500);
        let (hashes_with_skip, _) = run_with_idle_skip(rom, true, 500);

        let first_diff = hashes_no_skip.iter().zip(&hashes_with_skip).position(|(x, y)| x != y);
        assert_eq!(
            first_diff,
            None,
            "{}: diverged at frame {:?}",
            rom.display(),
            first_diff
        );
    }
}

#[test]
fn emerald_idle_skip_reduces_cpu_work_in_gameplay() {
    let home = std::env::var("HOME").unwrap_or_default();
    let emerald = PathBuf::from(home).join("Downloads/Pokemon - Emerald Version (USA, Europe).gba");
    if !emerald.exists() {
        eprintln!("skipping: Emerald ROM not found");
        return;
    }

    let mut g = Gba::new_headless();
    g.load_rom(&emerald).unwrap();
    g.set_deterministic_clock(Some(1_700_000_000));
    g.halt_skip = true;
    g.idle_skip = true;
    g.batch_peripherals_enabled = true;

    // Advance 250 frames through boot logos
    for f in 0..250 {
        let press = (f / 30) % 4 == 1;
        g.mmu.keypad.set_key_state(gba_simulator::gba::keypad::Key::Start, press && f % 120 < 60);
        g.mmu.keypad.set_key_state(gba_simulator::gba::keypad::Key::A, press && f % 120 >= 60);
        g.run_frame();
    }

    let mut total_skipped_cycles = 0u64;
    let monitored_frames = 60;
    for _ in 0..monitored_frames {
        let mut frame_cycles = 0;
        while frame_cycles < gba_simulator::gba::CYCLES_PER_FRAME {
            g.halt_budget = gba_simulator::gba::CYCLES_PER_FRAME - frame_cycles;
            let c = g.step_instruction();
            frame_cycles += c;
            if c > 40 {
                total_skipped_cycles += c as u64;
            }
        }
        g.catch_up_peripherals();
    }

    let total_frame_cycles = monitored_frames as u64 * gba_simulator::gba::CYCLES_PER_FRAME as u64;
    let skip_pct = (total_skipped_cycles as f64 / total_frame_cycles as f64) * 100.0;
    println!(
        "Emerald gameplay ({} frames): skipped {} / {} cycles ({:.1}%)",
        monitored_frames, total_skipped_cycles, total_frame_cycles, skip_pct
    );

    assert!(
        skip_pct >= 30.0,
        "Expected at least 30% idle cycles skipped on Emerald, got {:.1}%",
        skip_pct
    );
}
