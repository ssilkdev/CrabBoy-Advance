//! ARM7TDMI Core Implementation

pub mod alu;
pub mod arm;
pub mod thumb;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuMode {
    User = 0x10,
    Fiq = 0x11,
    Irq = 0x12,
    Supervisor = 0x13,
    Abort = 0x17,
    Undefined = 0x1B,
    System = 0x1F,
}

impl CpuMode {
    pub fn from_bits(bits: u32) -> Self {
        match bits & 0x1F {
            0x10 => CpuMode::User,
            0x11 => CpuMode::Fiq,
            0x12 => CpuMode::Irq,
            0x13 => CpuMode::Supervisor,
            0x17 => CpuMode::Abort,
            0x1B => CpuMode::Undefined,
            _ => CpuMode::System, // Defaults to System for 0x1F and unrecognized
        }
    }
}

pub const FLAG_N: u32 = 1 << 31;
pub const FLAG_Z: u32 = 1 << 30;
pub const FLAG_C: u32 = 1 << 29;
pub const FLAG_V: u32 = 1 << 28;
pub const FLAG_I: u32 = 1 << 7;
pub const FLAG_F: u32 = 1 << 6;
pub const FLAG_T: u32 = 1 << 5;

#[derive(Clone)]
pub struct Arm7Tdmi {
    /// Active registers R0-R15
    pub regs: [u32; 16],
    /// Current Program Status Register
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

    /// CPU halted state (e.g. SWI Halt waiting for IRQ)
    pub halted: bool,
    /// Pending IRQ flag
    pub irq_pending: bool,
    /// Master interrupt cycle counter
    pub cycles: u64,
}

impl Default for Arm7Tdmi {
    fn default() -> Self {
        Self::new()
    }
}

impl Arm7Tdmi {
    pub fn new() -> Self {
        let mut cpu = Self {
            regs: [0; 16],
            cpsr: (CpuMode::System as u32), // Start in System mode (or Supervisor)
            r8_usr: [0; 5],
            r8_fiq: [0; 5],
            r13_usr: 0x03007F00, // SP_usr standard GBA initial stack
            r14_usr: 0,
            r13_fiq: 0,
            r14_fiq: 0,
            spsr_fiq: 0,
            r13_irq: 0x03007FA0, // SP_irq
            r14_irq: 0,
            spsr_irq: 0,
            r13_svc: 0x03007FE0, // SP_svc
            r14_svc: 0,
            spsr_svc: 0,
            r13_abt: 0,
            r14_abt: 0,
            spsr_abt: 0,
            r13_und: 0,
            r14_und: 0,
            spsr_und: 0,
            halted: false,
            irq_pending: false,
            cycles: 0,
        };
        cpu.regs[13] = 0x03007F00;
        cpu
    }

    #[inline(always)]
    pub fn get_mode(&self) -> CpuMode {
        CpuMode::from_bits(self.cpsr)
    }

    pub fn set_mode(&mut self, new_mode: CpuMode) {
        let old_mode = self.get_mode();
        if old_mode == new_mode {
            self.cpsr = (self.cpsr & !0x1F) | (new_mode as u32);
            return;
        }

        // Save current banked registers
        match old_mode {
            CpuMode::User | CpuMode::System => {
                self.r8_usr.copy_from_slice(&self.regs[8..13]);
                self.r13_usr = self.regs[13];
                self.r14_usr = self.regs[14];
            }
            CpuMode::Fiq => {
                self.r8_fiq.copy_from_slice(&self.regs[8..13]);
                self.r13_fiq = self.regs[13];
                self.r14_fiq = self.regs[14];
            }
            CpuMode::Irq => {
                self.r8_usr.copy_from_slice(&self.regs[8..13]);
                self.r13_irq = self.regs[13];
                self.r14_irq = self.regs[14];
            }
            CpuMode::Supervisor => {
                self.r8_usr.copy_from_slice(&self.regs[8..13]);
                self.r13_svc = self.regs[13];
                self.r14_svc = self.regs[14];
            }
            CpuMode::Abort => {
                self.r8_usr.copy_from_slice(&self.regs[8..13]);
                self.r13_abt = self.regs[13];
                self.r14_abt = self.regs[14];
            }
            CpuMode::Undefined => {
                self.r8_usr.copy_from_slice(&self.regs[8..13]);
                self.r13_und = self.regs[13];
                self.r14_und = self.regs[14];
            }
        }

        // Load new banked registers
        match new_mode {
            CpuMode::User | CpuMode::System => {
                self.regs[8..13].copy_from_slice(&self.r8_usr);
                self.regs[13] = self.r13_usr;
                self.regs[14] = self.r14_usr;
            }
            CpuMode::Fiq => {
                self.regs[8..13].copy_from_slice(&self.r8_fiq);
                self.regs[13] = self.r13_fiq;
                self.regs[14] = self.r14_fiq;
            }
            CpuMode::Irq => {
                self.regs[8..13].copy_from_slice(&self.r8_usr);
                self.regs[13] = self.r13_irq;
                self.regs[14] = self.r14_irq;
            }
            CpuMode::Supervisor => {
                self.regs[8..13].copy_from_slice(&self.r8_usr);
                self.regs[13] = self.r13_svc;
                self.regs[14] = self.r14_svc;
            }
            CpuMode::Abort => {
                self.regs[8..13].copy_from_slice(&self.r8_usr);
                self.regs[13] = self.r13_abt;
                self.regs[14] = self.r14_abt;
            }
            CpuMode::Undefined => {
                self.regs[8..13].copy_from_slice(&self.r8_usr);
                self.regs[13] = self.r13_und;
                self.regs[14] = self.r14_und;
            }
        }

        self.cpsr = (self.cpsr & !0x1F) | (new_mode as u32);
    }

    pub fn set_cpsr(&mut self, new_cpsr: u32) {
        let new_mode = CpuMode::from_bits(new_cpsr);
        self.set_mode(new_mode);
        self.cpsr = new_cpsr;
    }

    #[inline(always)]
    pub fn get_spsr(&self) -> u32 {
        match self.get_mode() {
            CpuMode::Fiq => self.spsr_fiq,
            CpuMode::Irq => self.spsr_irq,
            CpuMode::Supervisor => self.spsr_svc,
            CpuMode::Abort => self.spsr_abt,
            CpuMode::Undefined => self.spsr_und,
            _ => self.cpsr,
        }
    }

    #[inline]
    pub fn set_spsr(&mut self, val: u32) {
        match self.get_mode() {
            CpuMode::Fiq => self.spsr_fiq = val,
            CpuMode::Irq => self.spsr_irq = val,
            CpuMode::Supervisor => self.spsr_svc = val,
            CpuMode::Abort => self.spsr_abt = val,
            CpuMode::Undefined => self.spsr_und = val,
            _ => {}
        }
    }

    #[inline(always)]
    pub fn is_thumb(&self) -> bool {
        (self.cpsr & FLAG_T) != 0
    }

    #[inline(always)]
    pub fn set_flag(&mut self, flag: u32, set: bool) {
        if set {
            self.cpsr |= flag;
        } else {
            self.cpsr &= !flag;
        }
    }

    #[inline(always)]
    pub fn get_flag(&self, flag: u32) -> bool {
        (self.cpsr & flag) != 0
    }

    #[inline(always)]
    pub fn check_condition(&self, cond: u32) -> bool {
        match cond {
            0 => self.get_flag(FLAG_Z),                                            // EQ
            1 => !self.get_flag(FLAG_Z),                                           // NE
            2 => self.get_flag(FLAG_C),                                            // CS/HS
            3 => !self.get_flag(FLAG_C),                                           // CC/LO
            4 => self.get_flag(FLAG_N),                                            // MI
            5 => !self.get_flag(FLAG_N),                                           // PL
            6 => self.get_flag(FLAG_V),                                            // VS
            7 => !self.get_flag(FLAG_V),                                           // VC
            8 => self.get_flag(FLAG_C) && !self.get_flag(FLAG_Z),                  // HI
            9 => !self.get_flag(FLAG_C) || self.get_flag(FLAG_Z),                  // LS
            10 => self.get_flag(FLAG_N) == self.get_flag(FLAG_V),                  // GE
            11 => self.get_flag(FLAG_N) != self.get_flag(FLAG_V),                  // LT
            12 => !self.get_flag(FLAG_Z) && (self.get_flag(FLAG_N) == self.get_flag(FLAG_V)), // GT
            13 => self.get_flag(FLAG_Z) || (self.get_flag(FLAG_N) != self.get_flag(FLAG_V)),  // LE
            14 => true,                                                             // AL
            15 => true,                                                             // NV / GBA allows NV as AL or undefined
            _ => false,
        }
    }

    /// Trigger hardware IRQ exception (vector 0x00000018)
    pub fn trigger_irq(&mut self) {
        if self.get_flag(FLAG_I) {
            return; // IRQ disabled
        }
        self.halted = false;
        let return_pc = self.regs[15].wrapping_add(4);
        let old_cpsr = self.cpsr;
        self.set_mode(CpuMode::Irq);
        self.spsr_irq = old_cpsr;
        self.regs[14] = return_pc; // LR = return address
        self.set_flag(FLAG_I, true); // Disable further IRQs
        self.set_flag(FLAG_T, false); // Switch to ARM state
        self.regs[15] = 0x0000_0018; // Vector address
    }

    /// Trigger software interrupt (SWI) exception (vector 0x00000008)
    pub fn trigger_swi(&mut self, comment: u32) {
        // PC already points past the SWI instruction (incremented in step_arm/step_thumb),
        // so it is already the correct return address for LR_svc.
        let return_pc = self.regs[15];
        let old_cpsr = self.cpsr;
        self.set_mode(CpuMode::Supervisor);
        self.spsr_svc = old_cpsr;
        self.regs[14] = return_pc;
        self.set_flag(FLAG_I, true);
        self.set_flag(FLAG_T, false);
        self.regs[15] = 0x0000_0008;
        let _ = comment;
    }
}
