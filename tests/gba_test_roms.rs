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

/// Row range (screen lines) where jsmolka's `m_test_eval` prints its
/// verdict: text at y=76, 8 pixels tall.
const VERDICT_ROWS: std::ops::Range<usize> = 76..84;

/// Leftmost lit pixel column in the verdict rows, if any.
///
/// "All tests passed" is drawn from x=56 and "Failed test NNN" from x=60;
/// glyphs have a 1-pixel left margin, so the first lit columns are 57 and
/// 61. That tells the two apart without any font knowledge.
fn verdict_left_edge(gba: &Gba) -> Option<usize> {
    let fb = gba.get_framebuffer();
    let bg = fb[0];
    (0..240).find(|&x| VERDICT_ROWS.clone().any(|y| fb[y * 240 + x] != bg))
}

/// The verdict of a jsmolka ROM: `Ok(())` or `Err(reason)`.
///
/// Every jsmolka ROM ends in `idle: b idle` after drawing its verdict, so
/// the run is complete once the PC stops moving between frames. The verdict
/// is read from the screen, not from r12: a test that leaves the CPU in FIQ
/// mode has a banked r12, and reading the wrong bank once hid a real failure.
/// On failure, `m_test_eval` also stores the failing test's digits at
/// 0x03000000/4/8, which gives the test number.
fn run_jsmolka(path: &Path) -> Result<(), String> {
    // Copy to a temp dir so `save/*` ROMs don't leave .sav files beside the
    // user's checkout (the cartridge writes `<rom>.sav` next to the ROM).
    let tmp = std::env::temp_dir().join(format!(
        "crabboy-gba-tests-{}-{}",
        std::process::id(),
        path.file_stem().unwrap().to_string_lossy()
    ));
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let rom = tmp.join(path.file_name().unwrap());
    std::fs::copy(path, &rom).map_err(|e| e.to_string())?;

    let mut gba = Gba::new();
    gba.load_rom(&rom).map_err(|e| format!("load failed: {e}"))?;

    let mut last_pc = u32::MAX;
    let mut stable_frames = 0;
    let mut verdict = Err("did not finish".to_string());
    for _ in 0..600 {
        gba.run_frame();
        let pc = gba.cpu.regs[15];
        stable_frames = if pc == last_pc { stable_frames + 1 } else { 0 };
        last_pc = pc;
        // Wait for the idle loop *and* a drawn verdict: the flash tests
        // spin on the chip's busy status for a while mid-run.
        if stable_frames >= 3 && verdict_left_edge(&gba).is_some() {
            verdict = match verdict_left_edge(&gba) {
                Some(57) => Ok(()),
                Some(61) => {
                    let digit = |a: u32| gba.mmu.read32(0x0300_0000 + a);
                    Err(format!("failed test #{}", digit(0) * 100 + digit(4) * 10 + digit(8)))
                }
                other => Err(format!("no verdict on screen (left edge {other:?}, pc={pc:#010x})")),
            };
            break;
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    verdict
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

/// The mGBA suite's menu, in order.
const MGBA_SUITES: &[&str] = &[
    "Memory tests",
    "I/O read tests",
    "Timing tests",
    "Timer count-up tests",
    "Timer IRQ tests",
    "Shifter tests",
    "Carry tests",
    "Multiply long tests",
    "BIOS math tests",
    "DMA tests",
    "SIO register R/W tests",
    "SIO timing tests",
    "Misc. edge case tests",
    // "Video tests" (last menu entry) is interactive: each test is viewed
    // by hand and has no pass/fail count, so it isn't run here.
];

/// Baseline pass counts. Update when a fix raises one; the test fails if
/// any suite drops below its baseline (a regression) or rises above it (so
/// the baseline gets bumped and the progress is recorded).
const MGBA_BASELINE: &[u32] = &[
    1081, // Memory            /1552
    124,  // I/O read          /130
    482,  // Timing            /2020
    345,  // Timer count-up    /936
    0,    // Timer IRQ         /90
    140,  // Shifter           /140
    93,   // Carry             /93
    52,   // Multiply long     /72
    609,  // BIOS math         /615
    1032, // DMA               /1244
    25,   // SIO register R/W  /90
    0,    // SIO timing        /4
    1,    // Misc. edge case   /12
];

fn press(gba: &mut Gba, key: gba_simulator::gba::keypad::Key) {
    gba.mmu.keypad.set_key_state(key, true);
    for _ in 0..4 {
        gba.run_frame();
    }
    gba.mmu.keypad.set_key_state(key, false);
    for _ in 0..8 {
        gba.run_frame();
    }
}

/// Run one mGBA sub-suite and return (passed, total) from its
/// `END: passed/total` debug message.
fn run_mgba_suite(path: &Path, index: usize) -> Option<(u32, u32)> {
    use gba_simulator::gba::keypad::Key;
    let tmp = std::env::temp_dir().join(format!("crabboy-mgba-suite-{}-{index}", std::process::id()));
    std::fs::create_dir_all(&tmp).ok()?;
    let rom = tmp.join("suite.gba");
    std::fs::copy(path, &rom).ok()?;
    let mut gba = Gba::new();
    gba.load_rom(&rom).ok()?;
    for _ in 0..30 {
        gba.run_frame();
    }
    for _ in 0..index {
        press(&mut gba, Key::Down);
    }
    press(&mut gba, Key::A);
    // Suites run to completion inside `run()` before drawing results; the
    // slowest (timing) takes a few seconds of emulated time.
    for _ in 0..(60 * 60) {
        gba.run_frame();
        if let Some(end) = gba.mmu.debug_port.messages.iter().find_map(|(_, m)| m.strip_prefix("END: ")) {
            let (p, t) = end.trim().split_once('/')?;
            let _ = std::fs::remove_dir_all(&tmp);
            return Some((p.parse().ok()?, t.parse().ok()?));
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    None
}

#[test]
fn mgba_suite() {
    let Some(dir) = rom_dir() else {
        eprintln!("skipping: no GBA test ROM dir (set GBA_TEST_ROM_DIR)");
        return;
    };
    let path = dir.join("suite.gba");
    if !path.is_file() {
        eprintln!("skipping: suite.gba not found");
        return;
    }
    // Run the sub-suites in parallel; each is an independent emulator.
    let results: Vec<Option<(u32, u32)>> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..MGBA_SUITES.len())
            .map(|i| {
                let path = &path;
                s.spawn(move || run_mgba_suite(path, i))
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    let (mut passed, mut total) = (0, 0);
    let mut problems = Vec::new();
    for (i, result) in results.iter().enumerate() {
        let name = MGBA_SUITES[i];
        match *result {
            Some((p, t)) => {
                passed += p;
                total += t;
                eprintln!("  {p:>4}/{t:<4} {name}");
                if p < MGBA_BASELINE[i] {
                    problems.push(format!("{name}: {p}/{t}, baseline {}", MGBA_BASELINE[i]));
                } else if p > MGBA_BASELINE[i] {
                    problems.push(format!("{name}: now {p}/{t}; raise MGBA_BASELINE[{i}] from {}", MGBA_BASELINE[i]));
                }
            }
            None => {
                eprintln!("     ?/?    {name} (did not report)");
                problems.push(format!("{name}: did not report a result"));
            }
        }
    }
    eprintln!("mGBA suite: {passed}/{total} passing");
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
