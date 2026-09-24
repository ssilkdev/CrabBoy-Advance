//! Halt skipping (a halted CPU jumps to the next hardware event instead of
//! stepping one cycle at a time) and batched peripheral stepping (JIT stage
//! 2) must not change anything: same pictures, same sound, same CPU state
//! and memory, frame for frame.

use gba_simulator::gba::Gba;
use std::path::PathBuf;

fn roms() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let d = PathBuf::from(home).join("Downloads");
    [
        "Dragon Ball - Advanced Adventure (USA).gba",
        "Pokemon - Emerald Version (USA, Europe).gba",
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

/// Per-frame hashes of picture + sound + CPU registers, with scripted input.
fn run(rom: &PathBuf, skip: bool, frames: u32) -> (Vec<u64>, u64) {
    run_with(rom, skip, true, frames)
}

fn run_with(rom: &PathBuf, skip: bool, batch: bool, frames: u32) -> (Vec<u64>, u64) {
    let mut g = Gba::new_headless();
    g.load_rom(rom).unwrap();
    g.set_deterministic_clock(Some(1_700_000_000));
    g.halt_skip = skip;
    g.batch_peripherals_enabled = batch;
    g.mmu.apu.capture = Some(Vec::new());
    let mut out = Vec::new();
    let mut halted_steps = 0u64;
    for f in 0..frames {
        // Mash START/A now and then so menus and gameplay get exercised.
        let press = (f / 30) % 4 == 1;
        g.mmu.keypad.set_key_state(gba_simulator::gba::keypad::Key::Start, press && f % 120 < 60);
        g.mmu.keypad.set_key_state(gba_simulator::gba::keypad::Key::A, press && f % 120 >= 60);
        g.run_frame();
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
        halted_steps += g.cpu.halted as u64;
    }
    (out, halted_steps)
}

#[test]
fn halt_skip_is_bit_identical() {
    let roms = roms();
    if roms.is_empty() {
        eprintln!("skipping: no test ROMs in ~/Downloads");
        return;
    }
    for rom in roms {
        let (a, _) = run(&rom, false, 3000);
        let (b, _) = run(&rom, true, 3000);
        let first_diff = a.iter().zip(&b).position(|(x, y)| x != y);
        assert_eq!(first_diff, None, "{}: diverged at frame {:?}", rom.display(), first_diff);
    }
}

/// JIT stage 2: letting the peripherals run behind the CPU until the next
/// hardware event must not change anything either (docs/JIT.md). Compared
/// with both optimizations off, so this checks them together too.
#[test]
fn batched_peripherals_are_bit_identical() {
    let roms = roms();
    if roms.is_empty() {
        eprintln!("skipping: no test ROMs in ~/Downloads");
        return;
    }
    for rom in roms {
        let (a, _) = run_with(&rom, false, false, 2000);
        let (b, _) = run_with(&rom, true, true, 2000);
        let first_diff = a.iter().zip(&b).position(|(x, y)| x != y);
        assert_eq!(first_diff, None, "{}: diverged at frame {:?}", rom.display(), first_diff);
    }
}
