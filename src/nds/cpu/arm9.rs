//! ARM946E-S Core Implementation (ARMv5TE + CP15 + TCM)
//!
//! Main CPU of the Nintendo DS, running at 67.028 MHz.

use crate::gba::cpu::CpuMode;

pub const FLAG_N: u32 = 1 << 31;
pub const FLAG_Z: u32 = 1 << 30;
pub const FLAG_C: u32 = 1 << 29;
pub const FLAG_V: u32 = 1 << 28;
pub const FLAG_Q: u32 = 1 << 27; // Sticky saturation flag (ARMv5TE)
pub const FLAG_I: u32 = 1 << 7;
pub const FLAG_F: u32 = 1 << 6;
pub const FLAG_T: u32 = 1 << 5;

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

    // Tightly Coupled Memory (0-waitstate)
    pub itcm: Box<[u8; 0x8000]>, // 32 KB
    pub dtcm: Box<[u8; 0x4000]>, // 16 KB

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
            r13_usr: 0,
            r14_usr: 0,
            r13_fiq: 0,
            r14_fiq: 0,
            spsr_fiq: 0,
            r13_irq: 0,
            r14_irq: 0,
            spsr_irq: 0,
            r13_svc: 0,
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

            itcm: vec![0u8; 0x8000].into_boxed_slice().try_into().unwrap(),
            dtcm: vec![0u8; 0x4000].into_boxed_slice().try_into().unwrap(),
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

    /// Read/Write CP15 Coprocessor (MCR / MRC)
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
            _ => {
                log::trace!("CP15 MRC read crn={crn} crm={crm} op1={op1} op2={op2}");
                0
            }
        }
    }

    /// ARMv5TE CLZ (Count Leading Zeros)
    #[inline]
    pub fn op_clz(&mut self, rd: usize, rm: usize) {
        let val = self.regs[rm];
        self.regs[rd] = val.leading_zeros();
    }

    /// ARMv5TE QADD (Saturating Add)
    #[inline]
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

    /// ARMv5TE QSUB (Saturating Subtract)
    #[inline]
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

    /// ARMv5TE SMULxy (Signed Halfword Multiply: 16x16 -> 32)
    #[inline]
    pub fn op_smulxy(&mut self, rd: usize, rm: usize, rs: usize, x_top: bool, y_top: bool) {
        let a = if x_top { (self.regs[rm] >> 16) as i16 } else { self.regs[rm] as i16 } as i32;
        let b = if y_top { (self.regs[rs] >> 16) as i16 } else { self.regs[rs] as i16 } as i32;
        self.regs[rd] = (a * b) as u32;
    }

    /// ARMv5TE SMLAxy (Signed Halfword Multiply and Accumulate: 16x16 + 32 -> 32)
    #[inline]
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

    /// ARMv5TE BLX (Branch with Link and Exchange)
    #[inline]
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
