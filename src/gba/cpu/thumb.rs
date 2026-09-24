//! THUMB 16-bit Instruction Decoder & Execution

use super::alu::{add_with_carry, barrel_shift, sub_with_borrow, ShiftType};
use super::{Arm7Tdmi, FLAG_C, FLAG_N, FLAG_T, FLAG_V, FLAG_Z};
use crate::gba::mmu::Mmu;

/// THUMB open bus depends on the region the code runs from (GBATEK
/// "Unpredictable Things"): 16-bit buses repeat the fetched halfword, while
/// BIOS/OAM and 32-bit IWRAM combine it with the neighbouring one.
fn thumb_open_bus(pc: u32, next: u32, fetched: u32) -> u32 {
    let (next, fetched) = (next & 0xFFFF, fetched & 0xFFFF);
    match pc >> 24 {
        // BIOS, OAM: [$+6]:[$+4]; approximated by the prefetched pair.
        0x00 | 0x07 => fetched << 16 | next,
        // IWRAM (32-bit bus): the aligned word containing $+4.
        0x03 => {
            if pc & 2 == 0 { next << 16 | fetched } else { fetched << 16 | next }
        }
        // EWRAM, palette, VRAM, ROM: 16-bit buses.
        _ => fetched << 16 | fetched,
    }
}

/// Execute one THUMB instruction through the two-stage prefetch pipeline
/// (see `step_arm`).
pub fn step_thumb(cpu: &mut Arm7Tdmi, mmu: &mut Mmu) -> u32 {
    let pc = cpu.regs[15];
    let waits_before = mmu.timing.waits.get();
    let (instr, next) = if cpu.pipe_valid && cpu.pipe_addr == pc {
        (cpu.pipe[0] as u16, cpu.pipe[1])
    } else {
        (mmu.fetch16(pc), mmu.fetch16(pc.wrapping_add(2)) as u32)
    };
    let fetched = mmu.fetch16(pc.wrapping_add(4)) as u32;
    mmu.open_bus = thumb_open_bus(pc, next, fetched);
    let cycles = execute_thumb(cpu, mmu, instr);
    if cpu.regs[15] == pc.wrapping_add(2) && cpu.is_thumb() {
        cpu.pipe = [next, fetched];
        cpu.pipe_addr = cpu.regs[15];
        cpu.pipe_valid = true;
    } else {
        cpu.pipe_valid = false;
    }
    // Base cycles (1 per access + internal) plus memory wait states.
    cycles + mmu.timing.waits.get().wrapping_sub(waits_before)
}

/// Thumb instruction formats, in decode-priority order (see `THUMB_FORMAT`).
const FMT_BL_HI: u8 = 0;
const FMT_BL_LO: u8 = 1;
const FMT_B: u8 = 2;
const FMT_SWI: u8 = 3;
const FMT_B_COND: u8 = 4;
const FMT_LDM_STM: u8 = 5;
const FMT_PUSH_POP: u8 = 6;
const FMT_ADD_SP: u8 = 7;
const FMT_LOAD_ADDR: u8 = 8;
const FMT_SP_LDR_STR: u8 = 9;
const FMT_LDRH_STRH_IMM: u8 = 10;
const FMT_LDR_STR_IMM: u8 = 11;
const FMT_LDR_STR_SIGNED: u8 = 12;
const FMT_LDR_STR_REG: u8 = 13;
const FMT_PC_LDR: u8 = 14;
const FMT_HI_REG_BX: u8 = 15;
const FMT_ALU: u8 = 16;
const FMT_IMM_OPS: u8 = 17;
const FMT_ADD_SUB: u8 = 18;
const FMT_SHIFT: u8 = 19;
const THUMB_NONE: u8 = 20;

/// Format of every Thumb instruction, indexed by its top 10 bits (every
/// format mask lives within bits 15..6). First matching test wins, exactly
/// like the if-chain it replaced.
static THUMB_FORMAT: [u8; 1024] = {
    const TESTS: [(u16, u16, u8); 20] = [
        (0xF800, 0xF000, FMT_BL_HI),
        (0xF800, 0xF800, FMT_BL_LO),
        (0xF800, 0xE000, FMT_B),
        (0xFF00, 0xDF00, FMT_SWI),
        (0xF000, 0xD000, FMT_B_COND),
        (0xF000, 0xC000, FMT_LDM_STM),
        (0xF600, 0xB400, FMT_PUSH_POP),
        (0xFF00, 0xB000, FMT_ADD_SP),
        (0xF000, 0xA000, FMT_LOAD_ADDR),
        (0xF000, 0x9000, FMT_SP_LDR_STR),
        (0xF000, 0x8000, FMT_LDRH_STRH_IMM),
        (0xE000, 0x6000, FMT_LDR_STR_IMM),
        (0xF200, 0x5200, FMT_LDR_STR_SIGNED),
        (0xF200, 0x5000, FMT_LDR_STR_REG),
        (0xF800, 0x4800, FMT_PC_LDR),
        (0xFC00, 0x4400, FMT_HI_REG_BX),
        (0xFC00, 0x4000, FMT_ALU),
        (0xE000, 0x2000, FMT_IMM_OPS),
        (0xF800, 0x1800, FMT_ADD_SUB),
        (0xE000, 0x0000, FMT_SHIFT),
    ];
    let mut t = [THUMB_NONE; 1024];
    let mut i = 0;
    while i < 1024 {
        let instr = (i as u16) << 6;
        let mut k = 0;
        while k < TESTS.len() {
            if instr & TESTS[k].0 == TESTS[k].1 {
                t[i] = TESTS[k].2;
                break;
            }
            k += 1;
        }
        i += 1;
    }
    t
};

fn execute_thumb(cpu: &mut Arm7Tdmi, mmu: &mut Mmu, instr: u16) -> u32 {
    let pc = cpu.regs[15];
    cpu.regs[15] = pc.wrapping_add(2);

    // Stage 1 of the JIT work (docs/JIT.md): the format is decided by the
    // top 10 bits, looked up in a table built at compile time from the
    // same mask/value tests, in the same order, as the old if-chain.
    match THUMB_FORMAT[(instr >> 6) as usize] {
        // Format 19: Long Branch with Link (BL)
        FMT_BL_HI => {
            // First half: setup upper 11 bits in LR (sign extended 11-bit offset)
            let offset = ((((instr & 0x07FF) as i16) << 5 >> 5) as i32) << 12;
            cpu.regs[14] = pc.wrapping_add(4).wrapping_add(offset as u32);
            return 1;
        }

        FMT_BL_LO => {
            // Second half: complete branch
            let offset = ((instr & 0x07FF) as u32) << 1;
            let target = cpu.regs[14].wrapping_add(offset);
            cpu.regs[14] = pc.wrapping_add(2) | 1;
            cpu.regs[15] = target & !1;
            return 3;
        }
        // Format 18: Unconditional Branch (B)
        FMT_B => {
            let offset = (((instr & 0x07FF) as i16) << 5 >> 4) as i32; // Sign-extend 11 bits and * 2
            cpu.regs[15] = pc.wrapping_add(4).wrapping_add(offset as u32);
            return 3;
        }
        // Format 17: Software Interrupt (SWI)
        FMT_SWI => {
            let comment = (instr & 0xFF) as u32;
            mmu.handle_swi(cpu, comment);
            return 3;
        }
        // Format 16: Conditional Branch (B<cond>)
        FMT_B_COND => {
            let cond = ((instr >> 8) & 0xF) as u32;
            if cond < 14 {
                if cpu.check_condition(cond) {
                    let offset = (((instr & 0xFF) as i8) as i32) << 1;
                    cpu.regs[15] = pc.wrapping_add(4).wrapping_add(offset as u32);
                    return 3;
                }
                return 1;
            }
            // cond == 14 is an undefined encoding on ARMv4T (15 is SWI,
            // handled above).
            cpu.trigger_undefined(pc.wrapping_add(2));
            return 3;
        }
        // Format 15: Multiple Load/Store (LDMIA, STMIA)
        //
        // ARM7TDMI quirks (ROADMAP M1, jsmolka thumb.gba 227-230), as in ARM
        // mode: an empty list transfers R15 and adds 0x40 to the base; STMIA
        // with the base in the list stores the old base only if it's first;
        // addresses are force-aligned without rotation.
        FMT_LDM_STM => {
            let l = (instr & (1 << 11)) != 0;
            let rb = ((instr >> 8) & 7) as usize;
            let reg_list = (instr & 0xFF) as u8;
            let base = cpu.regs[rb];

            if reg_list == 0 {
                if l {
                    cpu.regs[15] = mmu.read32(base & !3) & !1;
                } else {
                    // The stored PC is the instruction address + 6.
                    mmu.write32(base & !3, pc.wrapping_add(6));
                }
                cpu.regs[rb] = base.wrapping_add(0x40);
                return 3;
            }

            let num_regs = reg_list.count_ones();
            let final_addr = base.wrapping_add(num_regs * 4);
            let lowest = reg_list.trailing_zeros() as usize;
            let mut addr = base;
            for r in 0..8 {
                if (reg_list & (1 << r)) != 0 {
                    if l {
                        cpu.regs[r] = mmu.read32(addr & !3);
                    } else {
                        let val = if r == rb && r != lowest { final_addr } else { cpu.regs[r] };
                        mmu.write32(addr & !3, val);
                    }
                    addr = addr.wrapping_add(4);
                }
            }

            if !l || (reg_list & (1 << rb)) == 0 {
                cpu.regs[rb] = final_addr;
            }
            return num_regs + 2;
        }
        // Format 14: Push/Pop Registers (PUSH, POP)
        FMT_PUSH_POP => {
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
                        mmu.write32(sp & !3, cpu.regs[r]);
                        sp = sp.wrapping_add(4);
                    }
                }
                if r_bit {
                    mmu.write32(sp & !3, cpu.regs[14]); // Store LR
                }
                return num + 2;
            } else {
                // POP
                let mut sp = cpu.regs[13];
                let num = reg_list.count_ones() + if r_bit { 1 } else { 0 };

                for r in 0..8 {
                    if (reg_list & (1 << r)) != 0 {
                        cpu.regs[r] = mmu.read32(sp & !3);
                        sp = sp.wrapping_add(4);
                    }
                }
                if r_bit {
                    let target = mmu.read32(sp & !3);
                    sp = sp.wrapping_add(4);
                    cpu.regs[15] = target & !1;
                }
                cpu.regs[13] = sp;
                return num + (if r_bit { 3 } else { 2 });
            }
        }
        // Format 13: Add Offset to Stack Pointer (ADD SP, #±imm)
        FMT_ADD_SP => {
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
        FMT_LOAD_ADDR => {
            let is_sp = (instr & (1 << 11)) != 0;
            let rd = ((instr >> 8) & 7) as usize;
            let offset = ((instr & 0xFF) as u32) << 2;
            let base = if is_sp { cpu.regs[13] } else { (pc.wrapping_add(4)) & !2 };
            cpu.regs[rd] = base.wrapping_add(offset);
            return 1;
        }
        // Format 11: SP-Relative Load/Store (LDR, STR Rd, [SP, #imm])
        FMT_SP_LDR_STR => {
            let l = (instr & (1 << 11)) != 0;
            let rd = ((instr >> 8) & 7) as usize;
            let addr = cpu.regs[13].wrapping_add(((instr & 0xFF) as u32) << 2);
            if l {
                cpu.regs[rd] = mmu.read32(addr);
            } else {
                mmu.write32(addr, cpu.regs[rd]);
            }
            return 2;
        }
        // Format 10: Load/Store Halfword (LDRH, STRH Rd, [Rb, #imm])
        FMT_LDRH_STRH_IMM => {
            let l = (instr & (1 << 11)) != 0;
            let offset = (((instr >> 6) & 0x1F) as u32) << 1;
            let rb = ((instr >> 3) & 7) as usize;
            let rd = (instr & 7) as usize;
            let addr = cpu.regs[rb].wrapping_add(offset);
            if l {
                cpu.regs[rd] = super::load_halfword(mmu, addr);
            } else {
                mmu.write16(addr, (cpu.regs[rd] & 0xFFFF) as u16);
            }
            return 2;
        }
        // Format 9: Load/Store with Immediate Offset (STR, LDR, STRB, LDRB)
        FMT_LDR_STR_IMM => {
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
        FMT_LDR_STR_SIGNED => {
            let h = (instr & (1 << 11)) != 0;
            let s = (instr & (1 << 10)) != 0;
            let ro = ((instr >> 6) & 7) as usize;
            let rb = ((instr >> 3) & 7) as usize;
            let rd = (instr & 7) as usize;
            let addr = cpu.regs[rb].wrapping_add(cpu.regs[ro]);

            match (s, h) {
                (false, false) => mmu.write16(addr, (cpu.regs[rd] & 0xFFFF) as u16), // STRH
                (false, true) => cpu.regs[rd] = super::load_halfword(mmu, addr),     // LDRH
                (true, false) => cpu.regs[rd] = (mmu.read8(addr) as i8) as i32 as u32, // LDSB
                (true, true) => cpu.regs[rd] = super::load_signed_halfword(mmu, addr), // LDSH
            }
            return 2;
        }
        // Format 7: Load/Store with Register Offset
        FMT_LDR_STR_REG => {
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
        FMT_PC_LDR => {
            let rd = ((instr >> 8) & 7) as usize;
            let offset = ((instr & 0xFF) as u32) << 2;
            let addr = (pc.wrapping_add(4) & !2).wrapping_add(offset);
            cpu.regs[rd] = mmu.read32(addr);
            return 2;
        }
        // Format 5: Hi Register Operations / Branch Exchange
        FMT_HI_REG_BX => {
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
        FMT_ALU => {
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
        FMT_IMM_OPS => {
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
        FMT_ADD_SUB => {
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
        FMT_SHIFT => {
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
        _ => 1,
    }
}

#[cfg(test)]
mod decode_table_tests {
    use super::*;

    /// The if-chain the table replaced, test for test in the same order.
    fn reference_format(instr: u16) -> u8 {
        const TESTS: [(u16, u16); 20] = [
            (0xF800, 0xF000), (0xF800, 0xF800), (0xF800, 0xE000), (0xFF00, 0xDF00),
            (0xF000, 0xD000), (0xF000, 0xC000), (0xF600, 0xB400), (0xFF00, 0xB000),
            (0xF000, 0xA000), (0xF000, 0x9000), (0xF000, 0x8000), (0xE000, 0x6000),
            (0xF200, 0x5200), (0xF200, 0x5000), (0xF800, 0x4800), (0xFC00, 0x4400),
            (0xFC00, 0x4000), (0xE000, 0x2000), (0xF800, 0x1800), (0xE000, 0x0000),
        ];
        TESTS.iter().position(|&(m, v)| instr & m == v).map_or(THUMB_NONE, |k| k as u8)
    }

    #[test]
    fn table_matches_the_if_chain_for_every_instruction() {
        for instr in 0..=u16::MAX {
            assert_eq!(
                THUMB_FORMAT[(instr >> 6) as usize],
                reference_format(instr),
                "instr {instr:#06x}"
            );
        }
    }

    #[test]
    fn formats_are_named_in_chain_order() {
        assert_eq!(reference_format(0xF000), FMT_BL_HI);
        assert_eq!(reference_format(0xDF00), FMT_SWI);
        assert_eq!(reference_format(0xB500), FMT_PUSH_POP);
        assert_eq!(reference_format(0x4770), FMT_HI_REG_BX); // BX LR
        assert_eq!(reference_format(0x4000), FMT_ALU);
        assert_eq!(reference_format(0x0000), FMT_SHIFT);
    }
}
