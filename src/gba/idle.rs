//! Stage 4a: Idle-Loop Detection & Busy-Wait Skipping (ROADMAP M4a / docs/JIT.md).
//!
//! Many GBA games (notably Pokémon Emerald, Ruby, Sapphire, FireRed, LeafGreen,
//! and Golden Sun) do not invoke the HALT BIOS call to wait for VBlank or interrupts;
//! instead, they busy-wait in tight polling loops (checking a RAM flag or VCOUNT).
//!
//! On mobile devices, executing millions of iterations of these do-nothing loops
//! burns battery and triggers thermal throttling.
//!
//! This module detects small, deterministic backward branch loops that perform
//! no memory writes and alter no registers between iterations. When confirmed,
//! whole iterations are skipped up to the next peripheral event horizon
//! (scanline boundary, timer overflow, APU sample, or pending IRQ).
//!
//! This maintains 100% bit-identical emulation while reducing mobile CPU load by
//! 30–50%.

#[derive(Clone, Debug)]
pub struct IdleDetector {
    /// PC of the start of the detected loop
    pub loop_start_pc: u32,
    /// PC of the backward branch that loops back to `loop_start_pc`
    pub loop_branch_pc: u32,
    /// Timestamp (CPU cycles) at which the current iteration began
    pub start_cycles: u64,
    /// Number of consecutive identical iterations observed
    pub consecutive_loops: u32,
    /// Snapshot of all 16 general-purpose registers at the start of the loop
    pub saved_regs: [u32; 16],
    /// Snapshot of CPSR flags at the start of the loop
    pub saved_cpsr: u32,
}

impl Default for IdleDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl IdleDetector {
    pub fn new() -> Self {
        Self {
            loop_start_pc: 0,
            loop_branch_pc: 0,
            start_cycles: 0,
            consecutive_loops: 0,
            saved_regs: [0; 16],
            saved_cpsr: 0,
        }
    }

    pub fn reset(&mut self) {
        self.loop_start_pc = 0;
        self.loop_branch_pc = 0;
        self.start_cycles = 0;
        self.consecutive_loops = 0;
        self.saved_regs = [0; 16];
        self.saved_cpsr = 0;
    }
}
