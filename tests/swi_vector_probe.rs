//! Does the CPU ever vector to the SWI entry at 0x00000008?
//!
//! The HLE BIOS deliberately leaves 0x08 unpopulated on the theory that SWIs
//! are always intercepted in arm.rs/thumb.rs. But `Mmu::handle_swi` only
//! HLE-services swi_num <= 0x2A; anything above calls `cpu.trigger_swi()`,
//! which enters Supervisor mode and sets PC = 0x08. 0x08..0x14 are zeros
//! (`andeq r0,r0,r0`), so execution falls straight through into the BIOS IRQ
//! dispatcher at 0x18 -- but in SVC mode, on sp_svc.
//!
//! This probe tallies every SWI number executed and trips if PC reaches 0x08
//! or if the IRQ dispatcher at 0x18 is entered in the wrong mode.

use gba_simulator::gba::keypad::Key;
use gba_simulator::gba::{Gba, CYCLES_PER_FRAME};
use std::collections::BTreeMap;

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
fn swi_vector_fallthrough_probe() {
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

    let mut swi_tally: BTreeMap<u8, u64> = BTreeMap::new();
    let mut swi_hi_first: Option<(u32, u32, u8)> = None; // (pc, opcode, num)
    let mut hit_swi_vector = 0u64;
    let mut irq_entry_modes: BTreeMap<u32, u64> = BTreeMap::new();
    let mut first_bad_irq_mode: Option<(u32, u32)> = None; // (mode, sp)
    let mut total = 0u64;

    for step in 0..60 {
        let dir = if (step / 2) % 2 == 0 { Key::Up } else { Key::Down };
        gba.mmu.keypad.set_key_state(dir, true);
        for _ in 0..16 {
            let mut frame_cycles = 0u32;
            while frame_cycles < CYCLES_PER_FRAME {
                let pc = gba.cpu.regs[15];
                let thumb = gba.cpu.is_thumb();
                total += 1;

                if pc == 0x0000_0008 {
                    hit_swi_vector += 1;
                }
                if pc == 0x0000_0018 {
                    let mode = gba.cpu.cpsr & 0x1F;
                    *irq_entry_modes.entry(mode).or_insert(0) += 1;
                    if mode != 0x12 && first_bad_irq_mode.is_none() {
                        first_bad_irq_mode = Some((mode, gba.cpu.regs[13]));
                    }
                }

                if !gba.cpu.halted {
                    if thumb {
                        let op = gba.mmu.read16(pc);
                        if (op & 0xFF00) == 0xDF00 {
                            let n = (op & 0xFF) as u8;
                            *swi_tally.entry(n).or_insert(0) += 1;
                            if n > 0x2A && swi_hi_first.is_none() {
                                swi_hi_first = Some((pc, op as u32, n));
                            }
                        }
                    } else {
                        let op = gba.mmu.read32(pc);
                        let cond = (op >> 28) & 0xF;
                        if (op & 0x0F00_0000) == 0x0F00_0000 && cond != 0xF {
                            let n = ((op >> 16) & 0xFF) as u8;
                            *swi_tally.entry(n).or_insert(0) += 1;
                            if n > 0x2A && swi_hi_first.is_none() {
                                swi_hi_first = Some((pc, op, n));
                            }
                        }
                    }
                }

                frame_cycles += gba.step_instruction();
            }
            gba.mmu.ppu.frame_ready = false;
        }
        gba.mmu.keypad.set_key_state(dir, false);
    }

    println!("instructions executed: {}", total);
    println!();
    println!("SWI numbers executed:");
    for (n, c) in &swi_tally {
        let handled = if *n <= 0x2A { "HLE" } else { "-> trigger_swi (VECTORS TO 0x08)" };
        println!("  SWI 0x{:02X}  x{:<10} {}", n, c, handled);
    }
    if let Some((pc, op, n)) = swi_hi_first {
        println!("  first out-of-range SWI: num=0x{:02X} op=0x{:08X} at pc=0x{:08X}", n, op, pc);
    }
    println!();
    println!("times PC reached the SWI vector 0x08: {}", hit_swi_vector);
    println!();
    println!("IRQ dispatcher (0x18) entries by CPSR mode:");
    for (mode, c) in &irq_entry_modes {
        let name = match mode {
            0x10 => "User",
            0x11 => "FIQ",
            0x12 => "IRQ  <- correct",
            0x13 => "SVC  <- WRONG",
            0x17 => "Abort",
            0x1B => "Undefined",
            0x1F => "System",
            _ => "?",
        };
        println!("  mode 0x{:02X} ({:<16}) x{}", mode, name, c);
    }
    if let Some((mode, sp)) = first_bad_irq_mode {
        println!("  first wrong-mode entry: mode=0x{:02X} sp=0x{:08X}", mode, sp);
    }
}
