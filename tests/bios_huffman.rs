//! Monster Rancher Advance 2 unpacks its menu backgrounds with the BIOS
//! Huffman decompressor (SWI 0x13). The HLE BIOS didn't implement it, so
//! the tiles stayed empty and the Name Entry screen showed a garbage strip
//! over black. Skips when the ROM isn't in ~/Downloads.

use gba_simulator::gba::keypad::Key;
use gba_simulator::gba::Gba;

#[test]
fn name_entry_background_is_decompressed() {
    let home = std::env::var("HOME").unwrap_or_default();
    let rom = std::path::Path::new(&home).join("Downloads/Monster Rancher Advanced 2 (U).gba");
    if !rom.exists() {
        eprintln!("skipping: Monster Rancher Advance 2 ROM not found");
        return;
    }
    let mut g = Gba::new_headless();
    g.load_rom(&rom).unwrap();
    g.set_deterministic_clock(Some(1_700_000_000));
    // Title -> New Game -> three dialogue boxes -> Name Entry.
    let run = |g: &mut Gba, n: u32| (0..n).for_each(|_| g.run_frame());
    run(&mut g, 600);
    for key in [Key::Start, Key::A, Key::A, Key::A, Key::A] {
        g.mmu.keypad.set_key_state(key, true);
        run(&mut g, 6);
        g.mmu.keypad.set_key_state(key, false);
        run(&mut g, 180);
    }

    // BG3 (the backdrop layer) uses tiles 0x80..0x176 from char block 3.
    let ppu = &g.mmu.ppu;
    assert_eq!(ppu.bgcnt[3] & 0xFF0C, 0x1F0C, "expected the Name Entry screen (BG3 char block 3, map 0x1F)");
    let char_base = 0xC000usize;
    let empty = (0x80..0x176usize)
        .filter(|t| ppu.vram[char_base + t * 32..char_base + t * 32 + 32].iter().all(|&b| b == 0))
        .count();
    assert!(empty < 8, "{empty} of 246 backdrop tiles are empty: the Huffman-packed graphics weren't unpacked");

    // And the picture: the backdrop fills the screen, so few pixels are
    // pure black (it was ~80% black before the fix).
    let black = g.get_framebuffer().iter().filter(|&&p| p & 0x00FF_FFFF == 0).count();
    assert!(black < 240 * 160 / 10, "{black} black pixels");
}
