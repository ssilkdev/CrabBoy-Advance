//! GBA hardware-validation harness (ROADMAP M1).
//!
//! Runs public GBA test ROMs headlessly and reports which ones pass, so the
//! accuracy pass rate can be tracked over time.
//!
//! Suites:
//! - **jsmolka/gba-tests** (`arm`, `thumb`, `memory`, `bios`, `nes`,
//!   `unsafe`, `save/*`): each ROM leaves the number of its first failing
//!   test in `r12` when it reaches its idle loop, or 0 if all passed.
//!   <https://github.com/jsmolka/gba-tests>
//! - **mGBA test suite** (`suite.gba`): an interactive ROM with hundreds of
//!   sub-tests. It has no machine-readable verdict, so the harness only checks
//!   that it boots and runs without the CPU going off the rails. Detailed
//!   scoring is future work.
//!
//! ROMs are not vendored. Point `GBA_TEST_ROM_DIR` at a directory containing
//! a `gba-tests/` checkout and/or `suite.gba`; by default
//! `~/Downloads/gba-test-roms` is used. Missing ROMs are skipped.
//!
//! `cargo test --test gba_test_roms -- --nocapture` prints a scoreboard.

use gba_simulator::gba::Gba;
use std::path::{Path, PathBuf};

fn rom_dir() -> Option<PathBuf> {
    let dir = std::env::var("GBA_TEST_ROM_DIR").map(PathBuf::from).unwrap_or_else(|_| {
        PathBuf::from(std::env::var("HOME").unwrap_or_default())
            .join("Downloads")
            .join("gba-test-roms")
    });
    dir.is_dir().then_some(dir)
}

/// The verdict of a jsmolka ROM: `Ok(())` or `Err(first_failing_test)`.
///
/// Every jsmolka ROM ends in `idle: b idle` after writing its result, so the
/// run is complete once the PC stops moving between frames.
fn run_jsmolka(path: &Path) -> Result<(), String> {
    // Copy to a temp dir so `save/*` ROMs don't leave .sav files beside the
    // user's checkout (the cartridge writes `<rom>.sav` next to the ROM).
    let tmp = std::env::temp_dir().join(format!("crabboy-gba-tests-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let rom = tmp.join(path.file_name().unwrap());
    std::fs::copy(path, &rom).map_err(|e| e.to_string())?;

    let mut gba = Gba::new();
    gba.load_rom(&rom).map_err(|e| format!("load failed: {e}"))?;

    let mut last_pc = u32::MAX;
    let mut stable_frames = 0;
    for _ in 0..600 {
        gba.run_frame();
        let pc = gba.cpu.regs[15];
        if pc == last_pc {
            stable_frames += 1;
            if stable_frames >= 3 {
                let r12 = gba.cpu.regs[12];
                let _ = std::fs::remove_file(rom.with_extension("sav"));
                return if r12 == 0 { Ok(()) } else { Err(format!("failed test #{r12}")) };
            }
        } else {
            stable_frames = 0;
        }
        last_pc = pc;
    }
    Err(format!("did not finish (pc={:#010x}, r12={})", gba.cpu.regs[15], gba.cpu.regs[12]))
}

/// The jsmolka ROMs, and whether CrabBoy is currently expected to pass each.
///
/// When a fix makes an `expected: false` ROM pass, the test fails with a
/// "now passes" message so the table (and the pass rate) gets updated.
const JSMOLKA: &[(&str, bool)] = &[
    ("gba-tests/arm/arm.gba", true),
    ("gba-tests/thumb/thumb.gba", true),
    ("gba-tests/memory/memory.gba", true),
    ("gba-tests/bios/bios.gba", true),
    ("gba-tests/nes/nes.gba", true),
    ("gba-tests/unsafe/unsafe.gba", true),
    ("gba-tests/save/none.gba", true),
    ("gba-tests/save/sram.gba", true),
    ("gba-tests/save/flash64.gba", true),
    ("gba-tests/save/flash128.gba", true),
];

#[test]
fn jsmolka_gba_tests() {
    let Some(dir) = rom_dir() else {
        eprintln!("skipping: no GBA test ROM dir (set GBA_TEST_ROM_DIR)");
        return;
    };
    let mut ran = 0;
    let mut passed = 0;
    let mut regressions = Vec::new();
    let mut newly_passing = Vec::new();
    for &(name, expected) in JSMOLKA {
        let path = dir.join(name);
        if !path.is_file() {
            eprintln!("  skip  {name} (not found)");
            continue;
        }
        ran += 1;
        let result = run_jsmolka(&path);
        match (&result, expected) {
            (Ok(()), _) => {
                passed += 1;
                eprintln!("  PASS  {name}");
                if !expected {
                    newly_passing.push(name);
                }
            }
            (Err(why), true) => {
                eprintln!("  FAIL  {name}: {why}");
                regressions.push(format!("{name}: {why}"));
            }
            (Err(why), false) => eprintln!("  fail  {name}: {why} (known)"),
        }
    }
    if ran > 0 {
        eprintln!("jsmolka/gba-tests: {passed}/{ran} passing");
    }
    assert!(regressions.is_empty(), "regressions:\n{}", regressions.join("\n"));
    assert!(
        newly_passing.is_empty(),
        "these now pass; set `expected: true` in JSMOLKA: {newly_passing:?}"
    );
}

#[test]
fn mgba_suite_boots_and_runs() {
    let Some(dir) = rom_dir() else {
        eprintln!("skipping: no GBA test ROM dir (set GBA_TEST_ROM_DIR)");
        return;
    };
    let path = dir.join("suite.gba");
    if !path.is_file() {
        eprintln!("skipping: suite.gba not found");
        return;
    }
    let mut gba = Gba::new();
    gba.load_rom(&path).expect("load suite.gba");
    for _ in 0..300 {
        gba.run_frame();
    }
    let pc = gba.cpu.regs[15];
    // The suite runs from ROM (0x08..) or IWRAM/EWRAM; anything else means
    // the CPU jumped into unmapped memory.
    let in_code = (0x0800_0000..0x0E00_0000).contains(&pc)
        || (0x0200_0000..0x0204_0000).contains(&pc)
        || (0x0300_0000..0x0300_8000).contains(&pc)
        || pc < 0x4000;
    assert!(in_code, "mGBA suite lost control: pc={pc:#010x}");
}
