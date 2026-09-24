//! ARM 32-bit Instruction Decoder & Execution

use super::alu::{add_with_carry, barrel_shift, sub_with_borrow, ShiftType};
use super::{Arm7Tdmi, FLAG_C, FLAG_N, FLAG_T, FLAG_V, FLAG_Z};
use crate::gba::mmu::Mmu;

/// Execute one ARM instruction through the two-stage prefetch pipeline.
///
/// Steady state costs one memory read per instruction (the fetch of PC+8),
/// the same as fetching PC directly. After a branch the pipeline refills.
pub fn step_arm(cpu: &mut Arm7Tdmi, mmu: &mut Mmu) -> u32 {
    let pc = cpu.regs[15];
    let waits_before = mmu.timing.waits.get();
    let (instr, next) = if cpu.pipe_valid && cpu.pipe_addr == pc {
        (cpu.pipe[0], cpu.pipe[1])
    } else {
        (mmu.fetch32(pc), mmu.fetch32(pc.wrapping_add(4)))
    };
    // Fetch stage: PC+8 is read before this instruction executes. It is
    // also what the data bus holds, so it's the open-bus value.
    let fetched = mmu.fetch32(pc.wrapping_add(8));
    mmu.open_bus = fetched;
    let cycles = execute_arm(cpu, mmu, instr);
    // Keep the pipeline only for straight-line ARM execution.
    if cpu.regs[15] == pc.wrapping_add(4) && !cpu.is_thumb() {
        cpu.pipe = [next, fetched];
        cpu.pipe_addr = cpu.regs[15];
        cpu.pipe_valid = true;
    } else {
        cpu.pipe_valid = false;
    }
    // Base cycles (1 per access + internal) plus memory wait states.
    cycles + mmu.timing.waits.get().wrapping_sub(waits_before)
}

fn execute_arm(cpu: &mut Arm7Tdmi, mmu: &mut Mmu, instr: u32) -> u32 {
    let pc = cpu.regs[15];
    cpu.regs[15] = pc.wrapping_add(4);

    let cond = (instr >> 28) & 0xF;
    if !cpu.check_condition(cond) {
        return 1;
    }

    // Branch and Branch with Link (B, BL)
    if (instr & 0x0E00_0000) == 0x0A00_0000 {
        let is_link = (instr & (1 << 24)) != 0;
        let offset = ((instr & 0x00FF_FFFF) as i32) << 8 >> 6; // Sign extend and multiply by 4
        if is_link {
            cpu.regs[14] = pc.wrapping_add(4);
        }
        cpu.regs[15] = pc.wrapping_add(8).wrapping_add(offset as u32);
        return 3;
    }

    // Branch and Exchange (BX)
    if (instr & 0x0FFF_FFF0) == 0x012F_FF10 {
        let rm = (instr & 0xF) as usize;
        let target = cpu.regs[rm];
        if (target & 1) != 0 {
            cpu.set_flag(FLAG_T, true);
            cpu.regs[15] = target & !1;
        } else {
            cpu.regs[15] = target & !3;
        }
        return 3;
    }

    // Software Interrupt (SWI)
    if (instr & 0x0F00_0000) == 0x0F00_0000 {
        let comment = instr & 0x00FF_FFFF;
        mmu.handle_swi(cpu, comment);
        return 3;
    }

    // Block Data Transfer (LDM, STM)
    //
    // ARM7TDMI quirks (ROADMAP M1, jsmolka arm.gba 500-5xx):
    // - Addresses are force-aligned and never rotated, but writeback uses
    //   the unaligned base +/- 4*n.
    // - Empty register list: transfers R15 only and moves the base by 0x40.
    // - STM with the base in the list: the first register stored stores the
    //   old base; any later position stores the written-back base.
    // - LDM with the base in the list: the loaded value wins (no writeback).
    // - S bit without R15 in an LDM (or on any STM): user-bank registers.
    if (instr & 0x0E00_0000) == 0x0800_0000 {
        let p = (instr & (1 << 24)) != 0;
        let u = (instr & (1 << 23)) != 0;
        let s = (instr & (1 << 22)) != 0;
        let w = (instr & (1 << 21)) != 0;
        let l = (instr & (1 << 20)) != 0;
        let rn = ((instr >> 16) & 0xF) as usize;
        let empty = instr & 0xFFFF == 0;
        let reg_list = if empty { 1 << 15 } else { instr & 0xFFFF };

        let base = cpu.regs[rn];
        let num_regs = reg_list.count_ones();
        let total_bytes = if empty { 0x40 } else { num_regs * 4 };

        let start_addr = if u {
            if p { base.wrapping_add(4) } else { base }
        } else if p {
            base.wrapping_sub(total_bytes)
        } else {
            base.wrapping_sub(total_bytes).wrapping_add(4)
        };
        let final_addr = if u { base.wrapping_add(total_bytes) } else { base.wrapping_sub(total_bytes) };

        let pc_in_list = (reg_list & (1 << 15)) != 0;
        let user_bank = s && !(l && pc_in_list);
        let orig_mode = cpu.get_mode();
        if user_bank {
            cpu.set_mode(super::CpuMode::System);
        }

        let lowest = reg_list.trailing_zeros() as usize;
        let mut cur_addr = start_addr;
        for r in 0..16 {
            if (reg_list & (1 << r)) == 0 {
                continue;
            }
            let aligned = cur_addr & !3;
            if l {
                let val = mmu.cpu_read32(aligned);
                if r == 15 {
                    if s {
                        let spsr = cpu.get_spsr();
                        // Only an exception return from IRQ mode ends IRQ
                        // servicing (not SWI/UND returns).
                        if cpu.cpsr & 0x1F == 0x12 {
                            cpu.in_irq = false;
                        }
                        cpu.set_cpsr(spsr);
                        cpu.regs[15] = if (spsr & FLAG_T) != 0 { val & !1 } else { val & !3 };
                    } else {
                        cpu.regs[15] = val & !3;
                    }
                } else {
                    cpu.regs[r] = val;
                }
            } else {
                let val = if r == 15 {
                    pc.wrapping_add(12)
                } else if r == rn && w && r != lowest && !user_bank {
                    final_addr
                } else {
                    cpu.regs[r]
                };
                mmu.cpu_write32(aligned, val);
            }
            cur_addr = cur_addr.wrapping_add(4);
        }

        if user_bank {
            cpu.set_mode(orig_mode);
        }
        if w && (!l || (reg_list & (1 << rn)) == 0) {
            cpu.regs[rn] = final_addr;
        }

        return if l && pc_in_list { num_regs + 3 } else { num_regs + 2 };
    }

    // Single Data Transfer (LDR, STR, LDRB, STRB)
    if (instr & 0x0C00_0000) == 0x0400_0000 {
        let i = (instr & (1 << 25)) != 0;
        let p = (instr & (1 << 24)) != 0;
        let u = (instr & (1 << 23)) != 0;
        let b = (instr & (1 << 22)) != 0;
        let w = (instr & (1 << 21)) != 0;
        let l = (instr & (1 << 20)) != 0;
        let rn = ((instr >> 16) & 0xF) as usize;
        let rd = ((instr >> 12) & 0xF) as usize;

        let base = if rn == 15 { pc.wrapping_add(8) } else { cpu.regs[rn] };

        let offset = if !i {
            instr & 0xFFF
        } else {
            let rm = (instr & 0xF) as usize;
            let val = if rm == 15 { pc.wrapping_add(8) } else { cpu.regs[rm] };
            let shift_type = ShiftType::from_u32((instr >> 5) & 3);
            let shift_amount = (instr >> 7) & 0x1F;
            let (res, _) = barrel_shift(shift_type, val, shift_amount, cpu.get_flag(FLAG_C), true);
            res
        };

        let eff_addr = if u { base.wrapping_add(offset) } else { base.wrapping_sub(offset) };
        let target_addr = if p { eff_addr } else { base };

        if l {
            let val = if b {
                mmu.cpu_read8(target_addr) as u32
            } else {
                mmu.cpu_read32(target_addr)
            };
            if rd == 15 {
                cpu.regs[15] = val & !3;
            } else {
                cpu.regs[rd] = val;
            }
        } else {
            let val = if rd == 15 { pc.wrapping_add(12) } else { cpu.regs[rd] };
            if b {
                mmu.cpu_write8(target_addr, (val & 0xFF) as u8);
            } else {
                mmu.cpu_write32(target_addr, val);
            }
        }

        if (!p || w)
            && !(l && rd == rn) {
                cpu.regs[rn] = eff_addr;
            }

        return if l { 3 } else { 2 };
    }

    // Single Data Swap (SWP, SWPB)
    if (instr & 0x0FB0_0FF0) == 0x0100_0090 {
        let b = (instr & (1 << 22)) != 0; // SWPB if set
        let rn = ((instr >> 16) & 0xF) as usize;
        let rd = ((instr >> 12) & 0xF) as usize;
        let rm = (instr & 0xF) as usize;

        let addr = cpu.regs[rn];
        if b {
            let old = mmu.cpu_read8(addr);
            mmu.cpu_write8(addr, cpu.regs[rm] as u8);
            cpu.regs[rd] = old as u32;
        } else {
            let old = mmu.cpu_read32(addr);
            mmu.cpu_write32(addr, cpu.regs[rm]);
            cpu.regs[rd] = old;
        }
        return 4; // SWP takes 1S + 2N + 1I cycles
    }

    // Halfword and Signed Data Transfer (LDRH, STRH, LDRSB, LDRSH)
    // Bits 6..5 (op / SH) must be non-zero (01=H, 10=SB, 11=SH); 00 is Multiply or SWP.
    if (instr & 0x0E00_0090) == 0x0000_0090 && ((instr >> 5) & 3) != 0 {
        let p = (instr & (1 << 24)) != 0;
        let u = (instr & (1 << 23)) != 0;
        let i = (instr & (1 << 22)) != 0;
        let w = (instr & (1 << 21)) != 0;
        let l = (instr & (1 << 20)) != 0;
        let rn = ((instr >> 16) & 0xF) as usize;
        let rd = ((instr >> 12) & 0xF) as usize;
        let op = (instr >> 5) & 3;

        let base = if rn == 15 { pc.wrapping_add(8) } else { cpu.regs[rn] };
        let offset = if i {
            ((instr >> 4) & 0xF0) | (instr & 0x0F)
        } else {
            let rm = (instr & 0xF) as usize;
            if rm == 15 { pc.wrapping_add(8) } else { cpu.regs[rm] }
        };

        let eff_addr = if u { base.wrapping_add(offset) } else { base.wrapping_sub(offset) };
        let target_addr = if p { eff_addr } else { base };

        if l {
            let val = match op {
                1 => super::load_halfword(mmu, target_addr),
                2 => (mmu.cpu_read8(target_addr) as i8) as i32 as u32,
                3 => super::load_signed_halfword(mmu, target_addr),
                _ => 0,
            };
            if rd == 15 {
                cpu.regs[15] = val & !3;
            } else {
                cpu.regs[rd] = val;
            }
        } else {
            let val = if rd == 15 { pc.wrapping_add(12) } else { cpu.regs[rd] };
            mmu.cpu_write16(target_addr, (val & 0xFFFF) as u16);
        }

        if (!p || w)
            && !(l && rd == rn) {
                cpu.regs[rn] = eff_addr;
            }

        return 3;
    }

    // Multiply and Multiply-Accumulate
    if (instr & 0x0FC0_00F0) == 0x0000_0090 {
        let a = (instr & (1 << 21)) != 0;
        let s = (instr & (1 << 20)) != 0;
        let rd = ((instr >> 16) & 0xF) as usize;
        let rn = ((instr >> 12) & 0xF) as usize;
        let rs = ((instr >> 8) & 0xF) as usize;
        let rm = (instr & 0xF) as usize;

        let mut res = cpu.regs[rm].wrapping_mul(cpu.regs[rs]);
        if a {
            res = res.wrapping_add(cpu.regs[rn]);
        }
        cpu.regs[rd] = res;

        if s {
            cpu.set_flag(FLAG_N, (res & 0x8000_0000) != 0);
            cpu.set_flag(FLAG_Z, res == 0);
        }
        return 3;
    }

    // Multiply Long (UMULL, UMLAL, SMULL, SMLAL)
    if (instr & 0x0F80_00F0) == 0x0080_0090 {
        let is_signed = (instr & (1 << 22)) != 0;
        let a = (instr & (1 << 21)) != 0;
        let s = (instr & (1 << 20)) != 0;
        let rd_hi = ((instr >> 16) & 0xF) as usize;
        let rd_lo = ((instr >> 12) & 0xF) as usize;
        let rs = ((instr >> 8) & 0xF) as usize;
        let rm = (instr & 0xF) as usize;

        let mut res = if is_signed {
            ((cpu.regs[rm] as i32 as i64) * (cpu.regs[rs] as i32 as i64)) as u64
        } else {
            (cpu.regs[rm] as u64) * (cpu.regs[rs] as u64)
        };

        if a {
            let acc = ((cpu.regs[rd_hi] as u64) << 32) | (cpu.regs[rd_lo] as u64);
            res = res.wrapping_add(acc);
        }

        cpu.regs[rd_lo] = (res & 0xFFFF_FFFF) as u32;
        cpu.regs[rd_hi] = (res >> 32) as u32;

        if s {
            cpu.set_flag(FLAG_N, (res & 0x8000_0000_0000_0000) != 0);
            cpu.set_flag(FLAG_Z, res == 0);
        }
        return 4;
    }

    // Status Register Access (MRS, MSR)
    if (instr & 0x0FBF_0FFF) == 0x010F_0000 {
        // MRS
        let r = (instr & (1 << 22)) != 0;
        let rd = ((instr >> 12) & 0xF) as usize;
        cpu.regs[rd] = if r { cpu.get_spsr() } else { cpu.cpsr };
        return 1;
    }

    let is_msr_i = (instr & (1 << 25)) != 0;
    // MSR encoding: cond 00 I 10 R 10 field_mask 1111 <operand>
    //
    // Fixed bits that must be matched: 27..26 = 00, 24 = 1, 23 = 0, 21 = 1,
    // 20 = 0, and 15..12 = 1111. Bit 25 (I) and bit 22 (R) are operands and
    // must be DON'T CARE.
    //
    // The previous mask was 0x0D60_F000, which left bit 23 unchecked but
    // *constrained bit 22 to 0*. That made every `msr spsr_<f>, Rm` (R=1) fail
    // the test and fall through to the data-processing decoder, where the same
    // word decodes as CMN with S=0 -- a silent no-op. MSR SPSR was therefore
    // completely unimplemented.
    //
    // Pokemon Emerald's IntrMain saves SPSR on entry, re-enables IRQs in System
    // mode to run the game handler, then restores the saved value with
    // `msr spsr_fc, r0` (0xE169F000) before returning through the BIOS
    // `subs pc, lr, #4`. With the restore silently dropped, a nested IRQ during
    // the battle transition left SPSR_irq holding the System-mode CPSR (T=0),
    // so the interrupted THUMB code at 0x080008CE was resumed in ARM state at
    // 0x080008CC. Execution then ran off into uninitialised ROM at 0x08C00008.
    if (instr & 0x0DB0_F000) == 0x0120_F000 && (is_msr_i || (instr & 0x0FF0) == 0) {
        // MSR (Status Register Access)
        let r = (instr & (1 << 22)) != 0;
        let mask_ctrl = (instr & (1 << 16)) != 0;
        let mask_ext = (instr & (1 << 17)) != 0;
        let mask_status = (instr & (1 << 18)) != 0;
        let mask_flags = (instr & (1 << 19)) != 0;

        let val = if is_msr_i {
            let imm = instr & 0xFF;
            let rot = ((instr >> 8) & 0xF) * 2;
            imm.rotate_right(rot)
        } else {
            cpu.regs[(instr & 0xF) as usize]
        };

        let mut mask = 0u32;
        if mask_flags {
            mask |= 0xF000_0000;
        }
        if cpu.get_mode() != super::CpuMode::User {
            if mask_status {
                mask |= 0x00FF_0000;
            }
            if mask_ext {
                mask |= 0x0000_FF00;
            }
            if mask_ctrl {
                // Control byte. The T-bit (bit 5) is a special case:
                //
                //   - MSR CPSR: writing T is UNPREDICTABLE on ARMv4T, and doing
                //     it would switch execution state behind the pipeline's
                //     back. Mask it out -> 0xDF.
                //   - MSR SPSR: the SPSR is an ordinary data register here, not
                //     the live execution state. ALL 32 bits must be writable.
                //     Masking T corrupts the saved THUMB state of whatever was
                //     interrupted, so the eventual `subs pc, lr, #4` resumes
                //     THUMB code in ARM state. Use the full 0xFF.
                //
                // Pokemon Emerald's IntrMain saves SPSR on entry and restores
                // it with `msr spsr_fc, r0` before returning. With T masked the
                // restore silently dropped T=1, the battle-transition handler
                // returned into THUMB code as ARM, and the CPU derailed into
                // uninitialised ROM at 0x08C00008.
                mask |= if r { 0x0000_00FF } else { 0x0000_00DF };
            }
        }

        if r {
            let spsr = cpu.get_spsr();
            cpu.set_spsr((spsr & !mask) | (val & mask));
        } else {
            let new_cpsr = (cpu.cpsr & !mask) | (val & mask);
            cpu.set_cpsr(new_cpsr);
        }
        return 1;
    }

    // Data Processing Operations
    if (instr & 0x0C00_0000) == 0x0000_0000 {
        let i = (instr & (1 << 25)) != 0;
        let opcode = (instr >> 21) & 0xF;
        let s = (instr & (1 << 20)) != 0;
        let rn = ((instr >> 16) & 0xF) as usize;
        let rd = ((instr >> 12) & 0xF) as usize;

        // With a register-specified shift, the ARM7TDMI spends an extra
        // internal cycle fetching Rs, so PC reads as instruction + 12 instead
        // of + 8 (for both Rn and Rm). jsmolka arm.gba tests 224/225.
        let is_reg_shift = !i && (instr & (1 << 4)) != 0;
        let pc_operand = pc.wrapping_add(if is_reg_shift { 12 } else { 8 });

        let op1 = if rn == 15 { pc_operand } else { cpu.regs[rn] };

        let (op2, shifter_carry) = if i {
            let imm = instr & 0xFF;
            let rot = ((instr >> 8) & 0xF) * 2;
            if rot == 0 {
                (imm, cpu.get_flag(FLAG_C))
            } else {
                (imm.rotate_right(rot), (imm.rotate_right(rot) & 0x8000_0000) != 0)
            }
        } else {
            let rm = (instr & 0xF) as usize;
            let val = if rm == 15 { pc_operand } else { cpu.regs[rm] };
            let shift_type = ShiftType::from_u32((instr >> 5) & 3);
            let shift_amt = if is_reg_shift {
                let rs = ((instr >> 8) & 0xF) as usize;
                cpu.regs[rs] & 0xFF
            } else {
                (instr >> 7) & 0x1F
            };

            barrel_shift(shift_type, val, shift_amt, cpu.get_flag(FLAG_C), !is_reg_shift)
        };

        let mut carry_out = shifter_carry;
        let mut overflow = cpu.get_flag(FLAG_V);

        let res = match opcode {
            0x0 => op1 & op2,                         // AND
            0x1 => op1 ^ op2,                         // EOR
            0x2 => {                                  // SUB
                let (r, c, v) = sub_with_borrow(op1, op2, true);
                carry_out = c;
                overflow = v;
                r
            }
            0x3 => {                                  // RSB
                let (r, c, v) = sub_with_borrow(op2, op1, true);
                carry_out = c;
                overflow = v;
                r
            }
            0x4 => {                                  // ADD
                let (r, c, v) = add_with_carry(op1, op2, false);
                carry_out = c;
                overflow = v;
                r
            }
            0x5 => {                                  // ADC
                let (r, c, v) = add_with_carry(op1, op2, cpu.get_flag(FLAG_C));
                carry_out = c;
                overflow = v;
                r
            }
            0x6 => {                                  // SBC
                let (r, c, v) = sub_with_borrow(op1, op2, cpu.get_flag(FLAG_C));
                carry_out = c;
                overflow = v;
                r
            }
            0x7 => {                                  // RSC
                let (r, c, v) = sub_with_borrow(op2, op1, cpu.get_flag(FLAG_C));
                carry_out = c;
                overflow = v;
                r
            }
            0x8 => op1 & op2,                         // TST
            0x9 => op1 ^ op2,                         // TEQ
            0xA => {                                  // CMP
                let (r, c, v) = sub_with_borrow(op1, op2, true);
                carry_out = c;
                overflow = v;
                r
            }
            0xB => {                                  // CMN
                let (r, c, v) = add_with_carry(op1, op2, false);
                carry_out = c;
                overflow = v;
                r
            }
            0xC => op1 | op2,                         // ORR
            0xD => op2,                               // MOV
            0xE => op1 & !op2,                        // BIC
            0xF => !op2,                              // MVN
            _ => 0,
        };

        let is_test = (0x8..=0xB).contains(&opcode);
        let mut branched = false;
        if !is_test {
            if rd == 15 {
                if s {
                    let spsr = cpu.get_spsr();
                    // Only an exception return from IRQ mode ends IRQ
                    // servicing (not SWI/UND returns).
                    if cpu.cpsr & 0x1F == 0x12 {
                        cpu.in_irq = false;
                    }
                    cpu.set_cpsr(spsr);
                    if (spsr & FLAG_T) != 0 {
                        cpu.regs[15] = res & !1;
                    } else {
                        cpu.regs[15] = res & !3;
                    }
                } else {
                    cpu.regs[15] = res & !3;
                }
                branched = true;
            } else {
                cpu.regs[rd] = res;
            }
        }

        if s && (rd != 15 || is_test) {
            cpu.set_flag(FLAG_N, (res & 0x8000_0000) != 0);
            cpu.set_flag(FLAG_Z, res == 0);
            cpu.set_flag(FLAG_C, carry_out);
            cpu.set_flag(FLAG_V, overflow);
        }

        // TST/TEQ/CMP/CMN with Rd = PC (the old 26-bit "P" forms) copy SPSR
        // into CPSR on the ARM7TDMI, which can switch mode, but do not
        // branch or flush the pipeline. jsmolka arm.gba tests 234-235.
        if is_test && rd == 15 && cpu.get_mode() != super::CpuMode::User
            && cpu.get_mode() != super::CpuMode::System
        {
            let spsr = cpu.get_spsr();
            cpu.set_cpsr(spsr);
        }

        return if branched { 3 } else { 1 };
    }

    // Undefined instruction — trigger UND exception
    cpu.trigger_undefined(pc.wrapping_add(4));
    4
}
