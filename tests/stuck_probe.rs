//! Is the game stuck in a wait loop during the broken transition?

use gba_simulator::gba::keypad::Key;
use gba_simulator::gba::Gba;
use std::collections::HashMap;

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
fn is_the_cpu_stuck() {
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

    // Let it settle well into the stuck state.
    press(&mut gba, &[], 30);

    // Sample the PC over one frame's worth of instructions.
    let mut hist: HashMap<u32, u32> = HashMap::new();
    for _ in 0..200_000 {
        gba.step_instruction();
        *hist.entry(gba.cpu.regs[15]).or_insert(0) += 1;
    }

    let mut top: Vec<(u32, u32)> = hist.into_iter().collect();
    top.sort_by_key(|&(_, c)| std::cmp::Reverse(c));

    println!("distinct PCs in 200k instructions: {}", top.len());
    println!("hottest PCs:");
    for (pc, c) in top.iter().take(15) {
        println!("  0x{:08X}  {:6} ({:.1}%)", pc, c, *c as f64 / 2000.0);
    }

    println!();
    println!("IME={} IE=0x{:04X} IF=0x{:04X}", gba.mmu.ime, gba.mmu.ie, gba.mmu.if_reg);
    println!("halted={}", gba.cpu.halted);
    println!("dispstat=0x{:04X} vcount={}", gba.mmu.ppu.dispstat, gba.mmu.ppu.vcount);
    for i in 0..4 {
        let ch = &gba.mmu.dma.channels[i];
        println!(
            "DMA{} en={} cnt_h=0x{:04X} timing={} sad=0x{:08X} dad=0x{:08X} count={}",
            i,
            ch.enabled,
            ch.cnt_h,
            (ch.cnt_h >> 12) & 3,
            ch.sad,
            ch.dad,
            ch.count
        );
    }
}
