//! Trace backwards from the grass-battle derail.
//!
//! The stuck loop at 0x08C0000C is garbage data being executed. This probe
//! single-steps the CPU across the battle transition, keeping a ring buffer of
//! the last N executed instructions, and dumps it the instant PC first enters
//! the runaway region. That gives us the instruction that actually branched
//! into the weeds, rather than the symptom.

use gba_simulator::gba::keypad::Key;
use gba_simulator::gba::{Gba, CYCLES_PER_FRAME};

const RING: usize = 96;

#[derive(Clone, Copy, Default)]
struct Trace {
    pc: u32,
    opcode: u32,
    thumb: bool,
    cpsr: u32,
    regs: [u32; 16],
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

fn region(addr: u32) -> &'static str {
    match (addr >> 24) & 0xFF {
        0x00 => "BIOS",
        0x02 => "EWRAM",
        0x03 => "IWRAM",
        0x04 => "IO-REGS",
        0x05 => "PALETTE",
        0x06 => "VRAM",
        0x07 => "OAM",
        0x08 | 0x09 => "ROM0",
        0x0A | 0x0B => "ROM1",
        0x0C | 0x0D => "ROM2",
        0x0E => "SRAM",
        _ => "UNMAPPED",
    }
}

#[test]
#[ignore]
fn trace_backwards_from_derail() {
    let rom = std::env::var("GBA_TEST_ROM").unwrap();
    let mut gba = Gba::new();
    gba.load_rom(&rom).unwrap();

    // Boot + title + continue, same sequence as loop_disasm.rs.
    press(&mut gba, &[], 240);
    for _ in 0..12 {
        press(&mut gba, &[Key::Start], 6);
        press(&mut gba, &[], 24);
    }
    for _ in 0..10 {
        press(&mut gba, &[Key::A], 6);
        press(&mut gba, &[], 20);
    }

    // Walk in grass, but single-step so we can watch every instruction.
    let mut ring = [Trace::default(); RING];
    let mut idx = 0usize;
    let mut total = 0u64;
    let mut tripped = false;

    'outer: for step in 0..60 {
        let dir = if (step / 2) % 2 == 0 { Key::Up } else { Key::Down };
        for k in [dir] {
            gba.mmu.keypad.set_key_state(k, true);
        }
        for _ in 0..16 {
            let mut frame_cycles = 0u32;
            while frame_cycles < CYCLES_PER_FRAME {
                let pc = gba.cpu.regs[15];
                let thumb = gba.cpu.is_thumb();
                let opcode = if thumb {
                    gba.mmu.read16(pc) as u32
                } else {
                    gba.mmu.read32(pc)
                };
                ring[idx] = Trace {
                    pc,
                    opcode,
                    thumb,
                    cpsr: gba.cpu.cpsr,
                    regs: gba.cpu.regs,
                };
                idx = (idx + 1) % RING;
                total += 1;

                // Trip: PC entered the known runaway region.
                if (0x08C0_0000..0x08C0_1000).contains(&pc) {
                    println!("=== DERAIL at instruction {} ===", total);
                    println!("PC entered 0x{:08X} ({})", pc, region(pc));
                    println!();
                    println!("last {} instructions (oldest first):", RING - 1);
                    for i in 1..RING {
                        let t = ring[(idx + i) % RING];
                        if t.pc == 0 && t.opcode == 0 {
                            continue;
                        }
                        println!(
                            "  {:>9} {:08X}  {}  op={:08X}  cpsr={:08X} mode={:02X}  lr={:08X} sp={:08X}",
                            region(t.pc),
                            t.pc,
                            if t.thumb { "T" } else { "A" },
                            t.opcode,
                            t.cpsr,
                            t.cpsr & 0x1F,
                            t.regs[14],
                            t.regs[13],
                        );
                    }
                    println!();
                    let last = ring[(idx + RING - 1) % RING];
                    println!("regs at derail:");
                    for r in 0..16 {
                        println!("  r{:<2} = 0x{:08X}  [{}]", r, last.regs[r], region(last.regs[r]));
                    }
                    tripped = true;
                    break 'outer;
                }

                frame_cycles += gba.step_instruction();
            }
            gba.mmu.ppu.frame_ready = false;
        }
        for k in [dir] {
            gba.mmu.keypad.set_key_state(k, false);
        }
    }

    if !tripped {
        println!("no derail observed in {} instructions", total);
    }
}
