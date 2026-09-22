//! Sharp SM83 CPU core (Game Boy / Game Boy Color).
//!
//! The SM83 is *not* a Z80 and *not* an 8080: it has no IX/IY, no alternate
//! register set, no I/O ports, but it does have `LD (FF00+n),A`, `LDI/LDD`,
//! `ADD SP,r8` and a half-carry flag that is computed on 4-bit boundaries for
//! 8-bit ops and on bit 11 for 16-bit ops. Those two carry rules are the most
//! common source of subtle failures in `cpu_instrs` test ROMs, so they are
//! implemented explicitly rather than inferred.
//!
//! All cycle counts returned are **T-cycles** (4 per machine cycle) at the
//! DMG clock of 4,194,304 Hz. In CGB double-speed mode the CPU runs twice as
//! fast; that is handled by the caller (see `dmg::GameBoy::step_instruction`),
//! which divides peripheral cycles rather than changing these numbers.

use super::mmu::GbMmu;

pub const FLAG_Z: u8 = 0x80;
pub const FLAG_N: u8 = 0x40;
pub const FLAG_H: u8 = 0x20;
pub const FLAG_C: u8 = 0x10;

#[derive(Clone, Debug, Default)]
pub struct Sm83 {
    pub a: u8,
    pub f: u8,
    pub b: u8,
    pub c: u8,
    pub d: u8,
    pub e: u8,
    pub h: u8,
    pub l: u8,
    pub sp: u16,
    pub pc: u16,

    pub ime: bool,
    /// EI enables interrupts only *after* the following instruction.
    pub ime_pending: bool,
    pub halted: bool,
    /// Set when HALT executes with IME=0 and a pending interrupt: the next
    /// byte fetch does not advance PC (the infamous HALT bug). Emulated
    /// because several commercial titles (and `halt_bug.gb`) depend on it.
    pub halt_bug: bool,
    pub stopped: bool,
    pub cycles: u64,
}

impl Sm83 {
    pub fn new() -> Self {
        Self::default()
    }

    /// Post-boot-ROM register state. Values differ between DMG and CGB; the
    /// A register in particular is how a cartridge detects which machine it
    /// is running on (0x01 = DMG, 0x11 = CGB), so games branch on it.
    pub fn reset_post_boot(&mut self, cgb: bool) {
        if cgb {
            self.a = 0x11;
            self.f = 0x80;
            self.b = 0x00;
            self.c = 0x00;
            self.d = 0xFF;
            self.e = 0x56;
            self.h = 0x00;
            self.l = 0x0D;
        } else {
            self.a = 0x01;
            self.f = 0xB0;
            self.b = 0x00;
            self.c = 0x13;
            self.d = 0x00;
            self.e = 0xD8;
            self.h = 0x01;
            self.l = 0x4D;
        }
        self.sp = 0xFFFE;
        self.pc = 0x0100;
        self.ime = false;
        self.ime_pending = false;
        self.halted = false;
        self.halt_bug = false;
        self.stopped = false;
        self.cycles = 0;
    }

    #[inline]
    pub fn af(&self) -> u16 {
        ((self.a as u16) << 8) | (self.f as u16)
    }
    #[inline]
    pub fn bc(&self) -> u16 {
        ((self.b as u16) << 8) | (self.c as u16)
    }
    #[inline]
    pub fn de(&self) -> u16 {
        ((self.d as u16) << 8) | (self.e as u16)
    }
    #[inline]
    pub fn hl(&self) -> u16 {
        ((self.h as u16) << 8) | (self.l as u16)
    }
    #[inline]
    pub fn set_af(&mut self, v: u16) {
        self.a = (v >> 8) as u8;
        self.f = (v as u8) & 0xF0; // low nibble of F is hardwired to 0
    }
    #[inline]
    pub fn set_bc(&mut self, v: u16) {
        self.b = (v >> 8) as u8;
        self.c = v as u8;
    }
    #[inline]
    pub fn set_de(&mut self, v: u16) {
        self.d = (v >> 8) as u8;
        self.e = v as u8;
    }
    #[inline]
    pub fn set_hl(&mut self, v: u16) {
        self.h = (v >> 8) as u8;
        self.l = v as u8;
    }

    #[inline]
    fn flag(&self, f: u8) -> bool {
        (self.f & f) != 0
    }
    #[inline]
    fn set_flag(&mut self, f: u8, on: bool) {
        if on {
            self.f |= f;
        } else {
            self.f &= !f;
        }
    }

    #[inline]
    fn fetch8(&mut self, mmu: &mut GbMmu) -> u8 {
        let v = mmu.read(self.pc);
        if self.halt_bug {
            // HALT bug: the byte after HALT is read twice (PC not incremented).
            self.halt_bug = false;
        } else {
            self.pc = self.pc.wrapping_add(1);
        }
        v
    }

    #[inline]
    fn fetch16(&mut self, mmu: &mut GbMmu) -> u16 {
        let lo = self.fetch8(mmu) as u16;
        let hi = self.fetch8(mmu) as u16;
        (hi << 8) | lo
    }

    #[inline]
    fn push16(&mut self, mmu: &mut GbMmu, v: u16) {
        self.sp = self.sp.wrapping_sub(1);
        mmu.write(self.sp, (v >> 8) as u8);
        self.sp = self.sp.wrapping_sub(1);
        mmu.write(self.sp, v as u8);
    }

    #[inline]
    fn pop16(&mut self, mmu: &mut GbMmu) -> u16 {
        let lo = mmu.read(self.sp) as u16;
        self.sp = self.sp.wrapping_add(1);
        let hi = mmu.read(self.sp) as u16;
        self.sp = self.sp.wrapping_add(1);
        (hi << 8) | lo
    }

    /// Register index encoding shared by the LD/ALU/CB blocks:
    /// 0=B 1=C 2=D 3=E 4=H 5=L 6=(HL) 7=A
    #[inline]
    fn read_r(&mut self, idx: u8, mmu: &mut GbMmu) -> u8 {
        match idx {
            0 => self.b,
            1 => self.c,
            2 => self.d,
            3 => self.e,
            4 => self.h,
            5 => self.l,
            6 => mmu.read(self.hl()),
            _ => self.a,
        }
    }

    #[inline]
    fn write_r(&mut self, idx: u8, val: u8, mmu: &mut GbMmu) {
        match idx {
            0 => self.b = val,
            1 => self.c = val,
            2 => self.d = val,
            3 => self.e = val,
            4 => self.h = val,
            5 => self.l = val,
            6 => mmu.write(self.hl(), val),
            _ => self.a = val,
        }
    }

    // ---- ALU ----------------------------------------------------------

    fn alu_add(&mut self, val: u8, carry_in: bool) {
        let c = carry_in as u16;
        let res = self.a as u16 + val as u16 + c;
        let half = (self.a & 0x0F) + (val & 0x0F) + c as u8;
        self.set_flag(FLAG_Z, (res as u8) == 0);
        self.set_flag(FLAG_N, false);
        self.set_flag(FLAG_H, half > 0x0F);
        self.set_flag(FLAG_C, res > 0xFF);
        self.a = res as u8;
    }

    fn alu_sub(&mut self, val: u8, carry_in: bool, store: bool) {
        let c = carry_in as i16;
        let res = self.a as i16 - val as i16 - c;
        let half = (self.a & 0x0F) as i16 - (val & 0x0F) as i16 - c;
        let out = res as u8;
        self.set_flag(FLAG_Z, out == 0);
        self.set_flag(FLAG_N, true);
        self.set_flag(FLAG_H, half < 0);
        self.set_flag(FLAG_C, res < 0);
        if store {
            self.a = out;
        }
    }

    fn alu_and(&mut self, val: u8) {
        self.a &= val;
        let z = self.a == 0;
        self.f = 0;
        self.set_flag(FLAG_Z, z);
        self.set_flag(FLAG_H, true);
    }

    fn alu_xor(&mut self, val: u8) {
        self.a ^= val;
        let z = self.a == 0;
        self.f = 0;
        self.set_flag(FLAG_Z, z);
    }

    fn alu_or(&mut self, val: u8) {
        self.a |= val;
        let z = self.a == 0;
        self.f = 0;
        self.set_flag(FLAG_Z, z);
    }

    fn alu_op(&mut self, op: u8, val: u8) {
        match op {
            0 => self.alu_add(val, false),
            1 => {
                let c = self.flag(FLAG_C);
                self.alu_add(val, c)
            }
            2 => self.alu_sub(val, false, true),
            3 => {
                let c = self.flag(FLAG_C);
                self.alu_sub(val, c, true)
            }
            4 => self.alu_and(val),
            5 => self.alu_xor(val),
            6 => self.alu_or(val),
            _ => self.alu_sub(val, false, false), // CP
        }
    }

    fn inc8(&mut self, val: u8) -> u8 {
        let res = val.wrapping_add(1);
        self.set_flag(FLAG_Z, res == 0);
        self.set_flag(FLAG_N, false);
        self.set_flag(FLAG_H, (val & 0x0F) == 0x0F);
        res
    }

    fn dec8(&mut self, val: u8) -> u8 {
        let res = val.wrapping_sub(1);
        self.set_flag(FLAG_Z, res == 0);
        self.set_flag(FLAG_N, true);
        self.set_flag(FLAG_H, (val & 0x0F) == 0);
        res
    }

    fn add_hl(&mut self, val: u16) {
        let hl = self.hl();
        let res = hl as u32 + val as u32;
        self.set_flag(FLAG_N, false);
        self.set_flag(FLAG_H, (hl & 0x0FFF) + (val & 0x0FFF) > 0x0FFF);
        self.set_flag(FLAG_C, res > 0xFFFF);
        self.set_hl(res as u16);
    }

    /// `ADD SP,r8` and `LD HL,SP+r8` share the same flag rules: H and C are
    /// computed from the *unsigned low byte* of the operand, Z and N cleared.
    fn add_sp_imm(&mut self, imm: i8) -> u16 {
        let sp = self.sp;
        let off = imm as u16;
        let res = sp.wrapping_add(off);
        self.f = 0;
        self.set_flag(FLAG_H, (sp & 0x0F) + (off & 0x0F) > 0x0F);
        self.set_flag(FLAG_C, (sp & 0xFF) + (off & 0xFF) > 0xFF);
        res
    }

    /// Decimal adjust. Post-BCD correction driven by N/H/C, not by re-reading
    /// the operands; getting the N branch wrong is the classic `daa` failure.
    fn daa(&mut self) {
        let mut a = self.a;
        let mut correction: u8 = 0;
        let mut carry = self.flag(FLAG_C);

        if self.flag(FLAG_H) || (!self.flag(FLAG_N) && (a & 0x0F) > 9) {
            correction |= 0x06;
        }
        if carry || (!self.flag(FLAG_N) && a > 0x99) {
            correction |= 0x60;
            carry = true;
        }
        a = if self.flag(FLAG_N) {
            a.wrapping_sub(correction)
        } else {
            a.wrapping_add(correction)
        };
        self.a = a;
        self.set_flag(FLAG_Z, a == 0);
        self.set_flag(FLAG_H, false);
        self.set_flag(FLAG_C, carry);
    }

    // ---- CB-prefixed rotate/shift -------------------------------------

    fn cb_rot(&mut self, op: u8, val: u8) -> u8 {
        let c = self.flag(FLAG_C);
        let (res, carry) = match op {
            0 => (val.rotate_left(1), (val & 0x80) != 0),                       // RLC
            1 => (val.rotate_right(1), (val & 0x01) != 0),                      // RRC
            2 => ((val << 1) | c as u8, (val & 0x80) != 0),                     // RL
            3 => ((val >> 1) | ((c as u8) << 7), (val & 0x01) != 0),            // RR
            4 => (val << 1, (val & 0x80) != 0),                                 // SLA
            5 => ((val >> 1) | (val & 0x80), (val & 0x01) != 0),                // SRA
            6 => ((val << 4) | (val >> 4), false),                              // SWAP
            _ => (val >> 1, (val & 0x01) != 0),                                 // SRL
        };
        self.f = 0;
        self.set_flag(FLAG_Z, res == 0);
        self.set_flag(FLAG_C, carry);
        res
    }

    fn step_cb(&mut self, mmu: &mut GbMmu) -> u32 {
        let op = self.fetch8(mmu);
        let reg = op & 0x07;
        let is_hl = reg == 6;
        match op >> 6 {
            0 => {
                let val = self.read_r(reg, mmu);
                let res = self.cb_rot(op >> 3, val);
                self.write_r(reg, res, mmu);
                if is_hl {
                    16
                } else {
                    8
                }
            }
            1 => {
                // BIT n,r -- read-only, so (HL) costs 12 not 16
                let bit = (op >> 3) & 7;
                let val = self.read_r(reg, mmu);
                self.set_flag(FLAG_Z, (val & (1 << bit)) == 0);
                self.set_flag(FLAG_N, false);
                self.set_flag(FLAG_H, true);
                if is_hl {
                    12
                } else {
                    8
                }
            }
            2 => {
                let bit = (op >> 3) & 7;
                let val = self.read_r(reg, mmu) & !(1 << bit);
                self.write_r(reg, val, mmu);
                if is_hl {
                    16
                } else {
                    8
                }
            }
            _ => {
                let bit = (op >> 3) & 7;
                let val = self.read_r(reg, mmu) | (1 << bit);
                self.write_r(reg, val, mmu);
                if is_hl {
                    16
                } else {
                    8
                }
            }
        }
    }

    // ---- Interrupts ---------------------------------------------------

    /// Services the highest-priority pending+enabled interrupt if IME is set.
    /// Returns the T-cycles consumed (20, or 0 when nothing was serviced).
    fn service_interrupt(&mut self, mmu: &mut GbMmu) -> u32 {
        let pending = mmu.ie_reg & mmu.if_reg & 0x1F;
        if pending == 0 {
            return 0;
        }
        // Any pending+enabled interrupt wakes HALT regardless of IME.
        self.halted = false;
        if !self.ime {
            return 0;
        }
        let idx = pending.trailing_zeros() as u8;
        self.ime = false;
        mmu.if_reg &= !(1 << idx);
        self.push16(mmu, self.pc);
        self.pc = 0x40 + (idx as u16) * 8;
        20
    }

    /// Execute one instruction (or service one interrupt). Returns T-cycles.
    pub fn step(&mut self, mmu: &mut GbMmu) -> u32 {
        // EI takes effect after the instruction following it.
        let enable_ime_after = self.ime_pending;

        let irq_cycles = self.service_interrupt(mmu);
        if irq_cycles > 0 {
            self.cycles += irq_cycles as u64;
            return irq_cycles;
        }

        if self.halted {
            self.cycles += 4;
            return 4;
        }

        let cycles = self.execute(mmu);

        if enable_ime_after {
            self.ime = true;
            self.ime_pending = false;
        }

        self.cycles += cycles as u64;
        cycles
    }

    fn execute(&mut self, mmu: &mut GbMmu) -> u32 {
        let op = self.fetch8(mmu);

        // Regular blocks first: LD r,r' (0x40-0x7F except HALT) and ALU
        // A,r (0x80-0xBF). Decoding them structurally keeps the explicit
        // match below to the genuinely irregular opcodes.
        if (0x40..=0x7F).contains(&op) && op != 0x76 {
            let dst = (op >> 3) & 7;
            let src = op & 7;
            let val = self.read_r(src, mmu);
            self.write_r(dst, val, mmu);
            return if dst == 6 || src == 6 { 8 } else { 4 };
        }
        if (0x80..=0xBF).contains(&op) {
            let src = op & 7;
            let val = self.read_r(src, mmu);
            self.alu_op((op >> 3) & 7, val);
            return if src == 6 { 8 } else { 4 };
        }

        match op {
            0x00 => 4, // NOP
            0x10 => {
                // STOP. On CGB with KEY1 bit0 armed this performs the
                // speed switch instead of halting the machine.
                let _ = self.fetch8(mmu); // STOP is a 2-byte opcode
                if mmu.cgb && (mmu.key1 & 0x01) != 0 {
                    mmu.double_speed = !mmu.double_speed;
                    mmu.key1 = if mmu.double_speed { 0x80 } else { 0x00 };
                } else {
                    self.stopped = true;
                }
                4
            }
            0x76 => {
                // HALT
                if !self.ime && (mmu.ie_reg & mmu.if_reg & 0x1F) != 0 {
                    self.halt_bug = true;
                } else {
                    self.halted = true;
                }
                4
            }
            0xCB => self.step_cb(mmu),

            // 16-bit loads
            0x01 => {
                let v = self.fetch16(mmu);
                self.set_bc(v);
                12
            }
            0x11 => {
                let v = self.fetch16(mmu);
                self.set_de(v);
                12
            }
            0x21 => {
                let v = self.fetch16(mmu);
                self.set_hl(v);
                12
            }
            0x31 => {
                self.sp = self.fetch16(mmu);
                12
            }
            0x08 => {
                let addr = self.fetch16(mmu);
                mmu.write(addr, self.sp as u8);
                mmu.write(addr.wrapping_add(1), (self.sp >> 8) as u8);
                20
            }
            0xF9 => {
                self.sp = self.hl();
                8
            }
            0xF8 => {
                let imm = self.fetch8(mmu) as i8;
                let v = self.add_sp_imm(imm);
                self.set_hl(v);
                12
            }
            0xE8 => {
                let imm = self.fetch8(mmu) as i8;
                self.sp = self.add_sp_imm(imm);
                16
            }

            // 8-bit immediate loads
            0x06 | 0x0E | 0x16 | 0x1E | 0x26 | 0x2E | 0x36 | 0x3E => {
                let dst = (op >> 3) & 7;
                let v = self.fetch8(mmu);
                self.write_r(dst, v, mmu);
                if dst == 6 {
                    12
                } else {
                    8
                }
            }

            // Indirect loads
            0x02 => {
                mmu.write(self.bc(), self.a);
                8
            }
            0x12 => {
                mmu.write(self.de(), self.a);
                8
            }
            0x22 => {
                let hl = self.hl();
                mmu.write(hl, self.a);
                self.set_hl(hl.wrapping_add(1));
                8
            }
            0x32 => {
                let hl = self.hl();
                mmu.write(hl, self.a);
                self.set_hl(hl.wrapping_sub(1));
                8
            }
            0x0A => {
                self.a = mmu.read(self.bc());
                8
            }
            0x1A => {
                self.a = mmu.read(self.de());
                8
            }
            0x2A => {
                let hl = self.hl();
                self.a = mmu.read(hl);
                self.set_hl(hl.wrapping_add(1));
                8
            }
            0x3A => {
                let hl = self.hl();
                self.a = mmu.read(hl);
                self.set_hl(hl.wrapping_sub(1));
                8
            }
            0xE0 => {
                let n = self.fetch8(mmu) as u16;
                mmu.write(0xFF00 + n, self.a);
                12
            }
            0xF0 => {
                let n = self.fetch8(mmu) as u16;
                self.a = mmu.read(0xFF00 + n);
                12
            }
            0xE2 => {
                mmu.write(0xFF00 + self.c as u16, self.a);
                8
            }
            0xF2 => {
                self.a = mmu.read(0xFF00 + self.c as u16);
                8
            }
            0xEA => {
                let addr = self.fetch16(mmu);
                mmu.write(addr, self.a);
                16
            }
            0xFA => {
                let addr = self.fetch16(mmu);
                self.a = mmu.read(addr);
                16
            }

            // INC/DEC 16
            0x03 => {
                let v = self.bc().wrapping_add(1);
                self.set_bc(v);
                8
            }
            0x13 => {
                let v = self.de().wrapping_add(1);
                self.set_de(v);
                8
            }
            0x23 => {
                let v = self.hl().wrapping_add(1);
                self.set_hl(v);
                8
            }
            0x33 => {
                self.sp = self.sp.wrapping_add(1);
                8
            }
            0x0B => {
                let v = self.bc().wrapping_sub(1);
                self.set_bc(v);
                8
            }
            0x1B => {
                let v = self.de().wrapping_sub(1);
                self.set_de(v);
                8
            }
            0x2B => {
                let v = self.hl().wrapping_sub(1);
                self.set_hl(v);
                8
            }
            0x3B => {
                self.sp = self.sp.wrapping_sub(1);
                8
            }

            // INC/DEC 8
            0x04 | 0x0C | 0x14 | 0x1C | 0x24 | 0x2C | 0x34 | 0x3C => {
                let r = (op >> 3) & 7;
                let v = self.read_r(r, mmu);
                let res = self.inc8(v);
                self.write_r(r, res, mmu);
                if r == 6 {
                    12
                } else {
                    4
                }
            }
            0x05 | 0x0D | 0x15 | 0x1D | 0x25 | 0x2D | 0x35 | 0x3D => {
                let r = (op >> 3) & 7;
                let v = self.read_r(r, mmu);
                let res = self.dec8(v);
                self.write_r(r, res, mmu);
                if r == 6 {
                    12
                } else {
                    4
                }
            }

            // ADD HL,rr
            0x09 => {
                self.add_hl(self.bc());
                8
            }
            0x19 => {
                self.add_hl(self.de());
                8
            }
            0x29 => {
                self.add_hl(self.hl());
                8
            }
            0x39 => {
                self.add_hl(self.sp);
                8
            }

            // ALU with immediate
            0xC6 | 0xCE | 0xD6 | 0xDE | 0xE6 | 0xEE | 0xF6 | 0xFE => {
                let v = self.fetch8(mmu);
                self.alu_op((op >> 3) & 7, v);
                8
            }

            // Rotates on A (always clear Z, unlike their CB counterparts)
            0x07 | 0x0F | 0x17 | 0x1F => {
                let res = self.cb_rot((op >> 3) & 7, self.a);
                self.a = res;
                self.set_flag(FLAG_Z, false);
                4
            }
            0x27 => {
                self.daa();
                4
            }
            0x2F => {
                self.a = !self.a;
                self.set_flag(FLAG_N, true);
                self.set_flag(FLAG_H, true);
                4
            }
            0x37 => {
                self.set_flag(FLAG_N, false);
                self.set_flag(FLAG_H, false);
                self.set_flag(FLAG_C, true);
                4
            }
            0x3F => {
                let c = self.flag(FLAG_C);
                self.set_flag(FLAG_N, false);
                self.set_flag(FLAG_H, false);
                self.set_flag(FLAG_C, !c);
                4
            }

            // Jumps
            0xC3 => {
                self.pc = self.fetch16(mmu);
                16
            }
            0xE9 => {
                self.pc = self.hl();
                4
            }
            0xC2 | 0xCA | 0xD2 | 0xDA => {
                let addr = self.fetch16(mmu);
                if self.cond((op >> 3) & 3) {
                    self.pc = addr;
                    16
                } else {
                    12
                }
            }
            0x18 => {
                let off = self.fetch8(mmu) as i8;
                self.pc = self.pc.wrapping_add(off as u16);
                12
            }
            0x20 | 0x28 | 0x30 | 0x38 => {
                let off = self.fetch8(mmu) as i8;
                if self.cond((op >> 3) & 3) {
                    self.pc = self.pc.wrapping_add(off as u16);
                    12
                } else {
                    8
                }
            }

            // Calls / returns
            0xCD => {
                let addr = self.fetch16(mmu);
                self.push16(mmu, self.pc);
                self.pc = addr;
                24
            }
            0xC4 | 0xCC | 0xD4 | 0xDC => {
                let addr = self.fetch16(mmu);
                if self.cond((op >> 3) & 3) {
                    self.push16(mmu, self.pc);
                    self.pc = addr;
                    24
                } else {
                    12
                }
            }
            0xC9 => {
                self.pc = self.pop16(mmu);
                16
            }
            0xD9 => {
                self.pc = self.pop16(mmu);
                self.ime = true;
                self.ime_pending = false;
                16
            }
            0xC0 | 0xC8 | 0xD0 | 0xD8 => {
                if self.cond((op >> 3) & 3) {
                    self.pc = self.pop16(mmu);
                    20
                } else {
                    8
                }
            }
            0xC7 | 0xCF | 0xD7 | 0xDF | 0xE7 | 0xEF | 0xF7 | 0xFF => {
                self.push16(mmu, self.pc);
                self.pc = (op & 0x38) as u16;
                16
            }

            // Stack
            0xC1 => {
                let v = self.pop16(mmu);
                self.set_bc(v);
                12
            }
            0xD1 => {
                let v = self.pop16(mmu);
                self.set_de(v);
                12
            }
            0xE1 => {
                let v = self.pop16(mmu);
                self.set_hl(v);
                12
            }
            0xF1 => {
                let v = self.pop16(mmu);
                self.set_af(v);
                12
            }
            0xC5 => {
                self.push16(mmu, self.bc());
                16
            }
            0xD5 => {
                self.push16(mmu, self.de());
                16
            }
            0xE5 => {
                self.push16(mmu, self.hl());
                16
            }
            0xF5 => {
                self.push16(mmu, self.af());
                16
            }

            0xF3 => {
                self.ime = false;
                self.ime_pending = false;
                4
            }
            0xFB => {
                self.ime_pending = true;
                4
            }

            // Undefined opcodes lock up real hardware; treat as NOP but log
            // once so a bad jump is visible rather than silently looping.
            0xD3 | 0xDB | 0xDD | 0xE3 | 0xE4 | 0xEB | 0xEC | 0xED | 0xF4 | 0xFC | 0xFD => {
                log::warn!("SM83: undefined opcode {:02X} at {:04X}", op, self.pc.wrapping_sub(1));
                4
            }

            _ => 4,
        }
    }

    #[inline]
    fn cond(&self, code: u8) -> bool {
        match code {
            0 => !self.flag(FLAG_Z),
            1 => self.flag(FLAG_Z),
            2 => !self.flag(FLAG_C),
            _ => self.flag(FLAG_C),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dmg::mmu::GbMmu;

    fn harness(prog: &[u8]) -> (Sm83, GbMmu) {
        let mut rom = vec![0u8; 0x8000];
        rom[0x0147] = 0x00; // ROM ONLY
        rom[0x0100..0x0100 + prog.len()].copy_from_slice(prog);
        let mut mmu = GbMmu::new(rom, false);
        let mut cpu = Sm83::new();
        cpu.reset_post_boot(false);
        mmu.boot_rom_mapped = false;
        (cpu, mmu)
    }

    #[test]
    fn half_carry_is_computed_on_the_low_nibble() {
        let (mut cpu, mut mmu) = harness(&[0x3E, 0x0F, 0xC6, 0x01]); // LD A,0F ; ADD A,01
        cpu.step(&mut mmu);
        cpu.step(&mut mmu);
        assert_eq!(cpu.a, 0x10);
        assert!(cpu.f & FLAG_H != 0, "0F+01 must set H");
        assert!(cpu.f & FLAG_C == 0);
    }

    #[test]
    fn daa_after_addition_produces_bcd() {
        // LD A,0x09 ; ADD A,0x01 ; DAA  => 0x10 in BCD
        let (mut cpu, mut mmu) = harness(&[0x3E, 0x09, 0xC6, 0x01, 0x27]);
        for _ in 0..3 {
            cpu.step(&mut mmu);
        }
        assert_eq!(cpu.a, 0x10);
    }

    #[test]
    fn daa_after_subtraction_borrows_correctly() {
        // LD A,0x10 ; SUB 0x01 ; DAA => 0x09
        let (mut cpu, mut mmu) = harness(&[0x3E, 0x10, 0xD6, 0x01, 0x27]);
        for _ in 0..3 {
            cpu.step(&mut mmu);
        }
        assert_eq!(cpu.a, 0x09);
    }

    #[test]
    fn f_register_low_nibble_reads_back_as_zero() {
        // LD SP,0xC000 ; LD A,0xFF ; PUSH AF is awkward here, so poke AF.
        let (mut cpu, _mmu) = harness(&[0x00]);
        cpu.set_af(0xFFFF);
        assert_eq!(cpu.f & 0x0F, 0, "low nibble of F is hardwired to 0");
    }

    #[test]
    fn add_sp_imm_flags_come_from_the_unsigned_low_byte() {
        let (mut cpu, mut mmu) = harness(&[0x31, 0xFF, 0x0F, 0xE8, 0x01]); // LD SP,0x0FFF ; ADD SP,1
        cpu.step(&mut mmu);
        cpu.step(&mut mmu);
        assert_eq!(cpu.sp, 0x1000);
        assert!(cpu.f & FLAG_H != 0);
        assert!(cpu.f & FLAG_C != 0);
        assert!(cpu.f & FLAG_Z == 0, "Z is always cleared by ADD SP,r8");
    }

    #[test]
    fn cb_bit_on_hl_costs_twelve_cycles() {
        let (mut cpu, mut mmu) = harness(&[0xCB, 0x46]); // BIT 0,(HL)
        let c = cpu.step(&mut mmu);
        assert_eq!(c, 12);
    }

    #[test]
    fn interrupt_dispatch_pushes_pc_and_jumps_to_vector() {
        let (mut cpu, mut mmu) = harness(&[0x00]);
        cpu.sp = 0xFFFE;
        cpu.ime = true;
        mmu.ie_reg = 0x01;
        mmu.if_reg = 0x01;
        let c = cpu.step(&mut mmu);
        assert_eq!(c, 20);
        assert_eq!(cpu.pc, 0x0040, "VBlank vector");
        assert!(!cpu.ime);
        assert_eq!(mmu.if_reg & 0x01, 0, "IF bit cleared on dispatch");
    }

    #[test]
    fn ei_is_delayed_by_one_instruction() {
        let (mut cpu, mut mmu) = harness(&[0xFB, 0x00]); // EI ; NOP
        cpu.step(&mut mmu);
        assert!(!cpu.ime, "IME must not be set during EI itself");
        cpu.step(&mut mmu);
        assert!(cpu.ime, "IME set after the instruction following EI");
    }
}
