//! What is the stuck loop actually reading?

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
fn disassemble_stuck_loop() {
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
    for step in 0..52 {
        let dir = if (step / 2) % 2 == 0 { Key::Up } else { Key::Down };
        press(&mut gba, &[dir], 16);
    }
    press(&mut gba, &[], 30);

    println!("raw words at the hot loop:");
    for a in (0x08C0_0000u32..0x08C0_0040).step_by(4) {
        println!("  0x{:08X}: 0x{:08X}", a, gba.mmu.read32(a));
    }

    println!();
    println!("thumb={} cpsr=0x{:08X}", (gba.cpu.cpsr >> 5) & 1, gba.cpu.cpsr);
    println!("regs:");
    for r in 0..16 {
        println!("  r{:<2} = 0x{:08X}", r, gba.cpu.regs[r]);
    }

    println!();
    println!("WIN0H DMA table head at 0x020394E8:");
    for i in 0..12 {
        let a = 0x0203_94E8 + i * 2;
        println!("  [{:2}] 0x{:04X}", i, gba.mmu.read16(a));
    }
}
