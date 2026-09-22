//! Hardware-validation harness: runs real Game Boy test ROMs and asserts on
//! the text they print over the serial link.
//!
//! Blargg's test suites write their results to the link port byte by byte,
//! which is why `GbMmu::serial_out` exists. A passing run ends with "Passed";
//! a failing one prints which sub-test broke, so the assertion message is the
//! actual hardware diagnosis rather than "false != true".
//!
//! ROMs are not vendored (they are copyrighted test binaries). Point
//! `GB_TEST_ROM_DIR` at a directory holding them, otherwise these tests skip.

use gba_simulator::dmg::GameBoy;
use std::path::PathBuf;

fn rom_dir() -> Option<PathBuf> {
    let dir = std::env::var("GB_TEST_ROM_DIR")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            let d = dirs_downloads().join("gb-test-roms");
            d.is_dir().then_some(d)
        })?;
    dir.is_dir().then_some(dir)
}

fn dirs_downloads() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Downloads")
}

/// Run a ROM for at most `frames` frames, returning everything it printed.
/// Stops early once the serial log contains "Passed" or "Failed".
fn run_serial(rom: &PathBuf, frames: u64) -> String {
    let mut gb = GameBoy::from_file(rom, false).expect("load ROM");
    for _ in 0..frames {
        gb.run_frame();
        if gb.mmu.serial_out.len() > 4 {
            let s = String::from_utf8_lossy(&gb.mmu.serial_out);
            if s.contains("Passed") || s.contains("Failed") {
                break;
            }
        }
    }
    String::from_utf8_lossy(&gb.mmu.serial_out).to_string()
}

fn check(name: &str, frames: u64) {
    let Some(dir) = rom_dir() else {
        eprintln!("skipping {name}: no GB test ROM dir");
        return;
    };
    let rom = dir.join(name);
    if !rom.is_file() {
        eprintln!("skipping {name}: not present in {}", dir.display());
        return;
    }
    let out = run_serial(&rom, frames);
    assert!(
        out.contains("Passed"),
        "{name} did not pass.\n--- serial output ---\n{out}\n---------------------"
    );
}

#[test]
fn blargg_cpu_instrs_all_eleven_suites_pass() {
    // The full suite runs all 11 sub-tests; it needs a few thousand frames.
    check("cpu_instrs.gb", 4000);
}

#[test]
fn blargg_instr_timing_passes() {
    check("instr_timing.gb", 1500);
}

#[test]
fn dmg_acid2_renders_without_crashing() {
    // dmg-acid2 is a visual test: no serial output, so this only asserts the
    // core runs it to a stable, non-blank frame. Compare the PNG by eye.
    let Some(dir) = rom_dir() else { return };
    let rom = dir.join("dmg-acid2.gb");
    if !rom.is_file() {
        return;
    }
    let mut gb = GameBoy::from_file(&rom, false).expect("load dmg-acid2");
    for _ in 0..200 {
        gb.run_frame();
    }
    let fb = gb.get_framebuffer();
    let distinct: std::collections::HashSet<u32> = fb.iter().copied().collect();
    assert!(
        distinct.len() >= 2,
        "dmg-acid2 rendered a blank frame ({} distinct colors)",
        distinct.len()
    );
}

#[test]
fn cgb_acid2_runs_in_color_mode() {
    let Some(dir) = rom_dir() else { return };
    let rom = dir.join("cgb-acid2.gbc");
    if !rom.is_file() {
        return;
    }
    let mut gb = GameBoy::from_file(&rom, false).expect("load cgb-acid2");
    assert!(gb.is_cgb(), "cgb-acid2 must select CGB mode from its header");
    for _ in 0..200 {
        gb.run_frame();
    }
    let fb = gb.get_framebuffer();
    let distinct: std::collections::HashSet<u32> = fb.iter().copied().collect();
    assert!(
        distinct.len() >= 3,
        "cgb-acid2 produced too few colors ({})",
        distinct.len()
    );
}
