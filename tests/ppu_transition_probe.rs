//! Capture PPU register state across the battle transition so we can see
//! exactly which display setting is producing the broken frames.

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

fn describe(gba: &Gba) -> String {
    let p = &gba.mmu.ppu;
    let mode = p.dispcnt & 7;
    let forced_blank = (p.dispcnt >> 7) & 1;
    let bgs: Vec<usize> = (0..4).filter(|i| (p.dispcnt >> (8 + i)) & 1 == 1).collect();
    let obj = (p.dispcnt >> 12) & 1;
    let win0 = (p.dispcnt >> 13) & 1;
    let win1 = (p.dispcnt >> 14) & 1;
    let objwin = (p.dispcnt >> 15) & 1;
    format!(
        "dispcnt=0x{:04X} mode={} forced_blank={} bgs={:?} obj={} win0={} win1={} objwin={} \
         win0h=0x{:04X} win0v=0x{:04X} winin=0x{:04X} winout=0x{:04X} \
         bldcnt=0x{:04X} bldy=0x{:04X} mosaic=0x{:04X}",
        p.dispcnt, mode, forced_blank, bgs, obj, win0, win1, objwin,
        p.win0h, p.win0v, p.winin, p.winout, p.bldcnt, p.bldy, p.mosaic
    )
}

fn nonblack(gba: &Gba) -> usize {
    gba.get_framebuffer()
        .iter()
        .filter(|&&p| (p & 0x00FF_FFFF) != 0)
        .count()
}

#[test]
#[ignore]
fn dump_ppu_state_across_battle_transition() {
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

    println!("post-boot: {}", describe(&gba));
    println!();

    for step in 0..80 {
        let dir = if (step / 2) % 2 == 0 { Key::Up } else { Key::Down };
        press(&mut gba, &[dir], 16);
        let nb = nonblack(&gba);
        // Print densely around the transition.
        if (45..80).contains(&step) {
            println!("step {:3}  nonblack={:5}  {}", step, nb, describe(&gba));
        }
    }
}
