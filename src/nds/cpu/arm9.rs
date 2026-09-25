//! ARM946E-S Core Implementation (ARMv5TE + CP15)
//!
//! Main CPU of the Nintendo DS, running at 67.028 MHz.

use crate::gba::cpu::CpuMode;
use crate::gba::cpu::{FLAG_C, FLAG_I, FLAG_N, FLAG_T, FLAG_V, FLAG_Z};
use crate::nds::bus::NdsBus;
use crate::nds::cpu::executor::{step_instruction, CpuBus, CpuState, FLAG_Q};

pub struct Arm946eS {
    pub regs: [u32; 16],
    pub cpsr: u32,

    // Banked registers
    pub r8_usr: [u32; 5],
    pub r8_fiq: [u32; 5],
    pub r13_usr: u32,
    pub r14_usr: u32,
    pub r13_fiq: u32,
    pub r14_fiq: u32,
    pub spsr_fiq: u32,
    pub r13_irq: u32,
    pub r14_irq: u32,
    pub spsr_irq: u32,
    pub r13_svc: u32,
    pub r14_svc: u32,
    pub spsr_svc: u32,
    pub r13_abt: u32,
    pub r14_abt: u32,
    pub spsr_abt: u32,
    pub r13_und: u32,
    pub r14_und: u32,
    pub spsr_und: u32,

    // CP15 System Control Coprocessor
    pub cp15_control: u32,
    pub itcm_control: u32,
    pub dtcm_control: u32,

    pub halted: bool,
}

impl Default for Arm946eS {
    fn default() -> Self {
        Self::new()
    }
}

impl Arm946eS {
    pub fn new() -> Self {
        Self {
            regs: [0; 16],
            cpsr: CpuMode::System as u32,
            r8_usr: [0; 5],
            r8_fiq: [0; 5],
            r13_usr: 0x027FFFE0,
            r14_usr: 0,
            r13_fiq: 0,
            r14_fiq: 0,
            spsr_fiq: 0,
            r13_irq: 0x027FFF80,
            r14_irq: 0,
            spsr_irq: 0,
            r13_svc: 0x027FFFC0,
            r14_svc: 0,
            spsr_svc: 0,
            r13_abt: 0,
            r14_abt: 0,
            spsr_abt: 0,
            r13_und: 0,
            r14_und: 0,
            spsr_und: 0,

            cp15_control: 0x00050078, // Default DTCM/ITCM enabled
            itcm_control: 0x00000038, // 32KB at 0x00000000
            dtcm_control: 0x027C0030, // 16KB at 0x027C0000
            halted: false,
        }
    }

    #[inline]
    pub fn itcm_base(&self) -> u32 {
        self.itcm_control & 0xFFFFF000
    }

    #[inline]
    pub fn dtcm_base(&self) -> u32 {
        self.dtcm_control & 0xFFFFF000
    }

    #[inline]
    pub fn is_itcm_enabled(&self) -> bool {
        (self.cp15_control & (1 << 18)) != 0
    }

    #[inline]
    pub fn is_dtcm_enabled(&self) -> bool {
        (self.cp15_control & (1 << 16)) != 0
    }

    #[inline]
    pub fn mode(&self) -> CpuMode {
        CpuMode::from_bits(self.cpsr)
    }

    #[inline]
    pub fn is_thumb(&self) -> bool {
        (self.cpsr & FLAG_T) != 0
    }

    pub fn set_cpsr(&mut self, val: u32) {
        let old_mode = self.mode();
        let new_mode = CpuMode::from_bits(val);
        if old_mode != new_mode {
            self.switch_mode(old_mode, new_mode);
        }
        self.cpsr = val;
    }

    fn switch_mode(&mut self, old: CpuMode, new: CpuMode) {
        match old {
            CpuMode::User | CpuMode::System => {
                self.r13_usr = self.regs[13];
                self.r14_usr = self.regs[14];
            }
            CpuMode::Fiq => {
                self.r8_fiq.copy_from_slice(&self.regs[8..13]);
                self.r13_fiq = self.regs[13];
                self.r14_fiq = self.regs[14];
            }
            CpuMode::Irq => {
                self.r13_irq = self.regs[13];
                self.r14_irq = self.regs[14];
            }
            CpuMode::Supervisor => {
                self.r13_svc = self.regs[13];
                self.r14_svc = self.regs[14];
            }
            CpuMode::Abort => {
                self.r13_abt = self.regs[13];
                self.r14_abt = self.regs[14];
            }
            CpuMode::Undefined => {
                self.r13_und = self.regs[13];
                self.r14_und = self.regs[14];
            }
        }

        if old == CpuMode::Fiq && new != CpuMode::Fiq {
            self.regs[8..13].copy_from_slice(&self.r8_usr);
        } else if old != CpuMode::Fiq && new == CpuMode::Fiq {
            self.r8_usr.copy_from_slice(&self.regs[8..13]);
            self.regs[8..13].copy_from_slice(&self.r8_fiq);
        }

        match new {
            CpuMode::User | CpuMode::System => {
                self.regs[13] = self.r13_usr;
                self.regs[14] = self.r14_usr;
            }
            CpuMode::Fiq => {
                self.regs[13] = self.r13_fiq;
                self.regs[14] = self.r14_fiq;
            }
            CpuMode::Irq => {
                self.regs[13] = self.r13_irq;
                self.regs[14] = self.r14_irq;
            }
            CpuMode::Supervisor => {
                self.regs[13] = self.r13_svc;
                self.regs[14] = self.r14_svc;
            }
            CpuMode::Abort => {
                self.regs[13] = self.r13_abt;
                self.regs[14] = self.r14_abt;
            }
            CpuMode::Undefined => {
                self.regs[13] = self.r13_und;
                self.regs[14] = self.r14_und;
            }
        }
    }

    pub fn trigger_irq(&mut self) {
        if (self.cpsr & FLAG_I) != 0 {
            return;
        }
        self.halted = false;
        let old_cpsr = self.cpsr;
        let return_pc = self.regs[15].wrapping_add(4);
        self.set_cpsr((old_cpsr & !0x3F) | (CpuMode::Irq as u32) | FLAG_I);
        self.spsr_irq = old_cpsr;
        self.regs[14] = return_pc;
        let high_vec = (self.cp15_control & (1 << 13)) != 0;
        self.regs[15] = if high_vec { 0xFFFF_0018 } else { 0x0000_0018 };
    }

    pub fn trigger_swi(&mut self, _comment: u32) {
        let old_cpsr = self.cpsr;
        let return_pc = self.regs[15];
        self.set_cpsr((old_cpsr & !0x3F) | (CpuMode::Supervisor as u32) | FLAG_I);
        self.spsr_svc = old_cpsr;
        self.regs[14] = return_pc;
        let high_vec = (self.cp15_control & (1 << 13)) != 0;
        self.regs[15] = if high_vec { 0xFFFF_0008 } else { 0x0000_0008 };
    }

    pub fn mcr(&mut self, crn: u32, crm: u32, op1: u32, op2: u32, val: u32) {
        match (crn, crm, op1, op2) {
            (1, 0, 0, 0) => self.cp15_control = val,
            (9, 1, 0, 0) => self.dtcm_control = val,
            (9, 1, 0, 1) => self.itcm_control = val,
            _ => log::trace!("CP15 MCR write crn={crn} crm={crm} op1={op1} op2={op2} val={val:08X}"),
        }
    }

    pub fn mrc(&self, crn: u32, crm: u32, op1: u32, op2: u32) -> u32 {
        match (crn, crm, op1, op2) {
            (0, 0, 0, 0) => 0x41059461, // ARM946E-S rev 1 ID
            (0, 0, 0, 1) => 0x0F0D2112, // Cache type (8KB I-cache, 4KB D-cache)
            (1, 0, 0, 0) => self.cp15_control,
            (9, 1, 0, 0) => self.dtcm_control,
            (9, 1, 0, 1) => self.itcm_control,
            _ => 0,
        }
    }

    pub fn op_clz(&mut self, rd: usize, rm: usize) {
        let val = self.regs[rm];
        self.regs[rd] = val.leading_zeros();
    }

    pub fn op_qadd(&mut self, rd: usize, rm: usize, rn: usize) {
        let a = self.regs[rm] as i32;
        let b = self.regs[rn] as i32;
        let (res, overflow) = a.overflowing_add(b);
        if overflow {
            self.cpsr |= FLAG_Q;
            self.regs[rd] = if a < 0 { i32::MIN as u32 } else { i32::MAX as u32 };
        } else {
            self.regs[rd] = res as u32;
        }
    }

    pub fn op_qsub(&mut self, rd: usize, rm: usize, rn: usize) {
        let a = self.regs[rm] as i32;
        let b = self.regs[rn] as i32;
        let (res, overflow) = a.overflowing_sub(b);
        if overflow {
            self.cpsr |= FLAG_Q;
            self.regs[rd] = if a < 0 { i32::MIN as u32 } else { i32::MAX as u32 };
        } else {
            self.regs[rd] = res as u32;
        }
    }

    pub fn op_smulxy(&mut self, rd: usize, rm: usize, rs: usize, x_top: bool, y_top: bool) {
        let a = if x_top { (self.regs[rm] >> 16) as i16 } else { self.regs[rm] as i16 } as i32;
        let b = if y_top { (self.regs[rs] >> 16) as i16 } else { self.regs[rs] as i16 } as i32;
        self.regs[rd] = (a * b) as u32;
    }

    pub fn op_smlaxy(&mut self, rd: usize, rm: usize, rs: usize, rn: usize, x_top: bool, y_top: bool) {
        let a = if x_top { (self.regs[rm] >> 16) as i16 } else { self.regs[rm] as i16 } as i32;
        let b = if y_top { (self.regs[rs] >> 16) as i16 } else { self.regs[rs] as i16 } as i32;
        let mul = a * b;
        let acc = self.regs[rn] as i32;
        let (res, overflow) = mul.overflowing_add(acc);
        if overflow {
            self.cpsr |= FLAG_Q;
        }
        self.regs[rd] = res as u32;
    }

    pub fn op_blx_reg(&mut self, rm: usize) {
        let target = self.regs[rm];
        self.regs[14] = self.regs[15].wrapping_sub(4);
        if (target & 1) != 0 {
            self.cpsr |= FLAG_T;
            self.regs[15] = target & !1;
        } else {
            self.cpsr &= !FLAG_T;
            self.regs[15] = target & !3;
        }
    }
}

pub struct Arm9Bus<'a> {
    pub bus: &'a mut NdsBus,
}

impl<'a> CpuBus for Arm9Bus<'a> {
    #[inline(always)]
    fn read8(&mut self, addr: u32) -> u8 {
        self.bus.read_arm9_u8(addr)
    }

    #[inline(always)]
    fn read16(&mut self, addr: u32) -> u16 {
        self.bus.read_arm9_u16(addr)
    }

    #[inline(always)]
    fn read32(&mut self, addr: u32) -> u32 {
        self.bus.read_arm9_u32(addr)
    }

    #[inline(always)]
    fn write8(&mut self, addr: u32, val: u8) {
        self.bus.write_arm9_u8(addr, val);
    }

    #[inline(always)]
    fn write16(&mut self, addr: u32, val: u16) {
        self.bus.write_arm9_u16(addr, val);
    }

    #[inline(always)]
    fn write32(&mut self, addr: u32, val: u32) {
        self.bus.write_arm9_u32(addr, val);
    }
}

impl CpuState for Arm946eS {
    #[inline(always)]
    fn reg(&self, idx: usize) -> u32 {
        self.regs[idx]
    }

    #[inline(always)]
    fn set_reg(&mut self, idx: usize, val: u32) {
        self.regs[idx] = val;
    }

    #[inline(always)]
    fn cpsr(&self) -> u32 {
        self.cpsr
    }

    #[inline(always)]
    fn set_cpsr(&mut self, val: u32) {
        self.set_cpsr(val);
    }

    #[inline(always)]
    fn get_spsr(&self) -> u32 {
        match self.mode() {
            CpuMode::Fiq => self.spsr_fiq,
            CpuMode::Irq => self.spsr_irq,
            CpuMode::Supervisor => self.spsr_svc,
            CpuMode::Abort => self.spsr_abt,
            CpuMode::Undefined => self.spsr_und,
            _ => self.cpsr,
        }
    }

    #[inline(always)]
    fn set_spsr(&mut self, val: u32) {
        match self.mode() {
            CpuMode::Fiq => self.spsr_fiq = val,
            CpuMode::Irq => self.spsr_irq = val,
            CpuMode::Supervisor => self.spsr_svc = val,
            CpuMode::Abort => self.spsr_abt = val,
            CpuMode::Undefined => self.spsr_und = val,
            _ => {}
        }
    }

    #[inline(always)]
    fn is_thumb(&self) -> bool {
        (self.cpsr & FLAG_T) != 0
    }

    #[inline(always)]
    fn is_halted(&self) -> bool {
        self.halted
    }

    #[inline(always)]
    fn set_halted(&mut self, halted: bool) {
        self.halted = halted;
    }

    #[inline(always)]
    fn check_condition(&self, cond: u32) -> bool {
        let n = (self.cpsr & FLAG_N) != 0;
        let z = (self.cpsr & FLAG_Z) != 0;
        let c = (self.cpsr & FLAG_C) != 0;
        let v = (self.cpsr & FLAG_V) != 0;

        match cond {
            0 => z,                   // EQ
            1 => !z,                  // NE
            2 => c,                   // CS/HS
            3 => !c,                  // CC/LO
            4 => n,                   // MI
            5 => !n,                  // PL
            6 => v,                   // VS
            7 => !v,                  // VC
            8 => c && !z,             // HI
            9 => !c || z,             // LS
            10 => n == v,             // GE
            11 => n != v,             // LT
            12 => !z && (n == v),     // GT
            13 => z || (n != v),      // LE
            14 => true,               // AL
            _ => false,
        }
    }

    #[inline(always)]
    fn trigger_irq(&mut self) {
        self.trigger_irq();
    }

    #[inline(always)]
    fn trigger_swi(&mut self, comment: u32) {
        self.trigger_swi(comment);
    }

    #[inline(always)]
    fn is_armv5(&self) -> bool {
        true
    }

    #[inline(always)]
    fn mcr(&mut self, crn: u32, crm: u32, op1: u32, op2: u32, val: u32) {
        self.mcr(crn, crm, op1, op2, val);
    }

    #[inline(always)]
    fn mrc(&self, crn: u32, crm: u32, op1: u32, op2: u32) -> u32 {
        self.mrc(crn, crm, op1, op2)
    }
}

/// Execute one ARM9 instruction through memory and CP15 TCMs
pub fn step_arm9(cpu: &mut Arm946eS, bus: &mut NdsBus) -> u32 {
    let mut arm9_bus = Arm9Bus { bus };
    step_instruction(cpu, &mut arm9_bus)
}
