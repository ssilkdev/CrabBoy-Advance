//! A truncated or corrupt state file must never crash the emulator (release
//! builds abort on panic) and a failed load must leave the running game
//! untouched.

use gba_simulator::gba::Gba;

fn running_core(fill: u8) -> Gba {
    let mut g = Gba::new_headless();
    g.load_rom_bytes(vec![0u8; 0x1000]);
    g.run_frame();
    g.mmu.ewram[..64].fill(fill);
    g.mmu.iwram[..64].fill(fill);
    g
}

#[test]
fn every_truncation_of_a_state_fails_cleanly() {
    let state = running_core(0x5A).save_state();
    let v2 = state.windows(4).position(|w| w == b"CBA2").expect("v2 marker");

    let mut game = running_core(0x11);
    let untouched = game.save_state();
    // Everything from just before the v2 tail to the end: each section's
    // length checks, including the partial ones.
    for cut in (v2 - 8)..state.len() {
        let ok = game.load_state(&state[..cut]);
        if ok {
            // A state may legitimately end after a complete section; undo it.
            assert!(game.load_state(&untouched), "cut {cut}: could not restore");
        } else {
            assert!(game.save_state() == untouched, "cut {cut}: failed load changed the game");
        }
    }
    assert_eq!(game.mmu.ewram[0], 0x11);
}

#[test]
fn a_failed_load_keeps_the_game_running_as_before() {
    let state = running_core(0x5A).save_state();
    let v3 = state.windows(4).position(|w| w == b"CBA3").expect("v3 marker");
    let mut game = running_core(0x11);
    let mut twin = running_core(0x11);
    assert!(!game.load_state(&state[..v3 + 40]));
    assert_eq!(game.mmu.ewram[0], 0x11, "RAM from the rejected state leaked in");
    // And it keeps emulating exactly like a core that never tried.
    for _ in 0..5 {
        game.run_frame();
        twin.run_frame();
    }
    assert!(game.get_framebuffer()[..] == twin.get_framebuffer()[..]);
    assert!(game.save_state() == twin.save_state());
}

#[test]
fn a_complete_state_still_loads() {
    let src = running_core(0x5A);
    let mut game = running_core(0x11);
    assert!(game.load_state(&src.save_state()));
    assert_eq!(game.mmu.ewram[0], 0x5A);
}
