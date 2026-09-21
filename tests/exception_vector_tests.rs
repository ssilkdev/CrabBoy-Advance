//! Regression tests for the "runs off into IO space" class of crash.
//!
//! Two independent defects produced the same visible symptom (game freezes,
//! diagnostic report shows PC in 0x04xxxxxx IO space):
//!
//!   1. `save_state()` never serialized IME/IE/DMA/timers, so a restored
//!      state resumed with interrupts disabled and DMA cleared.
//!   2. The undefined-instruction exception vectors to 0x00000004, but the
//!      HLE BIOS only populates the IRQ stub at 0x18. The rest of the BIOS is
//!      zeros, which decode as `andeq r0,r0,r0` -- so the CPU quietly walks
//!      up through BIOS and off into IO space instead of trapping.

use gba_simulator::gba::Gba;

/// An unmapped/undefined ARM instruction must not send the CPU wandering
/// into IO register space.
#[test]
fn undefined_instruction_does_not_escape_into_io_space() {
    let mut gba = Gba::new();

    // Minimal ROM whose first instruction is an undefined encoding.
    // 0xE7FFFFFF is the canonical ARM "permanently undefined" space.
    let mut rom = vec![0u8; 0x200];
    rom[0..4].copy_from_slice(&0xE7FF_FFFFu32.to_le_bytes());
    gba.load_rom_bytes(rom);

    for i in 0..2000 {
        gba.step_instruction();
        let pc = gba.cpu.regs[15];
        assert!(
            !(0x0400_0000..0x0500_0000).contains(&pc),
            "step {}: undefined instruction let PC reach IO space (0x{:08X})",
            i,
            pc
        );
    }
}

/// The BIOS exception vectors the CPU can actually reach must contain
/// something that is not a fall-through, or execution runs away.
#[test]
fn bios_exception_vectors_are_populated() {
    let gba = Gba::new();

    // 0x04 = undefined instruction, 0x18 = IRQ. Both are reachable from
    // `arm.rs` / `trigger_irq`, so both must be real code. (0x08/SWI is
    // intentionally absent -- it's serviced by HLE, never vectored to.)
    for vector in [0x04u32, 0x18u32] {
        let word = gba.mmu.read32(vector);
        assert_ne!(
            word, 0,
            "BIOS exception vector 0x{:02X} is zero -- the CPU will fall \
             through it and walk into IO space",
            vector
        );
    }
}
