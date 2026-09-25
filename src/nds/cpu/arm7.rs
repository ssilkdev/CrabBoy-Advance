//! ARM7TDMI Sub-Processor Wrapper and Execution for Nintendo DS
//!
//! Reuses the battle-tested ARM7TDMI core from CrabBoy Advance,
//! running at 33.514 MHz.

pub use crate::gba::cpu::Arm7Tdmi;

use crate::nds::bus::NdsBus;
use crate::nds::cpu::executor::{step_instruction, CpuBus, CpuState};

pub struct Arm7Bus<'a> {
    pub bus: &'a mut NdsBus,
}

impl<'a> CpuBus for Arm7Bus<'a> {
    #[inline(always)]
    fn read8(&mut self, addr: u32) -> u8 {
        self.bus.read_arm7_u8(addr)
    }

    #[inline(always)]
    fn read16(&mut self, addr: u32) -> u16 {
        self.bus.read_arm7_u16(addr)
    }

    #[inline(always)]
    fn read32(&mut self, addr: u32) -> u32 {
        self.bus.read_arm7_u32(addr)
    }

    #[inline(always)]
    fn write8(&mut self, addr: u32, val: u8) {
        self.bus.write_arm7_u8(addr, val);
    }

    #[inline(always)]
    fn write16(&mut self, addr: u32, val: u16) {
        self.bus.write_arm7_u16(addr, val);
    }

    #[inline(always)]
    fn write32(&mut self, addr: u32, val: u32) {
        self.bus.write_arm7_u32(addr, val);
    }
}

pub struct Arm7Adapter<'a> {
    pub cpu: &'a mut Arm7Tdmi,
}

impl<'a> CpuState for Arm7Adapter<'a> {
    #[inline(always)]
    fn reg(&self, idx: usize) -> u32 {
        self.cpu.regs[idx]
    }

    #[inline(always)]
    fn set_reg(&mut self, idx: usize, val: u32) {
        self.cpu.regs[idx] = val;
    }

    #[inline(always)]
    fn cpsr(&self) -> u32 {
        self.cpu.cpsr
    }

    #[inline(always)]
    fn set_cpsr(&mut self, val: u32) {
        self.cpu.set_cpsr(val);
    }

    #[inline(always)]
    fn get_spsr(&self) -> u32 {
        self.cpu.get_spsr()
    }

    #[inline(always)]
    fn set_spsr(&mut self, val: u32) {
        self.cpu.set_spsr(val);
    }

    #[inline(always)]
    fn is_thumb(&self) -> bool {
        self.cpu.is_thumb()
    }

    #[inline(always)]
    fn is_halted(&self) -> bool {
        self.cpu.halted
    }

    #[inline(always)]
    fn set_halted(&mut self, halted: bool) {
        self.cpu.halted = halted;
    }

    #[inline(always)]
    fn check_condition(&self, cond: u32) -> bool {
        self.cpu.check_condition(cond)
    }

    #[inline(always)]
    fn trigger_irq(&mut self) {
        self.cpu.trigger_irq();
    }

    #[inline(always)]
    fn trigger_swi(&mut self, comment: u32) {
        self.cpu.trigger_swi(comment);
    }

    #[inline(always)]
    fn is_armv5(&self) -> bool {
        false
    }

    #[inline(always)]
    fn mcr(&mut self, _crn: u32, _crm: u32, _op1: u32, _op2: u32, _val: u32) {}

    #[inline(always)]
    fn mrc(&self, _crn: u32, _crm: u32, _op1: u32, _op2: u32) -> u32 {
        0
    }
}

/// Execute one instruction on the ARM7 sub-processor
pub fn step_arm7(cpu: &mut Arm7Tdmi, bus: &mut NdsBus) -> u32 {
    let mut adapter = Arm7Adapter { cpu };
    let mut arm7_bus = Arm7Bus { bus };
    step_instruction(&mut adapter, &mut arm7_bus)
}
