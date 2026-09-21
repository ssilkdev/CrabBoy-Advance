//! Regression tests for ARM `MSR SPSR_<fields>, <operand>`.
//!
//! Two defects made the Pokemon Emerald grass-battle transition derail:
//!
//!   1. The MSR decode mask (0x0D60_F000) constrained bit 22 (the R bit) to 0,
//!      so every `msr spsr_*` form failed to decode. It fell through to the
//!      data-processing decoder, where the same word is CMN with S=0 -- a
//!      silent no-op. MSR SPSR was entirely unimplemented.
//!
//!   2. The control-byte write mask was 0xDF for both CPSR and SPSR, i.e. the
//!      T-bit was dropped. That is correct for CPSR (UNPREDICTABLE on ARMv4T)
//!      but wrong for SPSR, which is a plain data register here and must accept
//!      all 32 bits.
//!
//! Emerald's IntrMain reads SPSR on entry and writes it back with
//! `msr spsr_fc, r0` before the BIOS `subs pc, lr, #4`. With the write dropped,
//! the saved THUMB bit was lost and interrupted THUMB code resumed in ARM
//! state, running off into uninitialised ROM.

use gba_simulator::gba::cpu::{Arm7Tdmi, CpuMode};
use gba_simulator::gba::mmu::Mmu;

/// Assemble the instruction at 0x02000000 in EWRAM and single-step it.
fn exec_arm(cpu: &mut Arm7Tdmi, mmu: &mut Mmu, instr: u32) {
    let pc = 0x0200_0000u32;
    mmu.write32(pc, instr);
    cpu.regs[15] = pc;
    gba_simulator::gba::cpu::arm::step_arm(cpu, mmu);
}

fn setup() -> (Arm7Tdmi, Mmu) {
    let mut cpu = Arm7Tdmi::new();
    let mmu = Mmu::new();
    cpu.set_mode(CpuMode::Irq);
    cpu.cpsr = (cpu.cpsr & !0x1F) | (CpuMode::Irq as u32);
    (cpu, mmu)
}

#[test]
fn msr_spsr_register_form_is_decoded_at_all() {
    let (mut cpu, mut mmu) = setup();
    cpu.spsr_irq = 0x0000_0000;
    cpu.regs[0] = 0x6000_001F; // System mode, NZ set

    // E169F000 = msr spsr_fc, r0   (this is what Emerald's IntrMain executes)
    exec_arm(&mut cpu, &mut mmu, 0xE169_F000);

    assert_ne!(
        cpu.spsr_irq, 0x0000_0000,
        "MSR SPSR did not decode at all -- the R bit (22) is being constrained \
         to 0 by the decode mask, so `msr spsr_*` falls through to the \
         data-processing decoder and silently does nothing"
    );
    assert_eq!(
        cpu.spsr_irq & 0x1F,
        0x1F,
        "MSR SPSR did not write the mode field"
    );
}

#[test]
fn msr_spsr_preserves_the_thumb_bit() {
    let (mut cpu, mut mmu) = setup();
    cpu.spsr_irq = 0x0000_0000;
    // System mode with T set -- an interrupted THUMB routine.
    cpu.regs[0] = 0x0000_003F;

    exec_arm(&mut cpu, &mut mmu, 0xE169_F000); // msr spsr_fc, r0

    assert_eq!(
        cpu.spsr_irq & (1 << 5),
        1 << 5,
        "MSR SPSR dropped the T bit. The SPSR is a data register, not live \
         execution state -- masking bit 5 corrupts the saved THUMB state and \
         makes the IRQ return resume THUMB code in ARM mode"
    );
    assert_eq!(cpu.spsr_irq, 0x0000_003F);
}

#[test]
fn msr_cpsr_still_masks_the_thumb_bit() {
    let (mut cpu, mut mmu) = setup();
    // Writing T via MSR CPSR is UNPREDICTABLE on ARMv4T and must stay masked:
    // it would switch execution state behind the pipeline's back.
    let before_t = cpu.cpsr & (1 << 5);
    cpu.regs[0] = 0x0000_003F; // asks for T=1, System mode

    exec_arm(&mut cpu, &mut mmu, 0xE129_F000); // msr cpsr_fc, r0

    assert_eq!(
        cpu.cpsr & (1 << 5),
        before_t,
        "MSR CPSR must not change the T bit"
    );
    assert_eq!(cpu.cpsr & 0x1F, 0x1F, "MSR CPSR should still change mode");
}

#[test]
fn irq_return_resumes_thumb_after_spsr_restore() {
    // End-to-end shape of Emerald's IntrMain epilogue:
    //   mrs r0, spsr        (save)
    //   ...handler runs in System mode, clobbering SPSR_irq...
    //   msr spsr_fc, r0     (restore)
    //   subs pc, lr, #4     (return)
    let (mut cpu, mut mmu) = setup();

    // Interrupted THUMB code at 0x080008CE.
    let saved_spsr = 0x0000_003F; // System mode, T=1
    cpu.spsr_irq = saved_spsr;

    // mrs r0, spsr
    exec_arm(&mut cpu, &mut mmu, 0xE14F_0000);
    assert_eq!(cpu.regs[0], saved_spsr, "MRS SPSR failed");

    // Handler clobbers SPSR_irq with a System-mode (ARM) value.
    cpu.spsr_irq = 0x6000_001F;

    // msr spsr_fc, r0 -- restore
    exec_arm(&mut cpu, &mut mmu, 0xE169_F000);
    assert_eq!(
        cpu.spsr_irq, saved_spsr,
        "SPSR restore lost the saved state"
    );

    // subs pc, lr, #4
    cpu.regs[14] = 0x0800_08D2;
    exec_arm(&mut cpu, &mut mmu, 0xE25E_F004);

    assert!(
        cpu.is_thumb(),
        "IRQ return resumed in ARM state despite the saved SPSR having T=1 -- \
         this is the grass-battle derail: THUMB code at an odd halfword address \
         executed as ARM"
    );
    assert_eq!(
        cpu.regs[15], 0x0800_08CE,
        "resumed at the wrong address"
    );
}
