//! ARMv4T / ARMv5TE Instruction Execution Engine
//!
//! Provides unified, cycle-accurate instruction decoding and execution for
//! both the ARM946E-S (ARMv5TE + DSP + CP15) and ARM7TDMI (ARMv4T) cores.

use crate::gba::cpu::alu::{add_with_carry, barrel_shift, sub_with_borrow, ShiftType};
use crate::gba::cpu::{FLAG_C, FLAG_N, FLAG_T, FLAG_V, FLAG_Z};

pub const FLAG_Q: u32 = 1 << 27; // Sticky overflow / saturation flag (ARMv5TE)

pub trait CpuBus {
    fn read8(&mut self, addr: u32) -> u8;
    fn read16(&mut self, addr: u32) -> u16;
    fn read32(&mut self, addr: u32) -> u32;
    fn write8(&mut self, addr: u32, val: u8);
    fn write16(&mut self, addr: u32, val: u16);
    fn write32(&mut self, addr: u32, val: u32);
}

pub trait CpuState {
    fn reg(&self, idx: usize) -> u32;
    fn set_reg(&mut self, idx: usize, val: u32);
    fn cpsr(&self) -> u32;
    fn set_cpsr(&mut self, val: u32);
    fn get_spsr(&self) -> u32;
    fn set_spsr(&mut self, val: u32);
    fn is_thumb(&self) -> bool;
    fn is_halted(&self) -> bool;
    fn set_halted(&mut self, halted: bool);
    fn check_condition(&self, cond: u32) -> bool;
    fn trigger_irq(&mut self);
    fn trigger_swi(&mut self, comment: u32);
    fn is_armv5(&self) -> bool;
    fn mcr(&mut self, crn: u32, crm: u32, op1: u32, op2: u32, val: u32);
    fn mrc(&self, crn: u32, crm: u32, op1: u32, op2: u32) -> u32;
}

/// Execute one instruction (ARM or Thumb) and return cycles taken
pub fn step_instruction<C: CpuState, B: CpuBus>(cpu: &mut C, bus: &mut B) -> u32 {
    if cpu.is_halted() {
        return 1;
    }

    if cpu.is_thumb() {
        step_thumb(cpu, bus)
    } else {
        step_arm(cpu, bus)
    }
}

// =========================================================================
// ARM Execution (32-bit)
// =========================================================================

fn step_arm<C: CpuState, B: CpuBus>(cpu: &mut C, bus: &mut B) -> u32 {
    let pc = cpu.reg(15);
    let instr = bus.read32(pc);
    let pc_val = pc.wrapping_add(8);
    cpu.set_reg(15, pc.wrapping_add(4));

    let cond = (instr >> 28) & 0xF;
    if cond == 15 {
        // Unconditional instructions in ARMv5
        if cpu.is_armv5() && (instr & 0xFE00_0000) == 0xFA00_0000 {
            // BLX <label>
            let h = (instr >> 23) & 2;
            let offset = (((instr & 0x00FF_FFFF) as i32) << 8 >> 6) as u32;
            let target = pc_val.wrapping_add(offset | h);
            cpu.set_reg(14, pc.wrapping_add(4));
            cpu.set_cpsr(cpu.cpsr() | FLAG_T);
            cpu.set_reg(15, target & !1);
            return 3;
        }
        return 1;
    }

    if !cpu.check_condition(cond) {
        return 1;
    }

    // Branch & Exchange (BX / BLX Rm)
    if (instr & 0x0FFF_FFF0) == 0x012F_FF10 {
        let rm = (instr & 0xF) as usize;
        let target = cpu.reg(rm);
        if (target & 1) != 0 {
            cpu.set_cpsr(cpu.cpsr() | FLAG_T);
            cpu.set_reg(15, target & !1);
        } else {
            cpu.set_cpsr(cpu.cpsr() & !FLAG_T);
            cpu.set_reg(15, target & !3);
        }
        return 3;
    }

    if cpu.is_armv5() && (instr & 0x0FFF_FFF0) == 0x012F_FF30 {
        // BLX Rm
        let rm = (instr & 0xF) as usize;
        let target = cpu.reg(rm);
        cpu.set_reg(14, pc.wrapping_add(4));
        if (target & 1) != 0 {
            cpu.set_cpsr(cpu.cpsr() | FLAG_T);
            cpu.set_reg(15, target & !1);
        } else {
            cpu.set_cpsr(cpu.cpsr() & !FLAG_T);
            cpu.set_reg(15, target & !3);
        }
        return 3;
    }

    // Branch & Branch with Link (B, BL)
    if (instr & 0x0E00_0000) == 0x0A00_0000 {
        let offset = (((instr & 0x00FF_FFFF) as i32) << 8 >> 6) as u32;
        if (instr & (1 << 24)) != 0 {
            cpu.set_reg(14, pc.wrapping_add(4));
        }
        cpu.set_reg(15, pc_val.wrapping_add(offset));
        return 3;
    }

    // Software Interrupt (SWI)
    if (instr & 0x0F00_0000) == 0x0F00_0000 {
        let comment = instr & 0x00FF_FFFF;
        if !crate::nds::bios::hle_swi(cpu, bus, comment, false) {
            cpu.trigger_swi(comment);
        }
        return 3;
    }

    // ARMv5TE CLZ
    if cpu.is_armv5() && (instr & 0x0FFF_0FF0) == 0x016F_0F10 {
        let rd = ((instr >> 12) & 0xF) as usize;
        let rm = (instr & 0xF) as usize;
        cpu.set_reg(rd, cpu.reg(rm).leading_zeros());
        return 1;
    }

    // ARMv5TE QADD / QSUB / QDADD / QDSUB
    if cpu.is_armv5() && (instr & 0x0F90_0FF0) == 0x0100_0050 {
        let rd = ((instr >> 12) & 0xF) as usize;
        let rn = ((instr >> 16) & 0xF) as usize;
        let rm = (instr & 0xF) as usize;
        let op = (instr >> 21) & 3;

        let a = cpu.reg(rm) as i32;
        let b = cpu.reg(rn) as i32;

        let res = match op {
            0 => sat_add(a, b, cpu),                  // QADD
            1 => sat_sub(a, b, cpu),                  // QSUB
            2 => sat_add(a, sat_double(b, cpu), cpu), // QDADD
            3 => sat_sub(a, sat_double(b, cpu), cpu), // QDSUB
            _ => 0,
        };
        cpu.set_reg(rd, res as u32);
        return 1;
    }

    // ARMv5TE DSP Multiplies (SMULxy, SMLAxy, SMULWy, SMLAWy, SMLALxy)
    if cpu.is_armv5() && (instr & 0x0F90_0090) == 0x0100_0080 {
        let rd = ((instr >> 16) & 0xF) as usize;
        let rn = ((instr >> 12) & 0xF) as usize;
        let rs = ((instr >> 8) & 0xF) as usize;
        let rm = (instr & 0xF) as usize;
        let op = (instr >> 21) & 3;
        let x_top = (instr & (1 << 5)) != 0;
        let y_top = (instr & (1 << 6)) != 0;

        let rm_val = cpu.reg(rm);
        let rs_val = cpu.reg(rs);
        let a = if x_top { (rm_val >> 16) as i16 } else { rm_val as i16 } as i32;
        let b = if y_top { (rs_val >> 16) as i16 } else { rs_val as i16 } as i32;

        match op {
            0 => {
                // SMLAxy: Rd = Rn + (Rm[x] * Rs[y])
                let prod = a * b;
                let acc = cpu.reg(rn) as i32;
                let (res, ov) = prod.overflowing_add(acc);
                if ov { cpu.set_cpsr(cpu.cpsr() | FLAG_Q); }
                cpu.set_reg(rd, res as u32);
            }
            1 => {
                if !x_top {
                    // SMLAWy: Rd = Rn + ((Rm * Rs[y]) >> 16)
                    let prod = ((cpu.reg(rm) as i32 as i64 * b as i64) >> 16) as i32;
                    let (res, ov) = prod.overflowing_add(cpu.reg(rn) as i32);
                    if ov { cpu.set_cpsr(cpu.cpsr() | FLAG_Q); }
                    cpu.set_reg(rd, res as u32);
                } else {
                    // SMULWy: Rd = (Rm * Rs[y]) >> 16
                    let prod = ((cpu.reg(rm) as i32 as i64 * b as i64) >> 16) as i32;
                    cpu.set_reg(rd, prod as u32);
                }
            }
            2 => {
                // SMLALxy: {Rd, Rn} += Rm[x] * Rs[y]
                let prod = (a as i64) * (b as i64);
                let acc = ((cpu.reg(rd) as u64) << 32) | (cpu.reg(rn) as u64);
                let sum = (acc as i64).wrapping_add(prod) as u64;
                cpu.set_reg(rn, sum as u32);
                cpu.set_reg(rd, (sum >> 32) as u32);
            }
            3 => {
                // SMULxy: Rd = Rm[x] * Rs[y]
                cpu.set_reg(rd, (a * b) as u32);
            }
            _ => {}
        }
        return 1;
    }

    // CP15 MCR / MRC (ARM9)
    if cpu.is_armv5() && (instr & 0x0F00_0010) == 0x0E00_0010 {
        let cp_num = (instr >> 8) & 0xF;
        if cp_num == 15 {
            let crn = (instr >> 16) & 0xF;
            let rd = ((instr >> 12) & 0xF) as usize;
            let crm = instr & 0xF;
            let op1 = (instr >> 21) & 0x7;
            let op2 = (instr >> 5) & 0x7;
            let is_mrc = (instr & (1 << 20)) != 0;

            if is_mrc {
                let val = cpu.mrc(crn, crm, op1, op2);
                if rd == 15 {
                    cpu.set_cpsr((cpu.cpsr() & 0x0FFF_FFFF) | (val & 0xF000_0000));
                } else {
                    cpu.set_reg(rd, val);
                }
            } else {
                let val = if rd == 15 { pc_val } else { cpu.reg(rd) };
                cpu.mcr(crn, crm, op1, op2, val);
            }
            return 2;
        }
    }

    // MRS / MSR (Status Register Transfer)
    if (instr & 0x0FBF_0FFF) == 0x010F_0000 {
        let r = (instr & (1 << 22)) != 0;
        let rd = ((instr >> 12) & 0xF) as usize;
        cpu.set_reg(rd, if r { cpu.get_spsr() } else { cpu.cpsr() });
        return 1;
    }
    if (instr & 0x0DB0_F000) == 0x0120_F000 {
        let r = (instr & (1 << 22)) != 0;
        let field_mask = instr & 0x000F_0000;
        let mut mask = 0u32;
        if (field_mask & (1 << 16)) != 0 { mask |= 0x0000_00FF; }
        if (field_mask & (1 << 17)) != 0 { mask |= 0x0000_FF00; }
        if (field_mask & (1 << 18)) != 0 { mask |= 0x00FF_0000; }
        if (field_mask & (1 << 19)) != 0 { mask |= 0xFF00_0000; }

        let operand = if (instr & (1 << 25)) != 0 {
            let imm = instr & 0xFF;
            let rot = ((instr >> 8) & 0xF) * 2;
            imm.rotate_right(rot)
        } else {
            cpu.reg((instr & 0xF) as usize)
        };

        if r {
            let cur = cpu.get_spsr();
            cpu.set_spsr((cur & !mask) | (operand & mask));
        } else {
            let cur = cpu.cpsr();
            cpu.set_cpsr((cur & !mask) | (operand & mask));
        }
        return 1;
    }

    // Single Data Swap (SWP, SWPB)
    if (instr & 0x0FB0_0FF0) == 0x0100_0090 {
        let rn = ((instr >> 16) & 0xF) as usize;
        let rd = ((instr >> 12) & 0xF) as usize;
        let rm = (instr & 0xF) as usize;
        let is_byte = (instr & (1 << 22)) != 0;
        let addr = cpu.reg(rn);

        if is_byte {
            let old = bus.read8(addr);
            bus.write8(addr, cpu.reg(rm) as u8);
            cpu.set_reg(rd, old as u32);
        } else {
            let old = bus.read32(addr & !3).rotate_right((addr & 3) * 8);
            bus.write32(addr & !3, cpu.reg(rm));
            cpu.set_reg(rd, old);
        }
        return 4;
    }

    // Multiply (MUL, MLA, UMULL, UMLAL, SMULL, SMLAL)
    if (instr & 0x0FC0_00F0) == 0x0000_0090 {
        let rd = ((instr >> 16) & 0xF) as usize;
        let rn = ((instr >> 12) & 0xF) as usize;
        let rs = ((instr >> 8) & 0xF) as usize;
        let rm = (instr & 0xF) as usize;
        let accumulate = (instr & (1 << 21)) != 0;
        let s = (instr & (1 << 20)) != 0;

        let mut res = cpu.reg(rm).wrapping_mul(cpu.reg(rs));
        if accumulate {
            res = res.wrapping_add(cpu.reg(rn));
        }
        cpu.set_reg(rd, res);
        if s {
            let mut cpsr = cpu.cpsr() & !(FLAG_N | FLAG_Z);
            if (res & 0x8000_0000) != 0 { cpsr |= FLAG_N; }
            if res == 0 { cpsr |= FLAG_Z; }
            cpu.set_cpsr(cpsr);
        }
        return 2;
    }

    if (instr & 0x0F80_00F0) == 0x0080_0090 {
        let rd_hi = ((instr >> 16) & 0xF) as usize;
        let rd_lo = ((instr >> 12) & 0xF) as usize;
        let rs = ((instr >> 8) & 0xF) as usize;
        let rm = (instr & 0xF) as usize;
        let is_signed = (instr & (1 << 22)) != 0;
        let accumulate = (instr & (1 << 21)) != 0;
        let s = (instr & (1 << 20)) != 0;

        let rm_val = cpu.reg(rm);
        let rs_val = cpu.reg(rs);
        let mut res = if is_signed {
            (rm_val as i32 as i64).wrapping_mul(rs_val as i32 as i64) as u64
        } else {
            (rm_val as u64).wrapping_mul(rs_val as u64)
        };

        if accumulate {
            let acc = ((cpu.reg(rd_hi) as u64) << 32) | (cpu.reg(rd_lo) as u64);
            res = res.wrapping_add(acc);
        }

        cpu.set_reg(rd_lo, res as u32);
        cpu.set_reg(rd_hi, (res >> 32) as u32);
        if s {
            let mut cpsr = cpu.cpsr() & !(FLAG_N | FLAG_Z);
            if (res & 0x8000_0000_0000_0000) != 0 { cpsr |= FLAG_N; }
            if res == 0 { cpsr |= FLAG_Z; }
            cpu.set_cpsr(cpsr);
        }
        return 3;
    }

    // Halfword and Signed Data Transfer (LDRH, STRH, LDRSB, LDRSH)
    if (instr & 0x0E00_0090) == 0x0000_0090 && ((instr >> 5) & 3) != 0 {
        let p = (instr & (1 << 24)) != 0;
        let u = (instr & (1 << 23)) != 0;
        let i = (instr & (1 << 22)) != 0;
        let w = (instr & (1 << 21)) != 0;
        let l = (instr & (1 << 20)) != 0;
        let rn = ((instr >> 16) & 0xF) as usize;
        let rd = ((instr >> 12) & 0xF) as usize;
        let sh = (instr >> 5) & 3;

        let offset = if i {
            (((instr >> 8) & 0xF) << 4) | (instr & 0xF)
        } else {
            cpu.reg((instr & 0xF) as usize)
        };

        let base = if rn == 15 { pc_val } else { cpu.reg(rn) };
        let offset_addr = if u { base.wrapping_add(offset) } else { base.wrapping_sub(offset) };
        let target_addr = if p { offset_addr } else { base };

        if l {
            let val = match sh {
                1 => bus.read16(target_addr & !1).rotate_right((target_addr & 1) * 8) as u32, // LDRH
                2 => (bus.read8(target_addr) as i8) as i32 as u32,                            // LDRSB
                3 => {
                    if (target_addr & 1) != 0 {
                        (bus.read8(target_addr) as i8) as i32 as u32
                    } else {
                        (bus.read16(target_addr) as i16) as i32 as u32
                    }
                } // LDRSH
                _ => 0,
            };
            cpu.set_reg(rd, val);
        } else {
            let val = if rd == 15 { pc_val } else { cpu.reg(rd) };
            bus.write16(target_addr & !1, val as u16);
        }

        if !l || rd != rn {
            if !p {
                cpu.set_reg(rn, offset_addr);
            } else if w {
                cpu.set_reg(rn, offset_addr);
            }
        }
        return 3;
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

        let offset = if !i {
            instr & 0xFFF
        } else {
            let rm = (instr & 0xF) as usize;
            let shift_type = ShiftType::from_u32((instr >> 5) & 3);
            let shift_amount = (instr >> 7) & 0x1F;
            let carry_in = (cpu.cpsr() & FLAG_C) != 0;
            let val = if rm == 15 { pc_val } else { cpu.reg(rm) };
            let (res, _) = barrel_shift(shift_type, val, shift_amount, carry_in, true);
            res
        };

        let base = if rn == 15 { pc_val } else { cpu.reg(rn) };
        let offset_addr = if u { base.wrapping_add(offset) } else { base.wrapping_sub(offset) };
        let target_addr = if p { offset_addr } else { base };

        if l {
            let val = if b {
                bus.read8(target_addr) as u32
            } else {
                bus.read32(target_addr & !3).rotate_right((target_addr & 3) * 8)
            };
            if rd == 15 {
                let v5 = cpu.is_armv5();
                branch_to(cpu, val, v5);
            } else {
                cpu.set_reg(rd, val);
            }
        } else {
            let val = if rd == 15 { pc_val.wrapping_add(4) } else { cpu.reg(rd) };
            if b {
                bus.write8(target_addr, val as u8);
            } else {
                bus.write32(target_addr & !3, val);
            }
        }

        if !l || rd != rn {
            if !p {
                cpu.set_reg(rn, offset_addr);
            } else if w {
                cpu.set_reg(rn, offset_addr);
            }
        }
        return 3;
    }

    // Block Data Transfer (LDM, STM)
    if (instr & 0x0E00_0000) == 0x0800_0000 {
        let p = (instr & (1 << 24)) != 0;
        let u = (instr & (1 << 23)) != 0;
        let s_bit = (instr & (1 << 22)) != 0;
        let w = (instr & (1 << 21)) != 0;
        let l = (instr & (1 << 20)) != 0;
        let rn = ((instr >> 16) & 0xF) as usize;
        let rlist = (instr & 0xFFFF) as u16;
        let armv5 = cpu.is_armv5();

        // Empty list: ARMv4 transfers r15 only; ARMv5 transfers nothing.
        // Both step the base by 0x40 (GBATEK "Strange effects").
        let empty = rlist == 0;
        let regs: u16 = if empty { if armv5 { 0 } else { 1 << 15 } } else { rlist };
        let span = if empty { 0x40 } else { rlist.count_ones() * 4 };
        let base = cpu.reg(rn);
        let mut addr = if u {
            if p { base.wrapping_add(4) } else { base }
        } else {
            let low = base.wrapping_sub(span);
            if p { low } else { low.wrapping_add(4) }
        };
        let new_base = if u { base.wrapping_add(span) } else { base.wrapping_sub(span) };

        // S bit without r15 in an LDM (or any STM): transfer the user bank.
        let pc_in_list = (regs & (1 << 15)) != 0;
        let user_bank = s_bit && !(l && pc_in_list);
        let saved_cpsr = cpu.cpsr();
        if user_bank {
            cpu.set_cpsr((saved_cpsr & !0x1F) | 0x1F); // System = user registers
        }

        if l {
            for reg in 0..16 {
                if (regs & (1 << reg)) != 0 {
                    let val = bus.read32(addr & !3);
                    if reg == 15 {
                        if s_bit {
                            // LDM ..., {.., r15}^: CPSR = SPSR, then jump.
                            let spsr = cpu.get_spsr();
                            cpu.set_cpsr(spsr);
                        }
                        branch_to(cpu, val, armv5 && !s_bit);
                    } else {
                        cpu.set_reg(reg, val);
                    }
                    addr = addr.wrapping_add(4);
                }
            }
        } else {
            let first = regs.trailing_zeros() as usize;
            for reg in 0..16 {
                if (regs & (1 << reg)) != 0 {
                    let val = if reg == 15 {
                        pc_val.wrapping_add(4)
                    } else if reg == rn && w && !armv5 && reg != first {
                        // ARMv4 stores the updated base unless it's first.
                        new_base
                    } else {
                        cpu.reg(reg)
                    };
                    bus.write32(addr & !3, val);
                    addr = addr.wrapping_add(4);
                }
            }
        }

        if user_bank {
            cpu.set_cpsr(saved_cpsr);
        }

        if w {
            let base_in_list = (regs & (1 << rn)) != 0;
            let writeback = if !l || !base_in_list {
                true
            } else if armv5 {
                // ARMv5: write back if the base is the only register or
                // not the last one; otherwise the loaded value stays.
                let last = 15 - regs.leading_zeros() as usize;
                regs.count_ones() == 1 || rn != last
            } else {
                false // ARMv4: the loaded value wins
            };
            if writeback {
                cpu.set_reg(rn, new_base);
            }
        }
        return rlist.count_ones().max(1) + 1;
    }

    // Data Processing (ALU)
    if (instr & 0x0C00_0000) == 0 {
        let i = (instr & (1 << 25)) != 0;
        let opcode = (instr >> 21) & 0xF;
        let s = (instr & (1 << 20)) != 0;
        let rn = ((instr >> 16) & 0xF) as usize;
        let rd = ((instr >> 12) & 0xF) as usize;

        let (operand2, shift_carry) = if i {
            let imm = instr & 0xFF;
            let rot = ((instr >> 8) & 0xF) * 2;
            if rot == 0 {
                (imm, (cpu.cpsr() & FLAG_C) != 0)
            } else {
                (imm.rotate_right(rot), (imm.rotate_right(rot) & 0x8000_0000) != 0)
            }
        } else {
            let rm = (instr & 0xF) as usize;
            let shift_type = ShiftType::from_u32((instr >> 5) & 3);
            let shift_by_reg = (instr & (1 << 4)) != 0;
            let carry_in = (cpu.cpsr() & FLAG_C) != 0;
            let val = if rm == 15 { pc_val } else { cpu.reg(rm) };

            let shift_amount = if shift_by_reg {
                let rs = ((instr >> 8) & 0xF) as usize;
                cpu.reg(rs) & 0xFF
            } else {
                (instr >> 7) & 0x1F
            };
            barrel_shift(shift_type, val, shift_amount, carry_in, !shift_by_reg)
        };

        let rn_val = if rn == 15 { pc_val } else { cpu.reg(rn) };
        let c_in = (cpu.cpsr() & FLAG_C) != 0;

        let (res, c_out, v_out) = match opcode {
            0 => (rn_val & operand2, shift_carry, false),                      // AND
            1 => (rn_val ^ operand2, shift_carry, false),                      // EOR
            2 => sub_with_borrow(rn_val, operand2, true),                      // SUB
            3 => sub_with_borrow(operand2, rn_val, true),                      // RSB
            4 => add_with_carry(rn_val, operand2, false),                      // ADD
            5 => add_with_carry(rn_val, operand2, c_in),                       // ADC
            6 => sub_with_borrow(rn_val, operand2, c_in),                      // SBC
            7 => sub_with_borrow(operand2, rn_val, c_in),                      // RSC
            8 => (rn_val & operand2, shift_carry, false),                      // TST
            9 => (rn_val ^ operand2, shift_carry, false),                      // TEQ
            10 => sub_with_borrow(rn_val, operand2, true),                     // CMP
            11 => add_with_carry(rn_val, operand2, false),                     // CMN
            12 => (rn_val | operand2, shift_carry, false),                     // ORR
            13 => (operand2, shift_carry, false),                              // MOV
            14 => (rn_val & !operand2, shift_carry, false),                     // BIC
            15 => (!operand2, shift_carry, false),                             // MVN
            _ => (0, false, false),
        };

        let is_test = matches!(opcode, 8..=11);
        if !is_test {
            cpu.set_reg(rd, res);
        }

        if s {
            if rd == 15 && !is_test {
                let spsr = cpu.get_spsr();
                cpu.set_cpsr(spsr);
            } else {
                let mut cpsr = cpu.cpsr() & !(FLAG_N | FLAG_Z);
                if (res & 0x8000_0000) != 0 { cpsr |= FLAG_N; }
                if res == 0 { cpsr |= FLAG_Z; }

                if matches!(opcode, 2..=7 | 10 | 11) {
                    cpsr &= !(FLAG_C | FLAG_V);
                    if c_out { cpsr |= FLAG_C; }
                    if v_out { cpsr |= FLAG_V; }
                } else {
                    cpsr &= !FLAG_C;
                    if c_out { cpsr |= FLAG_C; }
                }
                cpu.set_cpsr(cpsr);
            }
        }
        return 1;
    }

    1
}

// =========================================================================
// Thumb Execution (16-bit)
// =========================================================================

fn step_thumb<C: CpuState, B: CpuBus>(cpu: &mut C, bus: &mut B) -> u32 {
    let pc = cpu.reg(15);
    let instr = bus.read16(pc) as u32;
    let pc_val = pc.wrapping_add(4);
    cpu.set_reg(15, pc.wrapping_add(2));

    // Format 1: Move shifted register (LSL, LSR, ASR). op=3 is format 2.
    if (instr & 0xE000) == 0 && (instr & 0x1800) != 0x1800 {
        let op = (instr >> 11) & 3;
        let offset5 = (instr >> 6) & 0x1F;
        let rs = ((instr >> 3) & 7) as usize;
        let rd = (instr & 7) as usize;
        let shift_type = ShiftType::from_u32(op);
        let carry_in = (cpu.cpsr() & FLAG_C) != 0;
        let (res, c_out) = barrel_shift(shift_type, cpu.reg(rs), offset5, carry_in, true);
        cpu.set_reg(rd, res);
        set_nzc(cpu, res, c_out);
        return 1;
    }

    // Format 2: Add/subtract (reg/imm3)
    if (instr & 0xF800) == 0x1800 {
        let i = (instr & (1 << 10)) != 0;
        let op = (instr & (1 << 9)) != 0;
        let rn = ((instr >> 6) & 7) as usize;
        let rs = ((instr >> 3) & 7) as usize;
        let rd = (instr & 7) as usize;
        let operand = if i { rn as u32 } else { cpu.reg(rn) };
        let rs_val = cpu.reg(rs);

        let (res, c_out, v_out) = if op {
            sub_with_borrow(rs_val, operand, true)
        } else {
            add_with_carry(rs_val, operand, false)
        };
        cpu.set_reg(rd, res);
        set_nzcv(cpu, res, c_out, v_out);
        return 1;
    }

    // Format 3: Move/compare/add/subtract immediate (MOV, CMP, ADD, SUB imm8)
    if (instr & 0xE000) == 0x2000 {
        let op = (instr >> 11) & 3;
        let rd = ((instr >> 8) & 7) as usize;
        let imm = instr & 0xFF;
        let rd_val = cpu.reg(rd);

        match op {
            0 => { // MOV
                cpu.set_reg(rd, imm);
                set_nz(cpu, imm);
            }
            1 => { // CMP
                let (res, c_out, v_out) = sub_with_borrow(rd_val, imm, true);
                set_nzcv(cpu, res, c_out, v_out);
            }
            2 => { // ADD
                let (res, c_out, v_out) = add_with_carry(rd_val, imm, false);
                cpu.set_reg(rd, res);
                set_nzcv(cpu, res, c_out, v_out);
            }
            3 => { // SUB
                let (res, c_out, v_out) = sub_with_borrow(rd_val, imm, true);
                cpu.set_reg(rd, res);
                set_nzcv(cpu, res, c_out, v_out);
            }
            _ => {}
        }
        return 1;
    }

    // Format 4: ALU operations
    if (instr & 0xFC00) == 0x4000 {
        let op = (instr >> 6) & 0xF;
        let rs = ((instr >> 3) & 7) as usize;
        let rd = (instr & 7) as usize;
        let rd_val = cpu.reg(rd);
        let rs_val = cpu.reg(rs);
        let c_in = (cpu.cpsr() & FLAG_C) != 0;

        match op {
            0 => { let r = rd_val & rs_val; cpu.set_reg(rd, r); set_nz(cpu, r); } // AND
            1 => { let r = rd_val ^ rs_val; cpu.set_reg(rd, r); set_nz(cpu, r); } // EOR
            2 => { // LSL
                let (r, c) = barrel_shift(ShiftType::Lsl, rd_val, rs_val & 0xFF, c_in, false);
                cpu.set_reg(rd, r); set_nzc(cpu, r, c);
            }
            3 => { // LSR
                let (r, c) = barrel_shift(ShiftType::Lsr, rd_val, rs_val & 0xFF, c_in, false);
                cpu.set_reg(rd, r); set_nzc(cpu, r, c);
            }
            4 => { // ASR
                let (r, c) = barrel_shift(ShiftType::Asr, rd_val, rs_val & 0xFF, c_in, false);
                cpu.set_reg(rd, r); set_nzc(cpu, r, c);
            }
            5 => { // ADC
                let (r, c, v) = add_with_carry(rd_val, rs_val, c_in);
                cpu.set_reg(rd, r); set_nzcv(cpu, r, c, v);
            }
            6 => { // SBC
                let (r, c, v) = sub_with_borrow(rd_val, rs_val, c_in);
                cpu.set_reg(rd, r); set_nzcv(cpu, r, c, v);
            }
            7 => { // ROR
                let (r, c) = barrel_shift(ShiftType::Ror, rd_val, rs_val & 0xFF, c_in, false);
                cpu.set_reg(rd, r); set_nzc(cpu, r, c);
            }
            8 => { set_nz(cpu, rd_val & rs_val); } // TST
            9 => { // NEG
                let (r, c, v) = sub_with_borrow(0, rs_val, true);
                cpu.set_reg(rd, r); set_nzcv(cpu, r, c, v);
            }
            10 => { // CMP
                let (r, c, v) = sub_with_borrow(rd_val, rs_val, true);
                set_nzcv(cpu, r, c, v);
            }
            11 => { // CMN
                let (r, c, v) = add_with_carry(rd_val, rs_val, false);
                set_nzcv(cpu, r, c, v);
            }
            12 => { let r = rd_val | rs_val; cpu.set_reg(rd, r); set_nz(cpu, r); } // ORR
            13 => { let r = rd_val.wrapping_mul(rs_val); cpu.set_reg(rd, r); set_nz(cpu, r); } // MUL
            14 => { let r = rd_val & !rs_val; cpu.set_reg(rd, r); set_nz(cpu, r); } // BIC
            15 => { let r = !rs_val; cpu.set_reg(rd, r); set_nz(cpu, r); } // MVN
            _ => {}
        }
        return 1;
    }

    // Format 5: Hi register operations / branch exchange
    if (instr & 0xFC00) == 0x4400 {
        let op = (instr >> 8) & 3;
        let h1 = (instr & (1 << 7)) != 0;
        let h2 = (instr & (1 << 6)) != 0;
        let rs = (((instr >> 3) & 7) | if h2 { 8 } else { 0 }) as usize;
        let rd = ((instr & 7) | if h1 { 8 } else { 0 }) as usize;
        let rs_val = if rs == 15 { pc_val } else { cpu.reg(rs) };
        let rd_val = if rd == 15 { pc_val } else { cpu.reg(rd) };

        match op {
            0 => { // ADD
                let res = rd_val.wrapping_add(rs_val);
                cpu.set_reg(rd, if rd == 15 { res & !1 } else { res });
            }
            1 => { // CMP
                let (res, c, v) = sub_with_borrow(rd_val, rs_val, true);
                set_nzcv(cpu, res, c, v);
            }
            2 => { // MOV
                cpu.set_reg(rd, if rd == 15 { rs_val & !1 } else { rs_val });
            }
            3 => { // BX / BLX
                let is_blx = cpu.is_armv5() && (instr & (1 << 7)) != 0;
                if is_blx {
                    cpu.set_reg(14, (pc.wrapping_add(2)) | 1);
                }
                if (rs_val & 1) != 0 {
                    cpu.set_cpsr(cpu.cpsr() | FLAG_T);
                    cpu.set_reg(15, rs_val & !1);
                } else {
                    cpu.set_cpsr(cpu.cpsr() & !FLAG_T);
                    cpu.set_reg(15, rs_val & !3);
                }
            }
            _ => {}
        }
        return 2;
    }

    // Format 6: PC-relative load
    if (instr & 0xF800) == 0x4800 {
        let rd = ((instr >> 8) & 7) as usize;
        let word8 = (instr & 0xFF) * 4;
        let addr = (pc_val & !2).wrapping_add(word8);
        cpu.set_reg(rd, bus.read32(addr));
        return 2;
    }

    // Format 7: Load/store with register offset (LDR, LDRB, STR, STRB)
    if (instr & 0xF200) == 0x5000 {
        let l = (instr & (1 << 11)) != 0;
        let b = (instr & (1 << 10)) != 0;
        let ro = ((instr >> 6) & 7) as usize;
        let rb = ((instr >> 3) & 7) as usize;
        let rd = (instr & 7) as usize;
        let addr = cpu.reg(rb).wrapping_add(cpu.reg(ro));

        if l {
            let val = if b {
                bus.read8(addr) as u32
            } else {
                bus.read32(addr & !3).rotate_right((addr & 3) * 8)
            };
            cpu.set_reg(rd, val);
        } else {
            let val = cpu.reg(rd);
            if b { bus.write8(addr, val as u8); } else { bus.write32(addr & !3, val); }
        }
        return 2;
    }

    // Format 8: Load/store sign-extended byte/halfword (STRH, LDSB, LDRH, LDSH)
    if (instr & 0xF200) == 0x5200 {
        let h = (instr & (1 << 11)) != 0;
        let s = (instr & (1 << 10)) != 0;
        let ro = ((instr >> 6) & 7) as usize;
        let rb = ((instr >> 3) & 7) as usize;
        let rd = (instr & 7) as usize;
        let addr = cpu.reg(rb).wrapping_add(cpu.reg(ro));

        match (s, h) {
            (false, false) => bus.write16(addr & !1, cpu.reg(rd) as u16), // STRH
            (false, true) => { // LDRH
                let val = bus.read16(addr & !1).rotate_right((addr & 1) * 8);
                cpu.set_reg(rd, val as u32);
            }
            (true, false) => { // LDSB
                let val = (bus.read8(addr) as i8) as i32 as u32;
                cpu.set_reg(rd, val);
            }
            (true, true) => { // LDSH
                let val = if (addr & 1) != 0 {
                    (bus.read8(addr) as i8) as i32 as u32
                } else {
                    (bus.read16(addr) as i16) as i32 as u32
                };
                cpu.set_reg(rd, val);
            }
        }
        return 2;
    }

    // Format 9: Load/store with immediate offset (LDR, STR, LDRB, STRB)
    if (instr & 0xE000) == 0x6000 {
        let b = (instr & (1 << 12)) != 0;
        let l = (instr & (1 << 11)) != 0;
        let offset = ((instr >> 6) & 0x1F) << if b { 0 } else { 2 };
        let rb = ((instr >> 3) & 7) as usize;
        let rd = (instr & 7) as usize;
        let addr = cpu.reg(rb).wrapping_add(offset);

        if l {
            let val = if b {
                bus.read8(addr) as u32
            } else {
                bus.read32(addr & !3).rotate_right((addr & 3) * 8)
            };
            cpu.set_reg(rd, val);
        } else {
            let val = cpu.reg(rd);
            if b { bus.write8(addr, val as u8); } else { bus.write32(addr & !3, val); }
        }
        return 2;
    }

    // Format 10: Load/store halfword with immediate offset (LDRH, STRH)
    if (instr & 0xF000) == 0x8000 {
        let l = (instr & (1 << 11)) != 0;
        let offset = ((instr >> 6) & 0x1F) << 1;
        let rb = ((instr >> 3) & 7) as usize;
        let rd = (instr & 7) as usize;
        let addr = cpu.reg(rb).wrapping_add(offset);

        if l {
            let val = bus.read16(addr & !1).rotate_right((addr & 1) * 8);
            cpu.set_reg(rd, val as u32);
        } else {
            bus.write16(addr & !1, cpu.reg(rd) as u16);
        }
        return 2;
    }

    // Format 11: SP-relative load/store
    if (instr & 0xF000) == 0x9000 {
        let l = (instr & (1 << 11)) != 0;
        let rd = ((instr >> 8) & 7) as usize;
        let word8 = (instr & 0xFF) * 4;
        let addr = cpu.reg(13).wrapping_add(word8);

        if l {
            cpu.set_reg(rd, bus.read32(addr & !3));
        } else {
            bus.write32(addr & !3, cpu.reg(rd));
        }
        return 2;
    }

    // Format 12: Load address (ADD Rd, PC/SP, #imm)
    if (instr & 0xF000) == 0xA000 {
        let sp = (instr & (1 << 11)) != 0;
        let rd = ((instr >> 8) & 7) as usize;
        let word8 = (instr & 0xFF) * 4;
        let base = if sp { cpu.reg(13) } else { pc_val & !2 };
        cpu.set_reg(rd, base.wrapping_add(word8));
        return 1;
    }

    // Format 13: Add offset to Stack Pointer
    if (instr & 0xFF00) == 0xB000 {
        let s = (instr & (1 << 7)) != 0;
        let word7 = (instr & 0x7F) * 4;
        let sp = cpu.reg(13);
        cpu.set_reg(13, if s { sp.wrapping_sub(word7) } else { sp.wrapping_add(word7) });
        return 1;
    }

    // Format 14: Push/pop registers
    if (instr & 0xF600) == 0xB400 {
        let l = (instr & (1 << 11)) != 0;
        let r = (instr & (1 << 8)) != 0;
        let rlist = instr & 0xFF;

        let num_regs = rlist.count_ones() + if r { 1 } else { 0 };
        let total_bytes = num_regs * 4;

        if l {
            let mut addr = cpu.reg(13);
            for reg in 0..8 {
                if (rlist & (1 << reg)) != 0 {
                    cpu.set_reg(reg, bus.read32(addr));
                    addr = addr.wrapping_add(4);
                }
            }
            if r {
                let target = bus.read32(addr);
                addr = addr.wrapping_add(4);
                if cpu.is_armv5() {
                    branch_to(cpu, target, true);
                } else {
                    cpu.set_reg(15, target & !1);
                }
            }
            cpu.set_reg(13, addr);
        } else {
            let mut addr = cpu.reg(13).wrapping_sub(total_bytes);
            let final_sp = addr;
            for reg in 0..8 {
                if (rlist & (1 << reg)) != 0 {
                    bus.write32(addr, cpu.reg(reg));
                    addr = addr.wrapping_add(4);
                }
            }
            if r {
                bus.write32(addr, cpu.reg(14));
            }
            cpu.set_reg(13, final_sp);
        }
        return num_regs + 1;
    }

    // Format 15: Multiple load/store (LDMIA, STMIA)
    if (instr & 0xF000) == 0xC000 {
        let l = (instr & (1 << 11)) != 0;
        let rb = ((instr >> 8) & 7) as usize;
        let rlist = instr & 0xFF;
        let mut addr = cpu.reg(rb);

        if l {
            for reg in 0..8 {
                if (rlist & (1 << reg)) != 0 {
                    cpu.set_reg(reg, bus.read32(addr));
                    addr = addr.wrapping_add(4);
                }
            }
        } else {
            for reg in 0..8 {
                if (rlist & (1 << reg)) != 0 {
                    bus.write32(addr, cpu.reg(reg));
                    addr = addr.wrapping_add(4);
                }
            }
        }
        cpu.set_reg(rb, addr);
        return rlist.count_ones() + 1;
    }

    // Format 16: Conditional branch
    if (instr & 0xF000) == 0xD000 {
        let cond = (instr >> 8) & 0xF;
        if cond == 0xF {
            // Format 17: Software Interrupt (SWI)
            let comment = instr & 0xFF;
            if !crate::nds::bios::hle_swi(cpu, bus, comment, true) {
                cpu.trigger_swi(comment);
            }
            return 3;
        }

        if cpu.check_condition(cond) {
            let offset = (((instr & 0xFF) as i8) as i32) * 2;
            cpu.set_reg(15, pc_val.wrapping_add(offset as u32));
            return 3;
        }
        return 1;
    }

    // Format 19 suffix, BLX label (ARMv5): 11101 offset11
    if cpu.is_armv5() && (instr & 0xF800) == 0xE800 {
        let lr = cpu.reg(14);
        cpu.set_reg(14, pc.wrapping_add(2) | 1);
        cpu.set_cpsr(cpu.cpsr() & !FLAG_T);
        cpu.set_reg(15, lr.wrapping_add((instr & 0x7FF) << 1) & !3);
        return 3;
    }

    // Format 18: Unconditional branch
    if (instr & 0xF800) == 0xE000 {
        let offset = (((instr & 0x7FF) as i32) << 21 >> 20) as u32;
        cpu.set_reg(15, pc_val.wrapping_add(offset));
        return 3;
    }

    // Format 19: Long branch with link (BL / BLX)
    if (instr & 0xF000) == 0xF000 {
        let h = (instr >> 11) & 3;
        let offset11 = instr & 0x7FF;

        if h == 2 {
            // First instruction (prefix)
            let offset = ((offset11 as i32) << 21 >> 9) as u32;
            cpu.set_reg(14, pc_val.wrapping_add(offset));
        } else if h == 3 {
            // Second instruction (BL suffix)
            let lr = cpu.reg(14);
            let next_pc = pc.wrapping_add(2);
            cpu.set_reg(14, next_pc | 1);
            cpu.set_reg(15, lr.wrapping_add(offset11 << 1));
        } else if h == 1 && cpu.is_armv5() {
            // Second instruction (BLX suffix on ARMv5)
            let lr = cpu.reg(14);
            let next_pc = pc.wrapping_add(2);
            cpu.set_reg(14, next_pc | 1);
            cpu.set_cpsr(cpu.cpsr() & !FLAG_T);
            cpu.set_reg(15, (lr.wrapping_add(offset11 << 1)) & !3);
        }
        return 2;
    }

    1
}

// =========================================================================
// HLE Software Interrupts & Helpers
// =========================================================================

/// Jump to `target` after a load into r15. With `interwork` (ARMv5 LDR/LDM/POP)
/// bit 0 selects Thumb; otherwise the current state is kept.
fn branch_to<C: CpuState>(cpu: &mut C, target: u32, interwork: bool) {
    if interwork {
        if (target & 1) != 0 {
            cpu.set_cpsr(cpu.cpsr() | FLAG_T);
            cpu.set_reg(15, target & !1);
        } else {
            cpu.set_cpsr(cpu.cpsr() & !FLAG_T);
            cpu.set_reg(15, target & !3);
        }
    } else if cpu.is_thumb() {
        cpu.set_reg(15, target & !1);
    } else {
        cpu.set_reg(15, target & !3);
    }
}

#[inline(always)]
fn sat_add<C: CpuState>(a: i32, b: i32, cpu: &mut C) -> i32 {
    let (res, ov) = a.overflowing_add(b);
    if ov {
        cpu.set_cpsr(cpu.cpsr() | FLAG_Q);
        if a < 0 { i32::MIN } else { i32::MAX }
    } else {
        res
    }
}

#[inline(always)]
fn sat_sub<C: CpuState>(a: i32, b: i32, cpu: &mut C) -> i32 {
    let (res, ov) = a.overflowing_sub(b);
    if ov {
        cpu.set_cpsr(cpu.cpsr() | FLAG_Q);
        if a < 0 { i32::MIN } else { i32::MAX }
    } else {
        res
    }
}

#[inline(always)]
fn sat_double<C: CpuState>(v: i32, cpu: &mut C) -> i32 {
    let (res, ov) = v.overflowing_add(v);
    if ov {
        cpu.set_cpsr(cpu.cpsr() | FLAG_Q);
        if v < 0 { i32::MIN } else { i32::MAX }
    } else {
        res
    }
}

#[inline(always)]
fn set_nz<C: CpuState>(cpu: &mut C, val: u32) {
    let mut cpsr = cpu.cpsr() & !(FLAG_N | FLAG_Z);
    if (val & 0x8000_0000) != 0 { cpsr |= FLAG_N; }
    if val == 0 { cpsr |= FLAG_Z; }
    cpu.set_cpsr(cpsr);
}

#[inline(always)]
fn set_nzc<C: CpuState>(cpu: &mut C, val: u32, carry: bool) {
    let mut cpsr = cpu.cpsr() & !(FLAG_N | FLAG_Z | FLAG_C);
    if (val & 0x8000_0000) != 0 { cpsr |= FLAG_N; }
    if val == 0 { cpsr |= FLAG_Z; }
    if carry { cpsr |= FLAG_C; }
    cpu.set_cpsr(cpsr);
}

#[inline(always)]
fn set_nzcv<C: CpuState>(cpu: &mut C, val: u32, carry: bool, overflow: bool) {
    let mut cpsr = cpu.cpsr() & !(FLAG_N | FLAG_Z | FLAG_C | FLAG_V);
    if (val & 0x8000_0000) != 0 { cpsr |= FLAG_N; }
    if val == 0 { cpsr |= FLAG_Z; }
    if carry { cpsr |= FLAG_C; }
    if overflow { cpsr |= FLAG_V; }
    cpu.set_cpsr(cpsr);
}
