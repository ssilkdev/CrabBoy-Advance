//! Main GBA System Coordinator

pub mod apu;
pub mod cheats;
pub mod cpu;
pub mod diagnostics;
pub mod dma;
pub mod keypad;
pub mod mmu;
pub mod ppu;
pub mod timer;

use cheats::CheatManager;
use cpu::{arm::step_arm, thumb::step_thumb, Arm7Tdmi, CpuMode};
use diagnostics::{DiagnosticReport, SystemDiagnostics};
use mmu::{cartridge::Cartridge, Mmu};
pub use ppu::{Ppu, SCREEN_HEIGHT, SCREEN_WIDTH};
use std::path::Path;

pub const CYCLES_PER_FRAME: u32 = 280_896; // 228 scanlines * 1232 cycles (~59.73 Hz)

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
        self.diagnostics.reset();
    }

    /// Step a single instruction and advance peripherals
    pub fn step_instruction(&mut self) -> u32 {
        // Check pending IRQ
        if self.mmu.has_pending_irq() && !self.cpu.get_flag(cpu::FLAG_I) {
            self.cpu.trigger_irq();
        }

        // Synchronize PC and cycles to MMU for flight recording
        self.mmu.current_pc = self.cpu.regs[15];
        self.mmu.current_cycles = self.cpu.cycles;

        // Execute instruction
        let cycles = if self.cpu.halted {
            1
        } else if self.cpu.is_thumb() {
            step_thumb(&mut self.cpu, &mut self.mmu)
        } else {
            step_arm(&mut self.cpu, &mut self.mmu)
        };

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

        // Step Timers
        let (timer_irq_mask, overflows) = self.mmu.timers.step(cycles);
        if timer_irq_mask != 0 {
            for i in 0..4 {
                if (timer_irq_mask & (1 << i)) != 0 {
                    self.mmu.request_interrupt(3 + i as u16);
                }
            }
        }

        // Step SIO (Serial Communication)
        if self.mmu.sio.step(cycles) {
            self.mmu.request_interrupt(7); // SIO interrupt
        }

        // On GBA, HALT is only broken when (IE & IF) != 0
        if self.cpu.halted && (self.mmu.ie & self.mmu.if_reg) != 0 {
            self.cpu.halted = false;
        }

        // If waiting in IntrWait / VBlankIntrWait and not servicing an IRQ, check if target interrupt occurred
        if let Some(mask) = self.mmu.intr_wait_mask {
            if !self.cpu.in_irq {
                let flags = self.mmu.read16(0x0300_7FF8);
                if (flags & mask) != 0 {
                    self.mmu.write16(0x0300_7FF8, flags & !mask);
                    self.mmu.intr_wait_mask = None;
                    self.cpu.halted = false;
                } else {
                    self.cpu.halted = true;
                }
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

    /// Run full frame (~280,896 cycles)
    pub fn run_frame(&mut self) {
        let mut frame_cycles = 0;
        while frame_cycles < CYCLES_PER_FRAME {
            let c = self.step_instruction();
            frame_cycles += c;
        }
        self.mmu.ppu.frame_ready = false;
        self.frame_counter += 1;

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
                cart.flash.sync_to_disk();
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

        data
    }

    pub fn load_state(&mut self, data: &[u8]) -> bool {
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

        true
    }
}
