//! THUMB 16-bit Instruction Decoder & Execution

use super::alu::{add_with_carry, barrel_shift, sub_with_borrow, ShiftType};
use super::{Arm7Tdmi, FLAG_C, FLAG_N, FLAG_T, FLAG_V, FLAG_Z};
use crate::gba::mmu::Mmu;

pub fn step_thumb(cpu: &mut Arm7Tdmi, mmu: &mut Mmu) -> u32 {
    let pc = cpu.regs[15];
    let instr = mmu.read16(pc);
    cpu.regs[15] = pc.wrapping_add(2);

    // Format 19: Long Branch with Link (BL)
    if (instr & 0xF800) == 0xF000 {
        // First half: setup upper 11 bits in LR (sign extended 11-bit offset)
        let offset = ((((instr & 0x07FF) as i16) << 5 >> 5) as i32) << 12;
        cpu.regs[14] = pc.wrapping_add(4).wrapping_add(offset as u32);
        return 1;
    }
    if (instr & 0xF800) == 0xF800 {
        // Second half: complete branch
        let offset = ((instr & 0x07FF) as u32) << 1;
        let target = cpu.regs[14].wrapping_add(offset);
        cpu.regs[14] = pc.wrapping_add(2) | 1;
        cpu.regs[15] = target & !1;
        return 3;
    }

    // Format 18: Unconditional Branch (B)
    if (instr & 0xF800) == 0xE000 {
        let offset = (((instr & 0x07FF) as i16) << 5 >> 4) as i32; // Sign-extend 11 bits and * 2
        cpu.regs[15] = pc.wrapping_add(4).wrapping_add(offset as u32);
        return 3;
    }

    // Format 17: Software Interrupt (SWI)
    if (instr & 0xFF00) == 0xDF00 {
        let comment = (instr & 0xFF) as u32;
        mmu.handle_swi(cpu, comment);
        return 3;
    }

    // Format 16: Conditional Branch (B<cond>)
    if (instr & 0xF000) == 0xD000 {
        let cond = ((instr >> 8) & 0xF) as u32;
        if cond < 14 {
            if cpu.check_condition(cond) {
                let offset = (((instr & 0xFF) as i8) as i32) << 1;
                cpu.regs[15] = pc.wrapping_add(4).wrapping_add(offset as u32);
                return 3;
            }
            return 1;
        }
    }

    // Format 15: Multiple Load/Store (LDMIA, STMIA)
    if (instr & 0xF000) == 0xC000 {
        let l = (instr & (1 << 11)) != 0;
        let rb = ((instr >> 8) & 7) as usize;
        let reg_list = (instr & 0xFF) as u8;

        let mut addr = cpu.regs[rb];
        let num_regs = reg_list.count_ones();

        for r in 0..8 {
            if (reg_list & (1 << r)) != 0 {
                if l {
                    cpu.regs[r] = mmu.read32(addr);
                } else {
                    mmu.write32(addr, cpu.regs[r]);
                }
                addr = addr.wrapping_add(4);
            }
        }

        if !l || (reg_list & (1 << rb)) == 0 {
            cpu.regs[rb] = addr;
        }
        return num_regs + 2;
    }

    // Format 14: Push/Pop Registers (PUSH, POP)
    if (instr & 0xF600) == 0xB400 {
        let l = (instr & (1 << 11)) != 0;
        let r_bit = (instr & (1 << 8)) != 0;
        let reg_list = (instr & 0xFF) as u8;

        if !l {
            // PUSH
            let num = reg_list.count_ones() + if r_bit { 1 } else { 0 };
            let mut sp = cpu.regs[13].wrapping_sub(num * 4);
            cpu.regs[13] = sp;

            for r in 0..8 {
                if (reg_list & (1 << r)) != 0 {
                    mmu.write32(sp, cpu.regs[r]);
                    sp = sp.wrapping_add(4);
                }
            }
            if r_bit {
                mmu.write32(sp, cpu.regs[14]); // Store LR
            }
            return num + 2;
        } else {
            // POP
            let mut sp = cpu.regs[13];
            let num = reg_list.count_ones() + if r_bit { 1 } else { 0 };

            for r in 0..8 {
                if (reg_list & (1 << r)) != 0 {
                    cpu.regs[r] = mmu.read32(sp);
                    sp = sp.wrapping_add(4);
                }
            }
            if r_bit {
                let target = mmu.read32(sp);
                sp = sp.wrapping_add(4);
                cpu.regs[15] = target & !1;
            }
            cpu.regs[13] = sp;
            return num + (if r_bit { 3 } else { 2 });
        }
    }

    // Format 13: Add Offset to Stack Pointer (ADD SP, #±imm)
    if (instr & 0xFF00) == 0xB000 {
        let s = (instr & (1 << 7)) != 0;
        let offset = ((instr & 0x7F) as u32) << 2;
        if s {
            cpu.regs[13] = cpu.regs[13].wrapping_sub(offset);
        } else {
            cpu.regs[13] = cpu.regs[13].wrapping_add(offset);
        }
        return 1;
    }

    // Format 12: Load Address (ADD Rd, PC/SP, #imm)
    if (instr & 0xF000) == 0xA000 {
        let is_sp = (instr & (1 << 11)) != 0;
        let rd = ((instr >> 8) & 7) as usize;
        let offset = ((instr & 0xFF) as u32) << 2;
        let base = if is_sp { cpu.regs[13] } else { (pc.wrapping_add(4)) & !2 };
        cpu.regs[rd] = base.wrapping_add(offset);
        return 1;
    }

    // Format 11: SP-Relative Load/Store (LDR, STR Rd, [SP, #imm])
    if (instr & 0xF000) == 0x9000 {
        let l = (instr & (1 << 11)) != 0;
        let rd = ((instr >> 8) & 7) as usize;
        let addr = cpu.regs[13].wrapping_add(((instr & 0xFF) as u32) << 2);
        if l {
            cpu.regs[rd] = mmu.read32(addr);
        } else {
            mmu.write32(addr, cpu.regs[rd]);
        }
        return if l { 2 } else { 2 };
    }

    // Format 10: Load/Store Halfword (LDRH, STRH Rd, [Rb, #imm])
    if (instr & 0xF000) == 0x8000 {
        let l = (instr & (1 << 11)) != 0;
        let offset = (((instr >> 6) & 0x1F) as u32) << 1;
        let rb = ((instr >> 3) & 7) as usize;
        let rd = (instr & 7) as usize;
        let addr = cpu.regs[rb].wrapping_add(offset);
        if l {
            cpu.regs[rd] = mmu.read16(addr) as u32;
        } else {
            mmu.write16(addr, (cpu.regs[rd] & 0xFFFF) as u16);
        }
        return 2;
    }

    // Format 9: Load/Store with Immediate Offset (STR, LDR, STRB, LDRB)
    if (instr & 0xE000) == 0x6000 {
        let b = (instr & (1 << 12)) != 0;
        let l = (instr & (1 << 11)) != 0;
        let offset = if b {
            ((instr >> 6) & 0x1F) as u32
        } else {
            (((instr >> 6) & 0x1F) as u32) << 2
        };
        let rb = ((instr >> 3) & 7) as usize;
        let rd = (instr & 7) as usize;
        let addr = cpu.regs[rb].wrapping_add(offset);

        if l {
            cpu.regs[rd] = if b { mmu.read8(addr) as u32 } else { mmu.read32(addr) };
        } else {
            let val = cpu.regs[rd];
            if b {
                mmu.write8(addr, (val & 0xFF) as u8);
            } else {
                mmu.write32(addr, val);
            }
        }
        return 2;
    }

    // Format 8: Load/Store Sign-Extended Byte/Halfword
    if (instr & 0xF200) == 0x5200 {
        let h = (instr & (1 << 11)) != 0;
        let s = (instr & (1 << 10)) != 0;
        let ro = ((instr >> 6) & 7) as usize;
        let rb = ((instr >> 3) & 7) as usize;
        let rd = (instr & 7) as usize;
        let addr = cpu.regs[rb].wrapping_add(cpu.regs[ro]);

        match (s, h) {
            (false, false) => mmu.write16(addr, (cpu.regs[rd] & 0xFFFF) as u16), // STRH
            (false, true) => cpu.regs[rd] = mmu.read16(addr) as u32,             // LDRH
            (true, false) => cpu.regs[rd] = (mmu.read8(addr) as i8) as i32 as u32, // LDSB
            (true, true) => cpu.regs[rd] = (mmu.read16(addr) as i16) as i32 as u32, // LDSH
        }
        return 2;
    }

    // Format 7: Load/Store with Register Offset
    if (instr & 0xF200) == 0x5000 {
        let l = (instr & (1 << 11)) != 0;
        let b = (instr & (1 << 10)) != 0;
        let ro = ((instr >> 6) & 7) as usize;
        let rb = ((instr >> 3) & 7) as usize;
        let rd = (instr & 7) as usize;
        let addr = cpu.regs[rb].wrapping_add(cpu.regs[ro]);

        if l {
            cpu.regs[rd] = if b { mmu.read8(addr) as u32 } else { mmu.read32(addr) };
        } else {
            let val = cpu.regs[rd];
            if b {
                mmu.write8(addr, (val & 0xFF) as u8);
            } else {
                mmu.write32(addr, val);
            }
        }
        return 2;
    }

    // Format 6: PC-Relative Load (LDR Rd, [PC, #imm])
    if (instr & 0xF800) == 0x4800 {
        let rd = ((instr >> 8) & 7) as usize;
        let offset = ((instr & 0xFF) as u32) << 2;
        let addr = (pc.wrapping_add(4) & !2).wrapping_add(offset);
        cpu.regs[rd] = mmu.read32(addr);
        return 2;
    }

    // Format 5: Hi Register Operations / Branch Exchange
    if (instr & 0xFC00) == 0x4400 {
        let op = (instr >> 8) & 3;
        let h1 = (instr & (1 << 7)) != 0;
        let h2 = (instr & (1 << 6)) != 0;
        let rs = (((instr >> 3) & 7) | if h2 { 8 } else { 0 }) as usize;
        let rd = ((instr & 7) | if h1 { 8 } else { 0 }) as usize;

        let op1 = if rd == 15 { pc.wrapping_add(4) } else { cpu.regs[rd] };
        let op2 = if rs == 15 { pc.wrapping_add(4) } else { cpu.regs[rs] };

        match op {
            0 => {
                // ADD
                let res = op1.wrapping_add(op2);
                if rd == 15 {
                    cpu.regs[15] = res & !1;
                    return 3;
                } else {
                    cpu.regs[rd] = res;
                }
            }
            1 => {
                // CMP
                let (res, c, v) = sub_with_borrow(op1, op2, true);
                cpu.set_flag(FLAG_N, (res & 0x8000_0000) != 0);
                cpu.set_flag(FLAG_Z, res == 0);
                cpu.set_flag(FLAG_C, c);
                cpu.set_flag(FLAG_V, v);
            }
            2 => {
                // MOV
                if rd == 15 {
                    cpu.regs[15] = op2 & !1;
                    return 3;
                } else {
                    cpu.regs[rd] = op2;
                }
            }
            3 => {
                // BX
                if (op2 & 1) != 0 {
                    cpu.regs[15] = op2 & !1;
                } else {
                    cpu.set_flag(FLAG_T, false);
                    cpu.regs[15] = op2 & !3;
                }
                return 3;
            }
            _ => {}
        }
        return 1;
    }

    // Format 4: ALU Operations
    if (instr & 0xFC00) == 0x4000 {
        let op = (instr >> 6) & 0xF;
        let rs = ((instr >> 3) & 7) as usize;
        let rd = (instr & 7) as usize;

        let op1 = cpu.regs[rd];
        let op2 = cpu.regs[rs];
        let mut carry_out = cpu.get_flag(FLAG_C);
        let mut overflow = cpu.get_flag(FLAG_V);

        let res = match op {
            0x0 => op1 & op2, // AND
            0x1 => op1 ^ op2, // EOR
            0x2 => {          // LSL
                let (r, c) = barrel_shift(ShiftType::Lsl, op1, op2 & 0xFF, carry_out, false);
                carry_out = c;
                r
            }
            0x3 => {          // LSR
                let (r, c) = barrel_shift(ShiftType::Lsr, op1, op2 & 0xFF, carry_out, false);
                carry_out = c;
                r
            }
            0x4 => {          // ASR
                let (r, c) = barrel_shift(ShiftType::Asr, op1, op2 & 0xFF, carry_out, false);
                carry_out = c;
                r
            }
            0x5 => {          // ADC
                let (r, c, v) = add_with_carry(op1, op2, cpu.get_flag(FLAG_C));
                carry_out = c;
                overflow = v;
                r
            }
            0x6 => {          // SBC
                let (r, c, v) = sub_with_borrow(op1, op2, cpu.get_flag(FLAG_C));
                carry_out = c;
                overflow = v;
                r
            }
            0x7 => {          // ROR
                let (r, c) = barrel_shift(ShiftType::Ror, op1, op2 & 0xFF, carry_out, false);
                carry_out = c;
                r
            }
            0x8 => op1 & op2, // TST
            0x9 => {          // NEG
                let (r, c, v) = sub_with_borrow(0, op2, true);
                carry_out = c;
                overflow = v;
                r
            }
            0xA => {          // CMP
                let (r, c, v) = sub_with_borrow(op1, op2, true);
                carry_out = c;
                overflow = v;
                r
            }
            0xB => {          // CMN
                let (r, c, v) = add_with_carry(op1, op2, false);
                carry_out = c;
                overflow = v;
                r
            }
            0xC => op1 | op2, // ORR
            0xD => op1.wrapping_mul(op2), // MUL
            0xE => op1 & !op2, // BIC
            0xF => !op2,       // MVN
            _ => 0,
        };

        if op != 0x8 && op != 0xA && op != 0xB {
            cpu.regs[rd] = res;
        }

        cpu.set_flag(FLAG_N, (res & 0x8000_0000) != 0);
        cpu.set_flag(FLAG_Z, res == 0);
        cpu.set_flag(FLAG_C, carry_out);
        if op == 0x5 || op == 0x6 || op == 0x9 || op == 0xA || op == 0xB {
            cpu.set_flag(FLAG_V, overflow);
        }
        return 1;
    }

    // Format 3: Move/Compare/Add/Subtract Immediate
    if (instr & 0xE000) == 0x2000 {
        let op = (instr >> 11) & 3;
        let rd = ((instr >> 8) & 7) as usize;
        let imm = (instr & 0xFF) as u32;
        let op1 = cpu.regs[rd];

        match op {
            0 => {
                // MOV
                cpu.regs[rd] = imm;
                cpu.set_flag(FLAG_N, false);
                cpu.set_flag(FLAG_Z, imm == 0);
            }
            1 => {
                // CMP
                let (res, c, v) = sub_with_borrow(op1, imm, true);
                cpu.set_flag(FLAG_N, (res & 0x8000_0000) != 0);
                cpu.set_flag(FLAG_Z, res == 0);
                cpu.set_flag(FLAG_C, c);
                cpu.set_flag(FLAG_V, v);
            }
            2 => {
                // ADD
                let (res, c, v) = add_with_carry(op1, imm, false);
                cpu.regs[rd] = res;
                cpu.set_flag(FLAG_N, (res & 0x8000_0000) != 0);
                cpu.set_flag(FLAG_Z, res == 0);
                cpu.set_flag(FLAG_C, c);
                cpu.set_flag(FLAG_V, v);
            }
            3 => {
                // SUB
                let (res, c, v) = sub_with_borrow(op1, imm, true);
                cpu.regs[rd] = res;
                cpu.set_flag(FLAG_N, (res & 0x8000_0000) != 0);
                cpu.set_flag(FLAG_Z, res == 0);
                cpu.set_flag(FLAG_C, c);
                cpu.set_flag(FLAG_V, v);
            }
            _ => {}
        }
        return 1;
    }

    // Format 2: Add/Subtract
    if (instr & 0xF800) == 0x1800 {
        let i = (instr & (1 << 10)) != 0;
        let op_sub = (instr & (1 << 9)) != 0;
        let rn_imm = ((instr >> 6) & 7) as u32;
        let rs = ((instr >> 3) & 7) as usize;
        let rd = (instr & 7) as usize;

        let op1 = cpu.regs[rs];
        let op2 = if i { rn_imm } else { cpu.regs[rn_imm as usize] };

        let (res, c, v) = if op_sub {
            sub_with_borrow(op1, op2, true)
        } else {
            add_with_carry(op1, op2, false)
        };

        cpu.regs[rd] = res;
        cpu.set_flag(FLAG_N, (res & 0x8000_0000) != 0);
        cpu.set_flag(FLAG_Z, res == 0);
        cpu.set_flag(FLAG_C, c);
        cpu.set_flag(FLAG_V, v);
        return 1;
    }

    // Format 1: Move Shifted Register
    if (instr & 0xE000) == 0x0000 {
        let op = (instr >> 11) & 3;
        let offset = ((instr >> 6) & 0x1F) as u32;
        let rs = ((instr >> 3) & 7) as usize;
        let rd = (instr & 7) as usize;

        let shift_type = ShiftType::from_u32(op as u32);
        let val = cpu.regs[rs];
        let (res, carry) = barrel_shift(shift_type, val, offset, cpu.get_flag(FLAG_C), true);

        cpu.regs[rd] = res;
        cpu.set_flag(FLAG_N, (res & 0x8000_0000) != 0);
        cpu.set_flag(FLAG_Z, res == 0);
        if op != 0 || offset != 0 {
            cpu.set_flag(FLAG_C, carry);
        }
        return 1;
    }

    1
}
