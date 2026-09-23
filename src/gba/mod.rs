//! Main GBA System Coordinator

pub mod apu;
pub mod cheats;
pub mod cpu;
pub mod diagnostics;
pub mod dma;
pub mod keypad;
pub mod mmu;
pub mod ppu;
pub mod state;
pub mod timer;

use cheats::CheatManager;
use cpu::{arm::step_arm, thumb::step_thumb, Arm7Tdmi, CpuMode};
use diagnostics::{DiagnosticReport, SystemDiagnostics};
use mmu::{cartridge::Cartridge, Mmu};
pub use ppu::{Ppu, SCREEN_HEIGHT, SCREEN_WIDTH};
use std::path::Path;

/// Cycles between the IRQ line asserting (IE & IF != 0) and the CPU
/// taking the exception; matches mGBA's GBA_IRQ_DELAY and the Timer IRQ
/// test results.
pub const IRQ_DELAY: u64 = 7;
/// GBA CPU clock in Hz.
pub const CPU_HZ: u64 = 16_777_216;
pub const CYCLES_PER_FRAME: u32 = 280_896; // 228 scanlines * 1232 cycles (~59.73 Hz)

/// Marks the start of the v2 save-state tail (hardware controller state).
///
/// v1 states stopped after `io_regs` and silently lost IME/IE/DMA/timer state,
/// so restoring one resumed a machine with interrupts disabled and every DMA
/// channel cleared. States without this marker still load; they just can't
/// restore what was never written.
pub const STATE_V2_MAGIC: &[u8; 4] = b"CBA2";
/// Marks the v3 tail (ROADMAP M2): every remaining piece of machine state,
/// so a restored state replays exactly like the original run.
pub const STATE_V3_MAGIC: &[u8; 4] = b"CBA3";

pub struct Gba {
    pub cpu: Arm7Tdmi,
    pub mmu: Mmu,
    pub cheats: CheatManager,
    pub diagnostics: SystemDiagnostics,
    pub is_running: bool,
    pub frame_counter: u64,
}

impl Default for Gba {
    fn default() -> Self {
        Self::new()
    }
}

impl Gba {
    pub fn new() -> Self {
        Self {
            cpu: Arm7Tdmi::new(),
            mmu: Mmu::new(),
            cheats: CheatManager::new(),
            diagnostics: SystemDiagnostics::new(),
            is_running: true,
            frame_counter: 0,
        }
    }

    pub fn load_rom<P: AsRef<Path>>(&mut self, path: P) -> std::io::Result<()> {
        let cart = Cartridge::from_file(path)?;
        self.mmu.load_cartridge(cart);
        self.reset();
        Ok(())
    }

    pub fn load_rom_bytes(&mut self, rom: Vec<u8>) {
        let cart = Cartridge::from_bytes(rom);
        self.mmu.load_cartridge(cart);
        self.reset();
    }

    pub fn reset(&mut self) {
        self.cpu = Arm7Tdmi::new();
        // Standard GBA entry point: 0x0800_0000 (Game Pak ROM start)
        self.cpu.regs[15] = 0x0800_0000;
        self.cpu.set_mode(CpuMode::System);
        self.cpu.set_flag(cpu::FLAG_T, false); // Start in ARM state
        self.cpu.set_flag(cpu::FLAG_I, false); // Enable IRQs
        self.cpu.regs[13] = 0x0300_7F00;      // Initial user/system SP
        self.cpu.r13_irq = 0x0300_7FA0;
        self.cpu.r13_svc = 0x0300_7FE0;

        self.mmu.ppu = Ppu::new();
        self.mmu.ime = false;
        self.mmu.ie = 0;
        self.mmu.if_reg = 0;
        self.mmu.intr_wait_mask = None;
        self.mmu.bios_latch = mmu::BIOS_LATCH_BOOT;
        self.diagnostics.reset();
    }

    /// Step a single instruction and advance peripherals
    pub fn step_instruction(&mut self) -> u32 {
        // Check pending IRQ. The CPU only sees the IRQ line IRQ_DELAY
        // cycles after IE & IF becomes non-zero (ROADMAP M1; measured by
        // the mGBA suite's Timer IRQ tests).
        if self.mmu.has_pending_irq() && !self.cpu.get_flag(cpu::FLAG_I) {
            let ready = self.mmu.irq_assert_time.map_or(true, |t| self.cpu.cycles >= t + IRQ_DELAY);
            if ready {
                self.cpu.trigger_irq();
                if self.mmu.intr_wait_mask.is_some() {
                    self.mmu.intr_wait_dispatched = true;
                }
            }
        }

        // Synchronize PC and cycles to MMU for flight recording
        self.mmu.current_pc = self.cpu.regs[15];
        self.mmu.current_cycles = self.cpu.cycles;
        self.mmu.instr_clock_start = self.mmu.timing.clock.get();

        // BIOS open-bus latch for our IRQ dispatcher stub (see Mmu::new):
        // 0x24 jumps to the game's handler, 0x2C returns from the IRQ.
        match self.mmu.current_pc {
            0x24 => self.mmu.bios_latch = mmu::BIOS_LATCH_IN_IRQ,
            0x2C => self.mmu.bios_latch = mmu::BIOS_LATCH_AFTER_IRQ,
            _ => {}
        }

        // Execute instruction
        let cycles = if self.cpu.halted {
            1
        } else if self.cpu.is_thumb() {
            step_thumb(&mut self.cpu, &mut self.mmu)
        } else {
            step_arm(&mut self.cpu, &mut self.mmu)
        };

        // DMA triggered by this instruction (or by the previous step's
        // HBlank/VBlank/FIFO events) held the CPU off the bus.
        let cycles = cycles + self.mmu.take_dma_stall();
        self.cpu.cycles += cycles as u64;

        // Step PPU
        let (irq_vblank, irq_hblank, irq_vcounter, dma_vblank, dma_hblank) = self.mmu.ppu.step(cycles);
        if irq_vblank {
            self.mmu.request_interrupt(0); // VBlank IRQ
        }
        if irq_hblank {
            self.mmu.request_interrupt(1); // HBlank IRQ
        }
        if irq_vcounter {
            self.mmu.request_interrupt(2); // VCounter IRQ
        }

        // Trigger PPU DMAs
        if dma_vblank {
            for ch in self.mmu.dma.trigger(1) {
                self.mmu.execute_dma_channel(ch);
            }
        }
        if dma_hblank {
            for ch in self.mmu.dma.trigger(2) {
                self.mmu.execute_dma_channel(ch);
            }
        }

        // Timers: bring counters up to the end of this instruction; an
        // overflow raises IF at its exact cycle.
        self.mmu.timers.sync(self.cpu.cycles);
        let timer_events = self.mmu.timers.take_events();
        let overflows = timer_events.overflows;
        let mut irq_time = None;
        if timer_events.irq_mask != 0 {
            for i in 0..4 {
                if (timer_events.irq_mask & (1 << i)) != 0 {
                    self.mmu.request_interrupt(3 + i as u16);
                }
            }
            irq_time = Some(timer_events.irq_time);
        }

        // Step SIO (Serial Communication)
        if self.mmu.sio.step(cycles) {
            self.mmu.request_interrupt(7); // SIO interrupt
        }

        // Keypad IRQ (KEYCNT AND/OR condition against KEYINPUT)
        if self.mmu.keypad.check_irq() {
            self.mmu.request_interrupt(12);
        }

        // Track when the IRQ line was asserted (for IRQ_DELAY). Timer
        // IRQs know their exact overflow cycle; other sources count from
        // the end of this instruction.
        if (self.mmu.ie & self.mmu.if_reg) != 0 {
            if self.mmu.irq_assert_time.is_none() {
                self.mmu.irq_assert_time = Some(irq_time.unwrap_or(self.cpu.cycles));
            }
        } else {
            self.mmu.irq_assert_time = None;
        }

        // On GBA, HALT is only broken when (IE & IF) != 0
        if self.cpu.halted && (self.mmu.ie & self.mmu.if_reg) != 0 {
            self.cpu.halted = false;
        }

        // IntrWait / VBlankIntrWait: the real BIOS sleeps in HALT and only
        // re-checks its flags after an IRQ handler has run, so it can
        // never return before the IRQ is dispatched (which matters now
        // that IRQs are taken IRQ_DELAY cycles after IF is raised).
        if let Some(mask) = self.mmu.intr_wait_mask {
            if !self.cpu.in_irq && self.mmu.intr_wait_dispatched {
                let flags = self.mmu.read16(0x0300_7FF8);
                if (flags & mask) != 0 {
                    self.mmu.write16(0x0300_7FF8, flags & !mask);
                    self.mmu.intr_wait_mask = None;
                    self.cpu.halted = false;
                    // IntrWait returns through the BIOS SWI exit, so the
                    // BIOS open-bus latch holds its last opcode.
                    self.mmu.bios_latch = mmu::BIOS_LATCH_AFTER_SWI;
                } else {
                    // Not the IRQ we wait for: sleep until the next one.
                    self.mmu.intr_wait_dispatched = false;
                    self.cpu.halted = true;
                }
            } else if !self.cpu.in_irq {
                self.cpu.halted = true;
            }
        }

        // Step APU
        let (dma_req_a, dma_req_b) = self.mmu.apu.step(cycles, overflows);
        if dma_req_a {
            let mut handled = false;
            for ch_idx in 1..=2 {
                let ch = &self.mmu.dma.channels[ch_idx];
                let timing = (ch.cnt_h >> 12) & 3;
                if ch.enabled && timing == 3 && (ch.dad & !3) == 0x0400_00A0 {
                    self.mmu.execute_dma_channel(ch_idx);
                    handled = true;
                    break;
                }
            }
            if !handled {
                let ch = &self.mmu.dma.channels[1];
                if ch.enabled && ((ch.cnt_h >> 12) & 3) == 3 {
                    self.mmu.execute_dma_channel(1);
                }
            }
        }
        if dma_req_b {
            let mut handled = false;
            for ch_idx in 1..=2 {
                let ch = &self.mmu.dma.channels[ch_idx];
                let timing = (ch.cnt_h >> 12) & 3;
                if ch.enabled && timing == 3 && (ch.dad & !3) == 0x0400_00A4 {
                    self.mmu.execute_dma_channel(ch_idx);
                    handled = true;
                    break;
                }
            }
            if !handled {
                let ch = &self.mmu.dma.channels[2];
                if ch.enabled && ((ch.cnt_h >> 12) & 3) == 3 {
                    self.mmu.execute_dma_channel(2);
                }
            }
        }

        cycles
    }

    /// Make the cartridge RTC deterministic (ROADMAP M2): `Some(unix)` pins
    /// the clock to `unix` at power-on and advances it with emulated time
    /// only; `None` returns to the host wall clock. Movie replays, run-ahead
    /// and tests use this so the same inputs always give the same frames.
    pub fn set_deterministic_clock(&mut self, base_unix: Option<i64>) {
        if let Some(cart) = self.mmu.cartridge.as_mut() {
            cart.rtc.clock = match base_unix {
                Some(base_unix) => mmu::rtc::RtcClock::Emulated { base_unix },
                None => mmu::rtc::RtcClock::Host,
            };
            cart.rtc.emulated_secs = (self.cpu.cycles / CPU_HZ) as i64;
        }
    }

    /// Run full frame (~280,896 cycles)
    pub fn run_frame(&mut self) {
        let mut frame_cycles = 0;
        while frame_cycles < CYCLES_PER_FRAME {
            let c = self.step_instruction();
            frame_cycles += c;
        }
        self.mmu.ppu.frame_ready = false;
        self.frame_counter += 1;

        // Emulated wall time for a deterministic RTC (whole seconds are all
        // the RTC reports, so once per frame is precise enough).
        if let Some(cart) = self.mmu.cartridge.as_mut() {
            cart.rtc.emulated_secs = (self.cpu.cycles / CPU_HZ) as i64;
        }

        // Apply active cheats on VBlank
        self.cheats.apply(&mut self.mmu);

        // Flush frame audio samples and feed diagnostic linter
        self.mmu.apu.flush_samples();
        if !self.mmu.apu.pending_diagnostic_samples.is_empty() {
            self.diagnostics.process_audio_samples(&self.mmu.apu.pending_diagnostic_samples);
            self.mmu.apu.pending_diagnostic_samples.clear();
        }
        self.diagnostics.on_frame(&self.mmu.ppu, &self.mmu.apu);

        // Periodically sync save file to disk (every 60 frames / 1 sec)
        if self.frame_counter.is_multiple_of(60) {
            if let Some(ref mut cart) = self.mmu.cartridge {
                cart.save.sync_to_disk();
            }
        }
    }

    /// Run emulation for N frames headlessly and generate an authoritative DiagnosticReport
    pub fn run_diagnostics(&mut self, frames: u64) -> DiagnosticReport {
        for _ in 0..frames {
            self.run_frame();
        }
        self.diagnostics.generate_report(&self.mmu, self.frame_counter, self.cpu.cycles)
    }

    /// Dump current framebuffer as PNG
    pub fn dump_frame_png<P: AsRef<Path>>(&self, path: P) -> std::io::Result<()> {
        let mut raw_bytes = Vec::with_capacity(SCREEN_WIDTH * SCREEN_HEIGHT * 4);
        for &pixel in self.mmu.ppu.framebuffer.iter() {
            raw_bytes.push((pixel & 0xFF) as u8);         // R
            raw_bytes.push(((pixel >> 8) & 0xFF) as u8);  // G
            raw_bytes.push(((pixel >> 16) & 0xFF) as u8); // B
            raw_bytes.push(((pixel >> 24) & 0xFF) as u8); // A
        }
        image::save_buffer(
            path,
            &raw_bytes,
            SCREEN_WIDTH as u32,
            SCREEN_HEIGHT as u32,
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
    }

    pub fn get_framebuffer(&self) -> &[u32; SCREEN_WIDTH * SCREEN_HEIGHT] {
        &self.mmu.ppu.framebuffer
    }

    pub fn save_state(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(512 * 1024);
        // CPU regs
        for r in self.cpu.regs {
            data.extend_from_slice(&r.to_le_bytes());
        }
        data.extend_from_slice(&self.cpu.cpsr.to_le_bytes());
        data.extend_from_slice(&self.cpu.cycles.to_le_bytes());
        data.push(self.cpu.halted as u8);

        for r in self.cpu.r8_usr { data.extend_from_slice(&r.to_le_bytes()); }
        for r in self.cpu.r8_fiq { data.extend_from_slice(&r.to_le_bytes()); }
        data.extend_from_slice(&self.cpu.r13_usr.to_le_bytes());
        data.extend_from_slice(&self.cpu.r14_usr.to_le_bytes());
        data.extend_from_slice(&self.cpu.r13_fiq.to_le_bytes());
        data.extend_from_slice(&self.cpu.r14_fiq.to_le_bytes());
        data.extend_from_slice(&self.cpu.spsr_fiq.to_le_bytes());
        data.extend_from_slice(&self.cpu.r13_irq.to_le_bytes());
        data.extend_from_slice(&self.cpu.r14_irq.to_le_bytes());
        data.extend_from_slice(&self.cpu.spsr_irq.to_le_bytes());
        data.extend_from_slice(&self.cpu.r13_svc.to_le_bytes());
        data.extend_from_slice(&self.cpu.r14_svc.to_le_bytes());
        data.extend_from_slice(&self.cpu.spsr_svc.to_le_bytes());
        data.extend_from_slice(&self.cpu.r13_abt.to_le_bytes());
        data.extend_from_slice(&self.cpu.r14_abt.to_le_bytes());
        data.extend_from_slice(&self.cpu.spsr_abt.to_le_bytes());
        data.extend_from_slice(&self.cpu.r13_und.to_le_bytes());
        data.extend_from_slice(&self.cpu.r14_und.to_le_bytes());
        data.extend_from_slice(&self.cpu.spsr_und.to_le_bytes());
        data.push(self.cpu.irq_pending as u8);

        // Memory
        data.extend_from_slice(&self.mmu.ewram[..]);
        data.extend_from_slice(&self.mmu.iwram[..]);
        data.extend_from_slice(&self.mmu.ppu.vram[..]);
        data.extend_from_slice(&self.mmu.ppu.palette_ram[..]);
        data.extend_from_slice(&self.mmu.ppu.oam[..]);
        data.extend_from_slice(&self.mmu.io_regs[..]);

        // --- v2 tail -------------------------------------------------------
        // Everything above is raw memory + CPU. The hardware controllers below
        // live in their own structs and were previously NOT serialized, so a
        // restored state resumed with IME=false, IE=0 and all DMA channels
        // cleared. For Pokemon Emerald that meant the game's IRQ setup was
        // gone and execution ran off into IO space on the next interrupt.
        //
        // Appended as a versioned tail so older state files (which simply end
        // here) still load.
        data.extend_from_slice(STATE_V2_MAGIC);

        data.push(self.mmu.ime as u8);
        data.extend_from_slice(&self.mmu.ie.to_le_bytes());
        data.extend_from_slice(&self.mmu.if_reg.to_le_bytes());
        data.extend_from_slice(&self.mmu.waitcnt.to_le_bytes());

        for ch in &self.mmu.dma.channels {
            data.extend_from_slice(&ch.sad.to_le_bytes());
            data.extend_from_slice(&ch.dad.to_le_bytes());
            data.extend_from_slice(&ch.count.to_le_bytes());
            data.extend_from_slice(&ch.cnt_h.to_le_bytes());
            data.extend_from_slice(&ch.internal_sad.to_le_bytes());
            data.extend_from_slice(&ch.internal_dad.to_le_bytes());
            data.extend_from_slice(&ch.internal_count.to_le_bytes());
            data.push(ch.enabled as u8);
        }

        for tm in &self.mmu.timers.timers {
            data.extend_from_slice(&tm.reload.to_le_bytes());
            data.extend_from_slice(&tm.counter.to_le_bytes());
            data.extend_from_slice(&tm.cnt_h.to_le_bytes());
            data.push(tm.enabled as u8);
            data.extend_from_slice(&tm.prescaler_cycles.to_le_bytes());
            data.extend_from_slice(&tm.cycles_accum.to_le_bytes());
            data.push(tm.cascade as u8);
            data.push(tm.irq_enable as u8);
        }

        // --- v3 tail (ROADMAP M2) ------------------------------------------
        // Everything v1/v2 left out: CPU pipeline and IRQ-servicing flag,
        // PPU registers and scanline position, APU (DirectSound FIFOs, PSG
        // channels, sample clock, filters), timer anchors, bus timing and
        // prefetch buffer, serial port, keypad, cartridge (save chip, RTC,
        // sensors) and the MMU's own latches. With these, save -> load gives
        // a bit-identical continuation (tests/determinism.rs).
        use state::Snapshot;
        data.extend_from_slice(STATE_V3_MAGIC);
        let mut w = state::StateWriter::new(data);
        self.cpu.save(&mut w);
        self.mmu.ppu.save(&mut w);
        self.mmu.apu.save(&mut w);
        self.mmu.timers.save(&mut w);
        self.mmu.timing.save(&mut w);
        self.mmu.sio.save(&mut w);
        self.mmu.keypad.save(&mut w);
        w.u8(self.mmu.post_flg);
        w.u8(self.mmu.haltcnt);
        w.u32(self.mmu.open_bus);
        w.u32(self.mmu.bios_latch);
        w.u32(self.mmu.dma_stall);
        w.opt_u64(self.mmu.irq_assert_time);
        w.bool(self.mmu.intr_wait_dispatched);
        w.opt_u64(self.mmu.intr_wait_mask.map(|m| m as u64));
        w.u64(self.frame_counter);
        match &self.mmu.cartridge {
            Some(cart) => {
                w.bool(true);
                cart.save(&mut w);
            }
            None => w.bool(false),
        }
        w.buf
    }

    pub fn load_state(&mut self, data: &[u8]) -> bool {
        // The prefetch pipeline isn't part of the state format; refill it
        // from memory after loading.
        self.cpu.pipe_valid = false;
        // Timer anchors and the IRQ line time aren't serialized either.
        self.mmu.irq_assert_time = None;
        let min_len = 16 * 4 + 4 + 8 + 1 + 109 + 256 * 1024 + 32 * 1024 + 96 * 1024 + 1024 + 1024 + 1024;
        if data.len() < min_len {
            return false;
        }

        let mut offset = 0;
        for r in 0..16 {
            let bytes: [u8; 4] = data[offset..offset + 4].try_into().unwrap();
            self.cpu.regs[r] = u32::from_le_bytes(bytes);
            offset += 4;
        }

        let cpsr_bytes: [u8; 4] = data[offset..offset + 4].try_into().unwrap();
        self.cpu.cpsr = u32::from_le_bytes(cpsr_bytes);
        offset += 4;

        let cycles_bytes: [u8; 8] = data[offset..offset + 8].try_into().unwrap();
        self.cpu.cycles = u64::from_le_bytes(cycles_bytes);
        offset += 8;

        self.cpu.halted = data[offset] != 0;
        offset += 1;

        for i in 0..5 {
            let bytes: [u8; 4] = data[offset..offset + 4].try_into().unwrap();
            self.cpu.r8_usr[i] = u32::from_le_bytes(bytes);
            offset += 4;
        }
        for i in 0..5 {
            let bytes: [u8; 4] = data[offset..offset + 4].try_into().unwrap();
            self.cpu.r8_fiq[i] = u32::from_le_bytes(bytes);
            offset += 4;
        }
        let mut read_u32 = || {
            let bytes: [u8; 4] = data[offset..offset + 4].try_into().unwrap();
            offset += 4;
            u32::from_le_bytes(bytes)
        };
        self.cpu.r13_usr = read_u32();
        self.cpu.r14_usr = read_u32();
        self.cpu.r13_fiq = read_u32();
        self.cpu.r14_fiq = read_u32();
        self.cpu.spsr_fiq = read_u32();
        self.cpu.r13_irq = read_u32();
        self.cpu.r14_irq = read_u32();
        self.cpu.spsr_irq = read_u32();
        self.cpu.r13_svc = read_u32();
        self.cpu.r14_svc = read_u32();
        self.cpu.spsr_svc = read_u32();
        self.cpu.r13_abt = read_u32();
        self.cpu.r14_abt = read_u32();
        self.cpu.spsr_abt = read_u32();
        self.cpu.r13_und = read_u32();
        self.cpu.r14_und = read_u32();
        self.cpu.spsr_und = read_u32();
        self.cpu.irq_pending = data[offset] != 0;
        offset += 1;

        self.mmu.ewram.copy_from_slice(&data[offset..offset + 256 * 1024]);
        offset += 256 * 1024;

        self.mmu.iwram.copy_from_slice(&data[offset..offset + 32 * 1024]);
        offset += 32 * 1024;

        self.mmu.ppu.vram.copy_from_slice(&data[offset..offset + 96 * 1024]);
        offset += 96 * 1024;

        self.mmu.ppu.palette_ram.copy_from_slice(&data[offset..offset + 1024]);
        offset += 1024;

        self.mmu.ppu.oam.copy_from_slice(&data[offset..offset + 1024]);
        offset += 1024;

        self.mmu.io_regs.copy_from_slice(&data[offset..offset + 1024]);
        offset += 1024;

        // --- v2 tail (optional) --------------------------------------------
        // Restore the hardware controllers. Older state files end right here,
        // so their absence is not an error -- but then IME/IE/DMA/timers keep
        // whatever `reset()` left, which is why v1 states resumed broken.
        if data.len() >= offset + STATE_V2_MAGIC.len()
            && &data[offset..offset + STATE_V2_MAGIC.len()] == STATE_V2_MAGIC
        {
            offset += STATE_V2_MAGIC.len();

            let need = 1 + 2 + 2 + 2 + 4 * 21 + 4 * 16;
            if data.len() < offset + need {
                return false;
            }

            let u8_at = |o: &mut usize| {
                let v = data[*o];
                *o += 1;
                v
            };
            self.mmu.ime = u8_at(&mut offset) != 0;

            let u16_at = |o: &mut usize| {
                let v = u16::from_le_bytes(data[*o..*o + 2].try_into().unwrap());
                *o += 2;
                v
            };
            self.mmu.ie = u16_at(&mut offset);
            self.mmu.if_reg = u16_at(&mut offset);
            self.mmu.waitcnt = u16_at(&mut offset);
            self.mmu.timing.set_waitcnt(self.mmu.waitcnt);

            for i in 0..4 {
                let ch = &mut self.mmu.dma.channels[i];
                ch.sad = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
                offset += 4;
                ch.dad = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
                offset += 4;
                ch.count = u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap());
                offset += 2;
                ch.cnt_h = u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap());
                offset += 2;
                ch.internal_sad = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
                offset += 4;
                ch.internal_dad = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
                offset += 4;
                ch.internal_count = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
                offset += 4;
                ch.enabled = data[offset] != 0;
                offset += 1;
            }

            for i in 0..4 {
                let tm = &mut self.mmu.timers.timers[i];
                tm.reload = u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap());
                offset += 2;
                tm.counter = u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap());
                offset += 2;
                tm.cnt_h = u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap());
                offset += 2;
                tm.enabled = data[offset] != 0;
                offset += 1;
                tm.prescaler_cycles =
                    u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
                offset += 4;
                tm.cycles_accum = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
                offset += 4;
                tm.cascade = data[offset] != 0;
                offset += 1;
                tm.irq_enable = data[offset] != 0;
                offset += 1;
            }
            let now = self.cpu.cycles;
            self.mmu.timers.rebase_all(now);

            // --- v3 tail (optional; see save_state) ---------------------
            if data.len() >= offset + STATE_V3_MAGIC.len()
                && data[offset..offset + STATE_V3_MAGIC.len()] == STATE_V3_MAGIC[..]
            {
                offset += STATE_V3_MAGIC.len();
                if self.load_state_v3(&data[offset..]).is_none() {
                    return false;
                }
            }
        }

        true
    }

    /// Restore the v3 tail. `None` = malformed or from another game.
    fn load_state_v3(&mut self, data: &[u8]) -> Option<()> {
        use state::Snapshot;
        let mut r = state::StateReader::new(data, 0);
        self.cpu.load(&mut r)?;
        self.mmu.ppu.load(&mut r)?;
        self.mmu.apu.load(&mut r)?;
        self.mmu.timers.load(&mut r)?;
        self.mmu.timing.load(&mut r)?;
        self.mmu.sio.load(&mut r)?;
        self.mmu.keypad.load(&mut r)?;
        self.mmu.post_flg = r.u8()?;
        self.mmu.haltcnt = r.u8()?;
        self.mmu.open_bus = r.u32()?;
        self.mmu.bios_latch = r.u32()?;
        self.mmu.dma_stall = r.u32()?;
        self.mmu.irq_assert_time = r.opt_u64()?;
        self.mmu.intr_wait_dispatched = r.bool()?;
        self.mmu.intr_wait_mask = r.opt_u64()?.map(|m| m as u16);
        self.frame_counter = r.u64()?;
        let has_cart = r.bool()?;
        match (&mut self.mmu.cartridge, has_cart) {
            (Some(cart), true) => {
                cart.load(&mut r)?;
                cart.rtc.emulated_secs = (self.cpu.cycles / CPU_HZ) as i64;
            }
            (None, false) => {}
            _ => return None,
        }
        Some(())
    }
}
