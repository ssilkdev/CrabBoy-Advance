//! NDS hardware-validation harness: RockPolish/rockwrestler.
//!
//! rockwrestler is an interactive menu of ARM9/ARM7 tests (ARMv4/v5
//! instructions, IPC, DIV/SQRT unit, WRAMCNT/VRAMCNT/TCM). This harness drives
//! the menu with key presses, runs every test, and reads the verdict the ROM
//! draws at the top-left of the top screen: `OK` (2 glyphs, 16 px wide) or
//! `FAIL xxx` / `TIMEOUT xxx` (at least 8 glyphs).
//!
//! The ROM is not vendored (no licence). Point `NDS_TEST_ROM_DIR` at a
//! directory containing `rockwrestler.nds`, or put it in
//! `~/Downloads/nds-test-roms/`. Missing ROM = skipped test.
//! <https://github.com/RockPolish/rockwrestler>
//!
//! `cargo test --release --test nds_test_roms -- --nocapture` prints a scoreboard.

use gba_simulator::nds::{Nds, NdsKey};
use std::path::PathBuf;

fn rom_path() -> Option<PathBuf> {
    let dir = std::env::var("NDS_TEST_ROM_DIR").map(PathBuf::from).unwrap_or_else(|_| {
        PathBuf::from(std::env::var("HOME").unwrap_or_default())
            .join("Downloads")
            .join("nds-test-roms")
    });
    let p = dir.join("rockwrestler.nds");
    p.is_file().then_some(p)
}

fn press(nds: &mut Nds, key: NdsKey) {
    nds.set_nds_key(key, true);
    for _ in 0..4 {
        nds.run_frame();
    }
    nds.set_nds_key(key, false);
    for _ in 0..8 {
        nds.run_frame();
    }
}

fn wait(nds: &mut Nds, frames: u32) {
    for _ in 0..frames {
        nds.run_frame();
    }
}

/// Width in pixels of the verdict text in the top screen's first text row.
fn verdict_width(nds: &Nds) -> usize {
    let fb = nds.get_framebuffer();
    let bg = fb[256 * 100]; // plain background below the text
    (0..256)
        .rev()
        .find(|&x| (0..8).any(|y| fb[y * 256 + x] != bg))
        .map_or(0, |x| x + 1)
}

/// (submenu name, index in main menu, number of tests)
const MENUS: [(&str, usize, usize); 5] =
    [("ARMv4", 0, 1), ("ARMv5", 1, 11), ("IPC", 2, 3), ("DS math", 3, 5), ("Memory", 4, 3)];

const TEST_NAMES: [&str; 23] = [
    "condition codes", "CLZ", "QADD/QSUB", "QDADD/QDSUB", "SMULxy", "SMLAxy", "SMULWy",
    "SMLAWy", "SMLALxy", "BLX", "LDR r15", "LDM/STM", "IPCSYNC", "IPCFIFO", "IPCFIFO IRQ",
    "SQRT32", "SQRT64", "DIV 32/32", "DIV 64/32", "DIV 64/64", "WRAMCNT", "VRAMCNT", "TCM",
];

#[test]
fn rockwrestler_all_tests_pass() {
    let Some(rom) = rom_path() else {
        eprintln!("skipping: rockwrestler.nds not found");
        return;
    };
    let mut nds = Nds::new();
    nds.load_rom(&rom).expect("load rockwrestler");
    wait(&mut nds, 40);

    let mut failures = Vec::new();
    let mut n = 0;
    for (menu, idx, count) in MENUS {
        for _ in 0..idx {
            press(&mut nds, NdsKey::Down);
        }
        press(&mut nds, NdsKey::A);
        wait(&mut nds, 20);
        for _ in 0..count {
            press(&mut nds, NdsKey::A);
            wait(&mut nds, 90);
            let width = verdict_width(&nds);
            let ok = (9..=20).contains(&width);
            println!("{:<8} {:<16} {}", menu, TEST_NAMES[n], if ok { "OK" } else { "FAIL" });
            if !ok {
                failures.push(format!("{menu}/{} (verdict width {width}px)", TEST_NAMES[n]));
            }
            n += 1;
            press(&mut nds, NdsKey::B);
            wait(&mut nds, 20);
            press(&mut nds, NdsKey::Down);
        }
        press(&mut nds, NdsKey::B);
        wait(&mut nds, 20);
        for _ in 0..idx {
            press(&mut nds, NdsKey::Up);
        }
    }
    println!("rockwrestler: {}/{} passed", n - failures.len(), n);
    assert!(failures.is_empty(), "failing tests: {failures:#?}");
}
