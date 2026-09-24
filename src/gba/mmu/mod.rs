//! GBA Memory Management Unit (MMU) & System Bus Dispatcher

pub mod bios;
pub mod bios_math;
pub mod cartridge;
pub mod debug_port;
pub mod eeprom;
pub mod flash;
pub mod rtc;
pub mod save_backend;
pub mod sensors;
pub mod sio;
pub mod sram;
pub mod timing;

use super::apu::Apu;
use super::cpu::Arm7Tdmi;
use super::dma::DmaController;
use super::keypad::Keypad;
use super::ppu::Ppu;
use super::timer::TimerController;
use crate::gba::diagnostics::FlightRecorder;
use cartridge::Cartridge;
use save_backend::{SaveBackend, SaveType};
use sio::Sio;

/// A timer counter read returns the value latched this many cycles before
/// the access completes (the read takes 1N + 1I). Matches mGBA and the
/// suite's Timer IRQ expectations.
const TIMER_READ_LATENCY: u64 = 2;

pub struct Mmu {
    pub bios: Box<[u8; 16 * 1024]>,
    pub ewram: Box<[u8; 256 * 1024]>,
    pub iwram: Box<[u8; 32 * 1024]>,
    pub io_regs: Box<[u8; 1024]>,

    pub cartridge: Option<Cartridge>,
    pub ppu: Ppu,
    pub dma: DmaController,
    pub timers: TimerController,
    pub apu: Apu,
    pub keypad: Keypad,
    pub sio: Sio,

    pub ime: bool,
    pub ie: u16,
    pub if_reg: u16,
    pub waitcnt: u16,
    pub post_flg: u8,
    pub haltcnt: u8,

    /// Open-bus value: what a read from unmapped memory or a write-only IO
    /// register returns. On the ARM7TDMI this is the most recently
    /// prefetched opcode, which the CPU updates before each instruction
    /// (see `set_open_bus_*` in cpu/arm.rs and cpu/thumb.rs). ROADMAP M1.
    pub open_bus: u32,

    pub flight_recorder: FlightRecorder,
    pub current_pc: u32,
    /// BIOS open-bus latch: the last opcode fetched from BIOS (as seen at
    /// the fetch stage, i.e. PC+8). BIOS memory is read-protected, so a read
    /// from BIOS while executing outside it returns this value instead.
    /// Values follow the real BIOS (jsmolka bios.gba): 0xE129F000 after boot,
    /// 0xE3A02004 after an SWI, 0xE25EF004 inside an IRQ handler and
    /// 0xE55EC002 after returning from one. ROADMAP M1.
    pub bios_latch: u32,
    /// Wait-state accounting for CPU-visible accesses (see `timing`).
    pub timing: timing::BusTiming,
    /// Cycles the CPU was stalled (by DMA, or by time spent inside an HLE
    /// BIOS call) since the last `take_dma_stall`.
    pub dma_stall: u32,
    /// `timing.clock` at the start of the current instruction; with
    /// `current_cycles` this gives the exact cycle of an access mid-
    /// instruction (`now`). Set by `Gba::step_instruction`.
    pub instr_clock_start: u64,
    /// Cycle at which the IRQ line (IE & IF != 0) became asserted, if it
    /// is. The CPU takes the IRQ a few cycles later (see `IRQ_DELAY`).
    pub irq_assert_time: Option<u64>,
    /// An IRQ has been dispatched since the current IntrWait began.
    pub intr_wait_dispatched: bool,
    /// mGBA-compatible debug-print port at 0x04FFF600 (see `debug_port`).
    pub debug_port: debug_port::DebugPort,
    pub current_cycles: u64,
    pub intr_wait_mask: Option<u16>,
}

impl Default for Mmu {
    fn default() -> Self {
        Self::new()
    }
}

/// BIOS open-bus values of the real GBA BIOS at well-known points.
pub const BIOS_LATCH_BOOT: u32 = 0xE129_F000;
pub const BIOS_LATCH_AFTER_SWI: u32 = 0xE3A0_2004;
pub const BIOS_LATCH_IN_IRQ: u32 = 0xE25E_F004;
pub const BIOS_LATCH_AFTER_IRQ: u32 = 0xE55E_C002;

impl Mmu {
    pub fn new() -> Self {
        let mut bios = Box::new([0u8; 16 * 1024]);

        // Undefined-instruction vector at 0x00000004.
        //
        // `arm.rs` vectors here for any unrecognized encoding, but this area
        // used to be left as zeros -- which decode as `andeq r0,r0,r0`, i.e. a
        // fall-through. The CPU would then walk 0x04, 0x08, 0x0C ... up through
        // empty BIOS and off into IO space (0x04xxxxxx), producing a hung game
        // whose diagnostic report showed PC inside the IO registers.
        //
        // Real hardware returns from the exception. `subs pc, lr, #4` restores
        // CPSR from SPSR_und and resumes at the faulting instruction's
        // successor, which keeps a bad opcode local instead of fatal.
        bios[0x04..0x08].copy_from_slice(&0xE25E_F004u32.to_le_bytes()); // subs pc, lr, #4

        // Note: the SWI vector (0x08) is deliberately NOT stubbed. SWIs are
        // intercepted in arm.rs/thumb.rs and serviced by the HLE BIOS
        // (`handle_swi`), so the CPU never actually vectors to 0x08. Writing
        // a stub there would be dead code that implies otherwise.

        // GBA BIOS IRQ Vector & Dispatcher at 0x0000_0018
        // When an interrupt fires, CPU vectors to 0x18 in IRQ mode.
        // The BIOS saves registers, jumps to [0x03007FFC] (user handler),
        // and on return restores registers and returns via subs pc, lr, #4.
        let irq_code: [u32; 6] = [
            0xE92D_500F, // 0x18: stmfd sp!, {r0-r3, r12, lr}
            0xE3A0_0301, // 0x1C: mov r0, #0x04000000
            0xE28F_E000, // 0x20: add lr, pc, #0
            0xE510_F004, // 0x24: ldr pc, [r0, #-4] (reads [0x03007FFC])
            0xE8BD_500F, // 0x28: ldmfd sp!, {r0-r3, r12, lr}
            0xE25E_F004, // 0x2C: subs pc, lr, #4
        ];
        for (i, &instr) in irq_code.iter().enumerate() {
            let offset = 0x18 + i * 4;
            bios[offset..offset + 4].copy_from_slice(&instr.to_le_bytes());
        }

        Self {
            bios,
            ewram: vec![0u8; 256 * 1024].into_boxed_slice().try_into().unwrap(),
            iwram: vec![0u8; 32 * 1024].into_boxed_slice().try_into().unwrap(),
            io_regs: Box::new([0; 1024]),
            cartridge: None,
            ppu: Ppu::new(),
            dma: DmaController::new(),
            timers: TimerController::new(),
            apu: Apu::new(),
            keypad: Keypad::new(),
            sio: Sio::new(),
            ime: false,
            ie: 0,
            if_reg: 0,
            waitcnt: 0,
            post_flg: 0,
            haltcnt: 0,
            open_bus: 0,
            flight_recorder: FlightRecorder::new(),
            current_pc: 0,
            bios_latch: BIOS_LATCH_BOOT,
            debug_port: debug_port::DebugPort::new(),
            timing: timing::BusTiming::default(),
            dma_stall: 0,
            instr_clock_start: 0,
            irq_assert_time: None,
            intr_wait_dispatched: false,
            current_cycles: 0,
            intr_wait_mask: None,
        }
    }

    pub fn load_cartridge(&mut self, cart: Cartridge) {
        self.cartridge = Some(cart);
    }

    // ---- CPU-visible accessors: raw access plus wait-state accounting ----
    //
    // `*_raw` perform the access without timing (debuggers, cheats and
    // internal helpers may use them freely); the plain names are what the
    // CPU and DMA use. `fetch*` mark opcode fetches, which the cartridge
    // prefetch buffer can serve. ROADMAP M1.

    #[inline(always)]
    pub fn read8(&self, addr: u32) -> u8 {
        self.timing.access(addr, timing::Width::Byte, false);
        self.read8_raw(addr)
    }

    #[inline(always)]
    pub fn read16(&self, addr: u32) -> u16 {
        self.timing.access(addr & !1, timing::Width::Half, false);
        self.read16_raw(addr)
    }

    #[inline(always)]
    pub fn read32(&self, addr: u32) -> u32 {
        self.timing.access(addr & !3, timing::Width::Word, false);
        self.read32_raw(addr)
    }

    #[inline(always)]
    pub fn fetch16(&self, addr: u32) -> u16 {
        self.timing.access(addr & !1, timing::Width::Half, true);
        self.read16_raw(addr)
    }

    #[inline(always)]
    pub fn fetch32(&self, addr: u32) -> u32 {
        self.timing.access(addr & !3, timing::Width::Word, true);
        self.read32_raw(addr)
    }

    #[inline(always)]
    pub fn write8(&mut self, addr: u32, val: u8) {
        self.timing.access(addr, timing::Width::Byte, false);
        self.write8_raw(addr, val);
    }

    #[inline(always)]
    pub fn write16(&mut self, addr: u32, val: u16) {
        self.timing.access(addr & !1, timing::Width::Half, false);
        self.write16_raw(addr, val);
    }

    #[inline(always)]
    pub fn write32(&mut self, addr: u32, val: u32) {
        self.timing.access(addr & !3, timing::Width::Word, false);
        self.write32_raw(addr, val);
    }

    #[inline(always)]
    pub fn read8_raw(&self, addr: u32) -> u8 {
        match (addr >> 24) & 0xFF {
            0x00 if addr < 0x4000 => {
                if self.current_pc >= 0x4000 {
                    // Read-protected: open bus returns the latched opcode.
                    return (self.bios_latch >> ((addr & 3) * 8)) as u8;
                }
                self.bios[addr as usize]
            }
            0x02 => {
                let off = (addr & 0x3_FFFF) as usize;
                self.ewram[off]
            }
            0x03 => {
                let off = (addr & 0x7FFF) as usize;
                self.iwram[off]
            }
            0x04 if debug_port::DebugPort::contains(addr) => self.debug_port.read8(addr),
            // Only 0x04000000-0x040003FF holds IO registers (plus the
            // memory-control mirror at 0x0400_0800, which reads as open bus
            // on the GBA); the rest of the region is unmapped.
            0x04 if (addr & 0x00FF_FFFF) >= 0x400 => self.open_bus8(addr),
            0x04 => self.read_io8(addr),
            0x05 => {
                let off = (addr & 0x3FF) as usize;
                self.ppu.palette_ram[off]
            }
            0x06 => {
                let mut off = (addr & 0x1_FFFF) as usize;
                if off >= 0x18000 {
                    off -= 0x8000;
                }
                self.ppu.vram[off]
            }
            0x07 => {
                let off = (addr & 0x3FF) as usize;
                self.ppu.oam[off]
            }
            0x08..=0x0F => {
                if let Some(ref cart) = self.cartridge {
                    cart.read8(addr)
                } else {
                    0
                }
            }
            _ => self.open_bus8(addr),
        }
    }

    /// The open-bus byte for `addr`: the lane of the latched opcode word.
    #[inline(always)]
    fn open_bus8(&self, addr: u32) -> u8 {
        (self.open_bus >> ((addr & 3) * 8)) as u8
    }

    /// True for the SRAM/Flash save region (0x0E000000-0x0FFFFFFF), which
    /// sits on an 8-bit bus.
    #[inline(always)]
    fn is_save_bus(addr: u32) -> bool {
        (addr >> 25) == 0x07
    }

    #[inline(always)]
    pub fn read16_raw(&self, addr: u32) -> u16 {
        // 8-bit save bus: the one byte read appears in every lane
        // (jsmolka save tests #4).
        if Self::is_save_bus(addr) {
            return self.read8_raw(addr) as u16 * 0x0101;
        }
        // Plain aligned bus read. The CPU-visible rotation for misaligned
        // LDRH is 32-bit, so it lives in cpu::load_halfword, not here.
        let aligned = addr & !1;
        let b0 = self.read8_raw(aligned) as u16;
        let b1 = self.read8_raw(aligned + 1) as u16;
        b0 | (b1 << 8)
    }

    #[inline(always)]
    pub fn read32_raw(&self, addr: u32) -> u32 {
        if Self::is_save_bus(addr) {
            return self.read8_raw(addr) as u32 * 0x0101_0101;
        }
        let aligned = addr & !3;
        let b0 = self.read8_raw(aligned) as u32;
        let b1 = self.read8_raw(aligned + 1) as u32;
        let b2 = self.read8_raw(aligned + 2) as u32;
        let b3 = self.read8_raw(aligned + 3) as u32;
        let val = b0 | (b1 << 8) | (b2 << 16) | (b3 << 24);

        let unaligned_offset = (addr & 3) * 8;
        if unaligned_offset != 0 {
            val.rotate_right(unaligned_offset)
        } else {
            val
        }
    }

    #[inline(always)]
    pub fn write8_raw(&mut self, addr: u32, val: u8) {
        match (addr >> 24) & 0xFF {
            0x02 => {
                let off = (addr & 0x3_FFFF) as usize;
                self.ewram[off] = val;
            }
            0x03 => {
                let off = (addr & 0x7FFF) as usize;
                self.iwram[off] = val;
            }
            0x04 if debug_port::DebugPort::contains(addr) => self.debug_port.write8(addr, val),
            0x04 if (addr & 0x00FF_FFFF) >= 0x400 => {} // unmapped (see read8)
            0x04 => self.write_io8(addr, val),
            0x05 => {
                // Byte writes to palette RAM write to both bytes of the halfword
                let off = (addr & 0x3FE) as usize;
                self.ppu.palette_ram[off] = val;
                self.ppu.palette_ram[off + 1] = val;
            }
            0x06 => {
                let off_raw = addr & 0x1_FFFF;
                // On GBA, 8-bit writes to OBJ VRAM (>= 0x10000) are ignored
                if off_raw >= 0x10000 {
                    return;
                }
                // Byte writes to BG VRAM write to both bytes of the halfword
                let mut off = (addr & 0x1_FFFE) as usize;
                if off >= 0x18000 {
                    off -= 0x8000;
                }
                self.ppu.vram[off] = val;
                self.ppu.vram[off + 1] = val;
            }
            0x07 => {
                // OAM byte writes are ignored on real GBA hardware
            }
            0x08..=0x0F => {
                if let Some(ref mut cart) = self.cartridge {
                    cart.write8(addr, val);
                }
            }
            _ => {}
        }
    }

    #[inline(always)]
    pub fn write16_raw(&mut self, addr: u32, val: u16) {
        // 8-bit save bus: only the byte lane selected by the unaligned
        // address is written, at that address (jsmolka save tests #6/#7).
        if Self::is_save_bus(addr) {
            self.write8_raw(addr, (val >> ((addr & 1) * 8)) as u8);
            return;
        }
        let aligned = addr & !1;
        match (aligned >> 24) & 0xFF {
            0x04 if debug_port::DebugPort::contains(aligned) => self.debug_port.write16(aligned, val),
            0x04 if (aligned & 0x00FF_FFFF) >= 0x400 => {} // unmapped (see read8)
            0x04 => self.write_io16(aligned, val),
            0x05 => {
                let off = (aligned & 0x3FE) as usize;
                self.ppu.palette_ram[off] = (val & 0xFF) as u8;
                self.ppu.palette_ram[off + 1] = (val >> 8) as u8;
            }
            0x06 => {
                let mut off = (aligned & 0x1_FFFE) as usize;
                if off >= 0x18000 {
                    off -= 0x8000;
                }
                self.ppu.vram[off] = (val & 0xFF) as u8;
                self.ppu.vram[off + 1] = (val >> 8) as u8;
            }
            0x07 => {
                let off = (aligned & 0x3FE) as usize;
                self.ppu.oam[off] = (val & 0xFF) as u8;
                self.ppu.oam[off + 1] = (val >> 8) as u8;
            }
            _ => {
                self.write8_raw(aligned, (val & 0xFF) as u8);
                self.write8_raw(aligned + 1, (val >> 8) as u8);
            }
        }
    }

    #[inline(always)]
    pub fn write32_raw(&mut self, addr: u32, val: u32) {
        if Self::is_save_bus(addr) {
            self.write8_raw(addr, (val >> ((addr & 3) * 8)) as u8);
            return;
        }
        let aligned = addr & !3;
        self.write16_raw(aligned, (val & 0xFFFF) as u16);
        self.write16_raw(aligned + 2, (val >> 16) as u16);
    }

    fn read_io8(&self, addr: u32) -> u8 {
        let off = addr & 0x3FF;
        match off {
            // Readable sound registers; the write-only ones (0x8C-0x8F and
            // the FIFOs 0xA0-0xA7) fall through to read_io16's open bus.
            0x060..=0x08B | 0x090..=0x09F => self.apu.read_reg8(off),
            _ => {
                let is_high = (off & 1) != 0;
                let val16 = self.read_io16(off & !1);
                if is_high {
                    (val16 >> 8) as u8
                } else {
                    (val16 & 0xFF) as u8
                }
            }
        }
    }

    fn read_io16(&self, addr: u32) -> u16 {
        let off = addr & 0x3FE;
        // Readback rules, verified against mGBA's "I/O read" suite
        // (ROADMAP M1):
        match off {
            // Write-only registers read as open bus: BG scroll/affine,
            // windows, mosaic, BLDY, sound FIFOs, DMA addresses, and the
            // unused gaps between them.
            0x010..=0x046 | 0x04C..=0x04E | 0x054..=0x05E | 0x08C | 0x08E
            | 0x0A0..=0x0B6 | 0x0BC..=0x0C2 | 0x0C8..=0x0CE | 0x0D4..=0x0DA
            | 0x0E0..=0x0FE => return (self.open_bus >> ((addr & 2) * 8)) as u16,
            // Write-only DMA word counts read as zero.
            0x0B8 | 0x0C4 | 0x0D0 | 0x0DC => return 0,
            // Unused registers in the 0x100+ area read as zero.
            0x136 | 0x138..=0x13E | 0x142..=0x14E | 0x15A..=0x1FE | 0x206
            | 0x20A..=0x2FE | 0x302..=0x3FE => return 0,
            _ => {}
        }
        match off {
            0x000 => self.ppu.dispcnt,
            0x004 => self.ppu.dispstat,
            0x006 => self.ppu.vcount,
            // BG0/BG1 have no display-overflow bit (13).
            0x008 => self.ppu.bgcnt[0] & 0xDFFF,
            0x00A => self.ppu.bgcnt[1] & 0xDFFF,
            0x00C => self.ppu.bgcnt[2],
            0x00E => self.ppu.bgcnt[3],
            0x048 => self.ppu.winin & 0x3F3F,
            0x04A => self.ppu.winout & 0x3F3F,
            0x04C => self.ppu.mosaic,
            0x050 => self.ppu.bldcnt & 0x3FFF,
            0x052 => self.ppu.bldalpha & 0x1F1F,
            0x060..=0x0A6 => self.apu.read_reg16(addr),
            0x0B0 => self.dma.channels[0].sad as u16,
            0x0B2 => (self.dma.channels[0].sad >> 16) as u16,
            0x0B4 => self.dma.channels[0].dad as u16,
            0x0B6 => (self.dma.channels[0].dad >> 16) as u16,
            0x0B8 => self.dma.channels[0].count,
            0x0BA => self.dma.channels[0].cnt_h & 0xF7E0,
            0x0BC => self.dma.channels[1].sad as u16,
            0x0BE => (self.dma.channels[1].sad >> 16) as u16,
            0x0C0 => self.dma.channels[1].dad as u16,
            0x0C2 => (self.dma.channels[1].dad >> 16) as u16,
            0x0C4 => self.dma.channels[1].count,
            0x0C6 => self.dma.channels[1].cnt_h & 0xF7E0,
            0x0C8 => self.dma.channels[2].sad as u16,
            0x0CA => (self.dma.channels[2].sad >> 16) as u16,
            0x0CC => self.dma.channels[2].dad as u16,
            0x0CE => (self.dma.channels[2].dad >> 16) as u16,
            0x0D0 => self.dma.channels[2].count,
            0x0D2 => self.dma.channels[2].cnt_h & 0xF7E0,
            0x0D4 => self.dma.channels[3].sad as u16,
            0x0D6 => (self.dma.channels[3].sad >> 16) as u16,
            0x0D8 => self.dma.channels[3].dad as u16,
            0x0DA => (self.dma.channels[3].dad >> 16) as u16,
            0x0DC => self.dma.channels[3].count,
            0x0DE => self.dma.channels[3].cnt_h & 0xFFE0,
            0x100 => self.timers.read_counter(0, self.now().saturating_sub(TIMER_READ_LATENCY)),
            0x102 => self.timers.timers[0].cnt_h,
            0x104 => self.timers.read_counter(1, self.now().saturating_sub(TIMER_READ_LATENCY)),
            0x106 => self.timers.timers[1].cnt_h,
            0x108 => self.timers.read_counter(2, self.now().saturating_sub(TIMER_READ_LATENCY)),
            0x10A => self.timers.timers[2].cnt_h,
            0x10C => self.timers.read_counter(3, self.now().saturating_sub(TIMER_READ_LATENCY)),
            0x10E => self.timers.timers[3].cnt_h,
            0x120..=0x12A | 0x134 | 0x140 | 0x150..=0x158 => self.sio.read_io16(addr & 0x3FE),
            0x130 => self.keypad.read_keyinput(),
            0x132 => self.keypad.keycnt,
            0x200 => self.ie,
            0x202 => self.if_reg,
            0x204 => self.waitcnt,
            0x208 => self.ime as u16,
            0x300 => (self.post_flg as u16) | ((self.haltcnt as u16) << 8),
            _ => {
                let off = (addr & 0x3FE) as usize;
                (self.io_regs[off] as u16) | ((self.io_regs[off + 1] as u16) << 8)
            }
        }
    }

    fn write_io8(&mut self, addr: u32, val: u8) {
        let off = addr & 0x3FF;
        match off {
            0x060..=0x0A7 => {
                self.flight_recorder.record(self.current_cycles, self.current_pc, addr, val as u32, 8, true);
                self.apu.write_reg8(off, val);
            }
            0x208 => {
                self.flight_recorder.record(self.current_cycles, self.current_pc, addr, val as u32, 8, true);
                self.ime = (val & 1) != 0;
            }
            0x300 => {
                self.flight_recorder.record(self.current_cycles, self.current_pc, addr, val as u32, 8, true);
                self.post_flg = val;
            }
            0x301 => {
                self.flight_recorder.record(self.current_cycles, self.current_pc, addr, val as u32, 8, true);
                self.haltcnt = val;
            }
            _ => {
                let aligned = off & !1;
                let cur = self.read_io16(aligned);
                let new_val = if (off & 1) != 0 {
                    (cur & 0x00FF) | ((val as u16) << 8)
                } else {
                    (cur & 0xFF00) | (val as u16)
                };
                self.write_io16(aligned, new_val);
            }
        }
    }

    fn write_io16(&mut self, addr: u32, val: u16) {
        self.flight_recorder.record(self.current_cycles, self.current_pc, addr, val as u32, 16, true);
        match addr & 0x3FE {
            0x000 => self.ppu.dispcnt = val,
            0x004 => {
                // Bits 0..2 are read only in DISPSTAT
                self.ppu.dispstat = (self.ppu.dispstat & 7) | (val & !7);
            }
            0x008 => self.ppu.bgcnt[0] = val,
            0x00A => self.ppu.bgcnt[1] = val,
            0x00C => self.ppu.bgcnt[2] = val,
            0x00E => self.ppu.bgcnt[3] = val,
            0x010 => self.ppu.bghofs[0] = val & 0x1FF,
            0x012 => self.ppu.bgvofs[0] = val & 0x1FF,
            0x014 => self.ppu.bghofs[1] = val & 0x1FF,
            0x016 => self.ppu.bgvofs[1] = val & 0x1FF,
            0x018 => self.ppu.bghofs[2] = val & 0x1FF,
            0x01A => self.ppu.bgvofs[2] = val & 0x1FF,
            0x01C => self.ppu.bghofs[3] = val & 0x1FF,
            0x01E => self.ppu.bgvofs[3] = val & 0x1FF,
            0x020 => self.ppu.bg_pa[0] = val as i16,
            0x022 => self.ppu.bg_pb[0] = val as i16,
            0x024 => self.ppu.bg_pc[0] = val as i16,
            0x026 => self.ppu.bg_pd[0] = val as i16,
            0x028 => self.ppu.bg_x[0] = (self.ppu.bg_x[0] & !0xFFFF) | (val as i32),
            0x02A => {
                // BG2X high halfword: only bits 0-11 are meaningful (the reference
                // point is a 28-bit signed value); sign-extend from bit 11, not bit 15.
                let sign_ext = (((val & 0x0FFF) as i32) << 20) >> 4;
                self.ppu.bg_x[0] = (self.ppu.bg_x[0] & 0xFFFF) | sign_ext;
                self.ppu.bg_x_internal[0] = self.ppu.bg_x[0];
            }
            0x02C => self.ppu.bg_y[0] = (self.ppu.bg_y[0] & !0xFFFF) | (val as i32),
            0x02E => {
                let sign_ext = (((val & 0x0FFF) as i32) << 20) >> 4;
                self.ppu.bg_y[0] = (self.ppu.bg_y[0] & 0xFFFF) | sign_ext;
                self.ppu.bg_y_internal[0] = self.ppu.bg_y[0];
            }
            0x030 => self.ppu.bg_pa[1] = val as i16,
            0x032 => self.ppu.bg_pb[1] = val as i16,
            0x034 => self.ppu.bg_pc[1] = val as i16,
            0x036 => self.ppu.bg_pd[1] = val as i16,
            0x038 => self.ppu.bg_x[1] = (self.ppu.bg_x[1] & !0xFFFF) | (val as i32),
            0x03A => {
                let sign_ext = ((val as i16) as i32) << 16;
                self.ppu.bg_x[1] = (self.ppu.bg_x[1] & 0xFFFF) | sign_ext;
                self.ppu.bg_x_internal[1] = self.ppu.bg_x[1];
            }
            0x03C => self.ppu.bg_y[1] = (self.ppu.bg_y[1] & !0xFFFF) | (val as i32),
            0x03E => {
                let sign_ext = ((val as i16) as i32) << 16;
                self.ppu.bg_y[1] = (self.ppu.bg_y[1] & 0xFFFF) | sign_ext;
                self.ppu.bg_y_internal[1] = self.ppu.bg_y[1];
            }
            0x040 => self.ppu.win0h = val,
            0x042 => self.ppu.win1h = val,
            0x044 => self.ppu.win0v = val,
            0x046 => self.ppu.win1v = val,
            0x048 => self.ppu.winin = val,
            0x04A => self.ppu.winout = val,
            0x04C => self.ppu.mosaic = val,
            0x050 => self.ppu.bldcnt = val,
            0x052 => self.ppu.bldalpha = val,
            0x054 => self.ppu.bldy = val,
            0x060..=0x0A6 => self.apu.write_reg16(addr, val),
            0x0B0 => self.dma.channels[0].sad = (self.dma.channels[0].sad & !0xFFFF) | (val as u32),
            0x0B2 => self.dma.channels[0].sad = (self.dma.channels[0].sad & 0xFFFF) | ((val as u32) << 16),
            0x0B4 => self.dma.channels[0].dad = (self.dma.channels[0].dad & !0xFFFF) | (val as u32),
            0x0B6 => self.dma.channels[0].dad = (self.dma.channels[0].dad & 0xFFFF) | ((val as u32) << 16),
            0x0B8 => self.dma.channels[0].count = val,
            0x0BA => {
                if self.dma.channels[0].write_cnt_h(val, false) {
                    self.execute_dma_channel(0);
                }
            }
            0x0BC => self.dma.channels[1].sad = (self.dma.channels[1].sad & !0xFFFF) | (val as u32),
            0x0BE => self.dma.channels[1].sad = (self.dma.channels[1].sad & 0xFFFF) | ((val as u32) << 16),
            0x0C0 => self.dma.channels[1].dad = (self.dma.channels[1].dad & !0xFFFF) | (val as u32),
            0x0C2 => self.dma.channels[1].dad = (self.dma.channels[1].dad & 0xFFFF) | ((val as u32) << 16),
            0x0C4 => self.dma.channels[1].count = val,
            0x0C6 => {
                if self.dma.channels[1].write_cnt_h(val, false) {
                    self.execute_dma_channel(1);
                }
            }
            0x0C8 => self.dma.channels[2].sad = (self.dma.channels[2].sad & !0xFFFF) | (val as u32),
            0x0CA => self.dma.channels[2].sad = (self.dma.channels[2].sad & 0xFFFF) | ((val as u32) << 16),
            0x0CC => self.dma.channels[2].dad = (self.dma.channels[2].dad & !0xFFFF) | (val as u32),
            0x0CE => self.dma.channels[2].dad = (self.dma.channels[2].dad & 0xFFFF) | ((val as u32) << 16),
            0x0D0 => self.dma.channels[2].count = val,
            0x0D2 => {
                if self.dma.channels[2].write_cnt_h(val, false) {
                    self.execute_dma_channel(2);
                }
            }
            0x0D4 => self.dma.channels[3].sad = (self.dma.channels[3].sad & !0xFFFF) | (val as u32),
            0x0D6 => self.dma.channels[3].sad = (self.dma.channels[3].sad & 0xFFFF) | ((val as u32) << 16),
            0x0D8 => self.dma.channels[3].dad = (self.dma.channels[3].dad & !0xFFFF) | (val as u32),
            0x0DA => self.dma.channels[3].dad = (self.dma.channels[3].dad & 0xFFFF) | ((val as u32) << 16),
            0x0DC => self.dma.channels[3].count = val,
            0x0DE => {
                if self.dma.channels[3].write_cnt_h(val, true) {
                    self.execute_dma_channel(3);
                }
            }
            0x100 => {
                let now = self.now();
                self.timers.write_reload(0, val, now);
            }
            0x102 => {
                let now = self.now();
                self.timers.write_control(0, val, now);
            }
            0x104 => {
                let now = self.now();
                self.timers.write_reload(1, val, now);
            }
            0x106 => {
                let now = self.now();
                self.timers.write_control(1, val, now);
            }
            0x108 => {
                let now = self.now();
                self.timers.write_reload(2, val, now);
            }
            0x10A => {
                let now = self.now();
                self.timers.write_control(2, val, now);
            }
            0x10C => {
                let now = self.now();
                self.timers.write_reload(3, val, now);
            }
            0x10E => {
                let now = self.now();
                self.timers.write_control(3, val, now);
            }
            0x120..=0x12A | 0x134 | 0x140 | 0x150..=0x158 => self.sio.write_io16(addr & 0x3FE, val),
            0x130 => {} // KEYINPUT is read only
            0x132 => self.keypad.keycnt = val,
            0x200 => self.ie = val,
            0x202 => {
                // Writing 1s to IF clears the corresponding interrupt flags
                self.if_reg &= !val;
            }
            0x204 => {
                self.waitcnt = val;
                self.timing.set_waitcnt(val);
            }
            0x208 => self.ime = (val & 1) != 0,
            0x300 => {
                self.post_flg = (val & 0xFF) as u8;
                self.haltcnt = (val >> 8) as u8;
            }
            _ => {
                let off = (addr & 0x3FE) as usize;
                self.io_regs[off] = (val & 0xFF) as u8;
                self.io_regs[off + 1] = (val >> 8) as u8;
            }
        }
    }

    /// Intercepts DMA transfers whose source or destination lands in the
    /// cartridge's EEPROM window, handling the whole logical request/reply
    /// in one shot instead of running it through the generic per-halfword
    /// loop below (see eeprom.rs's module docs for why). Returns true if
    /// this transfer was an EEPROM access and has been fully handled,
    /// including completion bookkeeping (repeat/disable/IRQ).
    fn try_execute_eeprom_dma(&mut self, idx: usize) -> bool {
        let is_eeprom_cart = matches!(
            self.cartridge.as_ref().map(|c| c.save_type),
            Some(SaveType::Eeprom)
        );
        if !is_eeprom_cart {
            return false;
        }

        let (sad, dad, count, sad_ctrl, dad_ctrl, repeat, irq_on_finish) = {
            let ch = &self.dma.channels[idx];
            (
                ch.internal_sad,
                ch.internal_dad,
                ch.internal_count,
                (ch.cnt_h >> 7) & 3,
                (ch.cnt_h >> 5) & 3,
                (ch.cnt_h & (1 << 9)) != 0,
                (ch.cnt_h & (1 << 14)) != 0,
            )
        };

        let dad_is_eeprom = self.cartridge.as_ref().is_some_and(|c| c.is_eeprom_address(dad));
        let sad_is_eeprom = self.cartridge.as_ref().is_some_and(|c| c.is_eeprom_address(sad));
        if !dad_is_eeprom && !sad_is_eeprom {
            return false;
        }

        const STEP: u32 = 2; // EEPROM protocol transfers are always 16-bit halfwords

        let (final_sad, final_dad) = if dad_is_eeprom {
            // Write-direction: gather `count` protocol bits (bit 0 of each
            // halfword) from RAM at `sad`, MSB-first as received.
            let mut bits = Vec::with_capacity(count as usize);
            let mut addr = sad;
            for _ in 0..count {
                bits.push((self.read16(addr) & 1) != 0);
                addr = match sad_ctrl {
                    0 => addr.wrapping_add(STEP),
                    1 => addr.wrapping_sub(STEP),
                    _ => addr,
                };
            }
            if let Some(ee) = self.cartridge.as_mut().and_then(|c| c.save.as_eeprom_mut()) {
                ee.handle_write_request(&bits);
            }
            let dad_final = match dad_ctrl {
                0 | 3 => dad.wrapping_add(STEP.wrapping_mul(count)),
                1 => dad.wrapping_sub(STEP.wrapping_mul(count)),
                _ => dad, // Fixed: the EEPROM "register" address doesn't move
            };
            (addr, dad_final)
        } else {
            // Read-direction: produce `count` reply bits and scatter them
            // (one per halfword, bit 0 only) to RAM at `dad`.
            let mut bits = vec![false; count as usize];
            if let Some(cart) = self.cartridge.as_ref() {
                if let SaveBackend::Eeprom(ee) = &cart.save {
                    ee.handle_read_reply(&mut bits);
                }
            }
            let mut addr = dad;
            for &bit in &bits {
                self.write16(addr, bit as u16);
                addr = match dad_ctrl {
                    0 | 3 => addr.wrapping_add(STEP),
                    1 => addr.wrapping_sub(STEP),
                    _ => addr,
                };
            }
            let sad_final = match sad_ctrl {
                0 => sad.wrapping_add(STEP.wrapping_mul(count)),
                1 => sad.wrapping_sub(STEP.wrapping_mul(count)),
                _ => sad,
            };
            (sad_final, addr)
        };

        self.dma.channels[idx].internal_sad = final_sad;
        self.dma.channels[idx].internal_dad = final_dad;

        if repeat {
            let max_cnt = if idx == 3 { 0x10000 } else { 0x4000 };
            let cnt = (self.dma.channels[idx].count as u32) & (max_cnt - 1);
            self.dma.channels[idx].internal_count = if cnt == 0 { max_cnt } else { cnt };
            if dad_ctrl == 3 {
                self.dma.channels[idx].internal_dad = self.dma.channels[idx].dad;
            }
        } else {
            self.dma.channels[idx].enabled = false;
            self.dma.channels[idx].cnt_h &= !(1 << 15);
        }

        if irq_on_finish {
            self.request_interrupt(8 + idx as u16);
        }

        true
    }

    /// The current emulated cycle, including bus accesses already made by
    /// the instruction being executed. Timer reads/writes use this so they
    /// see the counter at the exact cycle of the access.
    #[inline(always)]
    pub fn now(&self) -> u64 {
        self.current_cycles + (self.timing.clock.get() - self.instr_clock_start)
    }

    /// DMA stall cycles waiting to be added to the next step.
    #[inline(always)]
    pub fn pending_dma_stall(&self) -> u32 {
        self.dma_stall
    }

    /// Take (and reset) the DMA stall cycles accumulated since last call.
    #[inline(always)]
    pub fn take_dma_stall(&mut self) -> u32 {
        std::mem::take(&mut self.dma_stall)
    }

    pub fn execute_dma_channel(&mut self, idx: usize) {
        if !self.dma.channels[idx].enabled {
            return;
        }
        if self.try_execute_eeprom_dma(idx) {
            return;
        }

        let (is_32bit, dad_ctrl, sad_ctrl, repeat, irq_on_finish, count, current_dad, current_sad, _is_direct_sound) = {
            let ch = &self.dma.channels[idx];
            if !ch.enabled {
                return;
            }

            let timing = (ch.cnt_h >> 12) & 3;
            let is_direct_sound = (idx == 1 || idx == 2) && timing == 3;

            // DirectSound FIFO DMA always transfers 4 32-bit words (16 bytes)
            let is_32bit = if is_direct_sound {
                true
            } else {
                (ch.cnt_h & (1 << 10)) != 0
            };

            let dad_ctrl = if is_direct_sound {
                2 // Fixed destination address for DirectSound FIFO
            } else {
                (ch.cnt_h >> 5) & 3
            };

            let sad_ctrl = (ch.cnt_h >> 7) & 3;
            let repeat = (ch.cnt_h & (1 << 9)) != 0;
            let irq_on_finish = (ch.cnt_h & (1 << 14)) != 0;

            let count = if is_direct_sound {
                4
            } else {
                let max_cnt = if idx == 3 { 0x10000 } else { 0x4000 };
                let cnt = (ch.count as u32) & (max_cnt - 1);
                if cnt == 0 { max_cnt } else { cnt }
            };

            let current_dad = if is_direct_sound {
                if (ch.dad & !3) == 0x0400_00A4 {
                    0x0400_00A4
                } else {
                    0x0400_00A0
                }
            } else {
                ch.internal_dad
            };

            (is_32bit, dad_ctrl, sad_ctrl, repeat, irq_on_finish, count, current_dad, ch.internal_sad, is_direct_sound)
        };

        let step_size = if is_32bit { 4 } else { 2 };
        let mut sad = current_sad;
        let mut dad = current_dad;

        // The CPU is halted for the whole transfer (ROADMAP M1). The copy
        // itself uses untimed accesses; its cost is charged as a stall.
        self.dma_stall += self.timing.dma_cost(sad, dad, is_32bit, count);

        for _ in 0..count {
            if is_32bit {
                let val = self.read32_raw(sad);
                self.write32_raw(dad, val);
            } else {
                let val = self.read16_raw(sad);
                self.write16_raw(dad, val);
            }

            match sad_ctrl {
                0 => sad = sad.wrapping_add(step_size),
                1 => sad = sad.wrapping_sub(step_size),
                _ => {} // Fixed
            }

            match dad_ctrl {
                0 => dad = dad.wrapping_add(step_size),
                1 => dad = dad.wrapping_sub(step_size),
                2 => {} // Fixed
                3 => dad = dad.wrapping_add(step_size),
                _ => {}
            }
        }

        // Advance internal working registers!
        self.dma.channels[idx].internal_sad = sad;
        self.dma.channels[idx].internal_dad = dad;

        let timing = (self.dma.channels[idx].cnt_h >> 12) & 3;
        let is_direct_sound = (idx == 1 || idx == 2) && timing == 3;

        if is_direct_sound {
            // DirectSound repeats until explicitly disabled by software.
            if !repeat {
                self.dma.channels[idx].enabled = false;
                self.dma.channels[idx].cnt_h &= !(1 << 15);
            }
        } else if repeat {
            if dad_ctrl == 3 {
                self.dma.channels[idx].internal_dad = self.dma.channels[idx].dad;
            }
        } else {
            self.dma.channels[idx].enabled = false;
            self.dma.channels[idx].cnt_h &= !(1 << 15);
        }

        if irq_on_finish {
            self.request_interrupt(8 + idx as u16);
        }
    }

    pub fn request_interrupt(&mut self, irq_bit: u16) {
        self.if_reg |= 1 << irq_bit;
        // Mirror into BIOS Interrupt Flags in IWRAM at 0x03007FF8
        let cur = self.read16(0x0300_7FF8);
        self.write16(0x0300_7FF8, cur | (1 << irq_bit));
    }

    pub fn has_pending_irq(&self) -> bool {
        self.ime && ((self.ie & self.if_reg) != 0)
    }

    pub fn handle_swi(&mut self, cpu: &mut Arm7Tdmi, comment: u32) {
        let swi_num = if comment >= 0x10000 {
            ((comment >> 16) & 0xFF) as u8
        } else {
            (comment & 0xFF) as u8
        };
        // Math SWIs: bit-exact results plus the time the real BIOS takes
        // (ROADMAP M1). The time is charged as a CPU stall; ~45 cycles of
        // SWI entry/exit overhead apply to every BIOS call.
        const SWI_OVERHEAD: u32 = 45;
        let r = |i: usize| cpu.regs[i];
        let math = match swi_num {
            0x06 => Some(bios_math::div(r(0) as i32, r(1) as i32)),
            0x07 => Some(bios_math::div(r(1) as i32, r(0) as i32)),
            0x08 => {
                let (v, c) = bios_math::sqrt(r(0));
                Some((v, r(1), r(3), c))
            }
            0x09 => Some(bios_math::arctan(r(0) as i32)),
            0x0A => {
                let (v, r1, c) = bios_math::arctan2(r(0) as i32, r(1) as i32);
                Some((v, r1, 0x170, c))
            }
            _ => None,
        };
        if let Some((r0, r1, r3, cycles)) = math {
            cpu.regs[0] = r0;
            cpu.regs[1] = r1;
            cpu.regs[3] = r3;
            self.dma_stall += cycles + SWI_OVERHEAD;
            self.bios_latch = BIOS_LATCH_AFTER_SWI;
            return;
        }

        // In GBA, BIOS functions 0x00..0x2A can be handled via HLE BIOS
        if swi_num <= 0x2A {
            let mut writes_8: Vec<(u32, u8)> = Vec::new();
            let mut writes_16: Vec<(u32, u16)> = Vec::new();
            let mut writes_32: Vec<(u32, u32)> = Vec::new();

            let pending_wait = {
                let read_8 = |addr: u32| -> u8 { self.read8(addr) };
                let mut write_8 = |addr: u32, val: u8| { writes_8.push((addr, val)); };
                let mut write_16 = |addr: u32, val: u16| { writes_16.push((addr, val)); };
                let mut write_32 = |addr: u32, val: u32| { writes_32.push((addr, val)); };

                bios::execute_hle_swi(swi_num, cpu, &read_8, &mut write_8, &mut write_16, &mut write_32)
            };

            for (addr, val) in writes_8 {
                self.write8(addr, val);
            }
            for (addr, val) in writes_16 {
                self.write16(addr, val);
            }
            for (addr, val) in writes_32 {
                self.write32(addr, val);
            }

            self.intr_wait_mask = pending_wait;
            self.intr_wait_dispatched = false;
            if pending_wait.is_some() {
                // The real IntrWait writes IME=1 before halting, so the
                // IRQ it waits for can actually be taken (games often call
                // VBlankIntrWait with IME off).
                self.ime = true;
            }
            self.bios_latch = BIOS_LATCH_AFTER_SWI;
        } else {
            cpu.trigger_swi(comment);
        }
    }
}
