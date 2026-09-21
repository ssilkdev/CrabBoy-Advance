//! How is WIN0H actually being fed during the battle transition?
//!
//! If the game updates it per scanline (HBlank IRQ or HBlank DMA), then the
//! end-of-frame register snapshot is meaningless and what matters is the
//! per-line sequence the PPU sees.

use gba_simulator::gba::keypad::Key;
use gba_simulator::gba::Gba;

fn press(gba: &mut Gba, keys: &[Key], frames: usize) {
    for _ in 0..frames {
        for k in keys {
            gba.mmu.keypad.set_key_state(*k, true);
        }
        gba.run_frame();
        for k in keys {
            gba.mmu.keypad.set_key_state(*k, false);
        }
    }
}

#[test]
#[ignore]
fn win0h_write_frequency_during_transition() {
    let rom = std::env::var("GBA_TEST_ROM").unwrap();
    let mut gba = Gba::new();
    gba.load_rom(&rom).unwrap();

    press(&mut gba, &[], 240);
    for _ in 0..12 {
        press(&mut gba, &[Key::Start], 6);
        press(&mut gba, &[], 24);
    }
    for _ in 0..10 {
        press(&mut gba, &[Key::A], 6);
        press(&mut gba, &[], 20);
    }

    // Walk to just before the transition.
    for step in 0..52 {
        let dir = if (step / 2) % 2 == 0 { Key::Up } else { Key::Down };
        press(&mut gba, &[dir], 16);
    }

    // Now step frame by frame, counting WIN0H / DMA activity.
    for f in 0..40 {
        gba.mmu.flight_recorder.clear();
        gba.run_frame();
        let ev = gba.mmu.flight_recorder.recent_events(512);

        let win0h: Vec<&_> = ev.iter().filter(|e| e.reg_name == "WIN0H").collect();
        let dma_cnt = ev
            .iter()
            .filter(|e| e.reg_name.starts_with("DMA") && e.reg_name.ends_with("CNT_H"))
            .count();

        let vals: Vec<u32> = win0h.iter().take(8).map(|e| e.val).collect();
        let distinct: std::collections::BTreeSet<u32> =
            win0h.iter().map(|e| e.val).collect();

        println!(
            "frame {:2}: WIN0H writes={:3} distinct={:3} dma_cnt_writes={:3} first_vals={:?}",
            f,
            win0h.len(),
            distinct.len(),
            dma_cnt,
            vals
        );
    }
}
