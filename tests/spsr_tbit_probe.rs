//! Prove the SPSR T-bit is being dropped by MSR.
//!
//! Hypothesis: `arm.rs` MSR masks bit 5 out of the control field
//! (`mask |= 0x0000_00DF`) for BOTH CPSR and SPSR writes. Masking T is correct
//! for CPSR (ARMv4: changing T via MSR is UNPREDICTABLE) but WRONG for SPSR --
//! the SPSR is a data register and MSR must write all 32 bits.
//!
//! Emerald's IntrMain does `mrs r0, spsr` on entry, re-enables IRQs, runs the
//! handler in System mode, then restores with `msr spsr_fc, r0`. A NESTED IRQ
//! during the handler overwrites SPSR_irq with the System-mode CPSR (T=0). The
//! restore should put T=1 back; with bit 5 masked it cannot, so the final
//! `subs pc, lr, #4` resumes THUMB code in ARM state.
//!
//! This probe watches the BIOS IRQ return at 0x2C and reports every case where
//! the restored CPSR's T bit disagrees with the return address alignment.

use gba_simulator::gba::keypad::Key;
use gba_simulator::gba::{Gba, CYCLES_PER_FRAME};

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
fn spsr_tbit_probe() {
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

    let mut irq_returns = 0u64;
    let mut nested = 0u64;
    let mut in_irq_depth = 0i32;
    let mut max_depth = 0i32;
    let mut mismatches = 0u64;
    let mut first_mismatch: Option<(u32, u32)> = None; // (spsr, lr)

    for step in 0..60 {
        let dir = if (step / 2) % 2 == 0 { Key::Up } else { Key::Down };
        gba.mmu.keypad.set_key_state(dir, true);
        for _ in 0..16 {
            let mut fc = 0u32;
            while fc < CYCLES_PER_FRAME {
                let pc = gba.cpu.regs[15];

                if pc == 0x0000_0018 {
                    in_irq_depth += 1;
                    if in_irq_depth > 1 {
                        nested += 1;
                    }
                    max_depth = max_depth.max(in_irq_depth);
                }

                // BIOS IRQ epilogue: subs pc, lr, #4
                if pc == 0x0000_002C {
                    irq_returns += 1;
                    in_irq_depth = (in_irq_depth - 1).max(0);
                    let spsr = gba.cpu.spsr_irq;
                    let lr = gba.cpu.regs[14];
                    let target = lr.wrapping_sub(4);
                    let spsr_thumb = (spsr & (1 << 5)) != 0;
                    // A word-misaligned return target can only be THUMB code.
                    let must_be_thumb = (target & 3) != 0;
                    if must_be_thumb && !spsr_thumb {
                        mismatches += 1;
                        if first_mismatch.is_none() {
                            first_mismatch = Some((spsr, lr));
                        }
                    }
                }

                fc += gba.step_instruction();
            }
            gba.mmu.ppu.frame_ready = false;
        }
        gba.mmu.keypad.set_key_state(dir, false);
    }

    println!("IRQ returns observed : {}", irq_returns);
    println!("nested IRQ entries   : {}", nested);
    println!("max IRQ nesting depth: {}", max_depth);
    println!("T-bit mismatches     : {}", mismatches);
    if let Some((spsr, lr)) = first_mismatch {
        println!(
            "  first: spsr_irq=0x{:08X} (T={}) lr=0x{:08X} -> resume 0x{:08X} (halfword-aligned => THUMB)",
            spsr,
            (spsr >> 5) & 1,
            lr,
            lr.wrapping_sub(4)
        );
    }
}
