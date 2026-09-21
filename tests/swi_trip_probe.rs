//! Trip the instant PC reaches the SWI vector (0x08) and dump a long history.
//!
//! swi_vector_probe.rs proved PC reaches 0x00000008 exactly once across the
//! battle transition, and that the BIOS IRQ dispatcher at 0x18 is entered once
//! in Supervisor mode rather than IRQ mode. Those are the same event: SVC entry
//! at 0x08 falls through the zero-filled 0x08..0x18 into the IRQ dispatcher.
//!
//! This dumps the instruction that vectored there.

use gba_simulator::gba::keypad::Key;
use gba_simulator::gba::{Gba, CYCLES_PER_FRAME};

const RING: usize = 64;

#[derive(Clone, Copy, Default)]
struct T {
    pc: u32,
    op: u32,
    thumb: bool,
    cpsr: u32,
    r: [u32; 16],
}

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

fn reg(a: u32) -> &'static str {
    match (a >> 24) & 0xFF {
        0x00 => "BIOS",
        0x02 => "EWRAM",
        0x03 => "IWRAM",
        0x04 => "IO",
        0x05 => "PAL",
        0x06 => "VRAM",
        0x07 => "OAM",
        0x08..=0x0D => "ROM",
        0x0E => "SRAM",
        _ => "??",
    }
}

#[test]
#[ignore]
fn trip_on_swi_vector() {
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

    let mut ring = [T::default(); RING];
    let mut idx = 0usize;
    let mut n = 0u64;

    'outer: for step in 0..60 {
        let dir = if (step / 2) % 2 == 0 { Key::Up } else { Key::Down };
        gba.mmu.keypad.set_key_state(dir, true);
        for _ in 0..16 {
            let mut fc = 0u32;
            while fc < CYCLES_PER_FRAME {
                let pc = gba.cpu.regs[15];
                let thumb = gba.cpu.is_thumb();
                let op = if thumb { gba.mmu.read16(pc) as u32 } else { gba.mmu.read32(pc) };
                ring[idx] = T { pc, op, thumb, cpsr: gba.cpu.cpsr, r: gba.cpu.regs };
                idx = (idx + 1) % RING;
                n += 1;

                if pc == 0x0000_0008 {
                    println!("=== PC REACHED SWI VECTOR 0x08 at instruction {} ===", n);
                    println!("cpsr=0x{:08X} mode=0x{:02X}  spsr_svc=0x{:08X}  lr=0x{:08X}",
                        gba.cpu.cpsr, gba.cpu.cpsr & 0x1F, gba.cpu.spsr_svc, gba.cpu.regs[14]);
                    println!();
                    println!("preceding {} instructions (oldest first):", RING - 1);
                    for i in 1..RING {
                        let t = ring[(idx + i) % RING];
                        println!(
                            "  {:<5} {:08X} {} op={:08X} cpsr={:08X} m={:02X} lr={:08X} sp={:08X}",
                            reg(t.pc), t.pc, if t.thumb { "T" } else { "A" },
                            t.op, t.cpsr, t.cpsr & 0x1F, t.r[14], t.r[13]
                        );
                    }
                    // The instruction just before landing on 0x08 is the culprit.
                    let culprit = ring[(idx + RING - 2) % RING];
                    println!();
                    println!("CULPRIT: pc=0x{:08X} {} op=0x{:08X}",
                        culprit.pc, if culprit.thumb { "THUMB" } else { "ARM" }, culprit.op);
                    if !culprit.thumb {
                        let cond = (culprit.op >> 28) & 0xF;
                        println!("  cond nibble = 0x{:X}", cond);
                        println!("  bits 27..24 = 0x{:X}", (culprit.op >> 24) & 0xF);
                        println!("  arm.rs SWI test (instr & 0x0F000000)==0x0F000000 -> {}",
                            (culprit.op & 0x0F00_0000) == 0x0F00_0000);
                        let comment = culprit.op & 0x00FF_FFFF;
                        let swi_num = if comment >= 0x10000 { ((comment >> 16) & 0xFF) as u8 }
                                      else { (comment & 0xFF) as u8 };
                        println!("  comment = 0x{:06X} -> swi_num = 0x{:02X} (HLE range: {})",
                            comment, swi_num, swi_num <= 0x2A);
                    } else {
                        println!("  thumb SWI test (op & 0xFF00)==0xDF00 -> {}",
                            (culprit.op & 0xFF00) == 0xDF00);
                    }
                    break 'outer;
                }

                fc += gba.step_instruction();
            }
            gba.mmu.ppu.frame_ready = false;
        }
        gba.mmu.keypad.set_key_state(dir, false);
    }
}
