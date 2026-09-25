//! Main GBA System Coordinator

pub mod accessibility;
pub mod memmap;
pub mod apu;
pub mod cheats;
pub mod cpu;
pub mod diagnostics;
pub mod dma;
pub mod frame_blend;
pub mod hd_pack;
pub mod keypad;
pub mod mmu;
pub mod ppu;
pub mod replay;
pub mod run_ahead;
pub mod m4a;
pub mod save_sync;
pub mod shader;
pub mod state;
pub mod timer;
pub mod widescreen;

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
/// Marks the optional framebuffer block at the end of a v3 state.
const STATE_FB_MAGIC: &[u8; 4] = b"CBFB";

pub struct Gba {
    pub cpu: Arm7Tdmi,
    pub mmu: Mmu,
    pub cheats: CheatManager,
    pub diagnostics: SystemDiagnostics,
    pub is_running: bool,
    pub frame_counter: u64,
    /// Set while run-ahead emulates frames that will be rolled back
    /// (ROADMAP M3): no audio output or capture, no diagnostics, no save
    /// file writes.
    pub speculative: bool,
    /// High-resolution M4A / Sappy audio re-synthesis engine (ROADMAP M9)
    pub m4a: m4a::HdM4aEngine,
    /// While the CPU is halted, jump straight to the next hardware event
    /// instead of stepping one cycle at a time. Bit-identical results (see
    /// tests/halt_skip.rs); off only for comparison.
    pub halt_skip: bool,
    /// Cycles left in the current `run_frame` (a halt skip never crosses
    /// the frame end). 1 outside `run_frame`, so single-stepping is exact.
    #[doc(hidden)]
    pub halt_budget: u32,
    /// Let the peripherals run behind the CPU until the next hardware
    /// event (JIT stage 2). Bit-identical (tests/halt_skip.rs); off only
    /// for comparison. Takes effect inside `run_frame` only.
    pub batch_peripherals_enabled: bool,
    batch_peripherals: bool,
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
            speculative: false,
            m4a: m4a::HdM4aEngine::new(),
            halt_skip: true,
            halt_budget: 1,
            batch_peripherals_enabled: true,
            batch_peripherals: false,
        }
    }

    /// A core that never opens a host audio device: for replays, tests and
    /// tools (ROADMAP M2). Audio is still produced (and can be captured via
    /// `mmu.apu.capture`), it just isn't played.
    pub fn new_headless() -> Self {
        // Scoped: only this core's AudioOutput is headless.
        let prev = apu::audio_output::set_headless(true);
        let gba = Self::new();
        apu::audio_output::set_headless(prev);
        gba
    }

    /// Set whether the core is running speculative (run-ahead) frames.
    pub fn set_speculative(&mut self, spec: bool) {
        self.speculative = spec;
        self.mmu.apu.speculative = spec;
    }

    /// Enable or disable isolated per-layer framebuffer capture and draw commands (ROADMAP M5).
    pub fn set_layer_capture(&mut self, enable: bool) {
        self.mmu.ppu.set_layer_capture(enable);
    }

    /// Access isolated RGBA framebuffer surface for a specific layer.
    pub fn get_layer_framebuffer(&self, layer: ppu::layers::PpuLayer) -> Option<&[u32; ppu::SCREEN_WIDTH * ppu::SCREEN_HEIGHT]> {
        self.mmu.ppu.get_layer_framebuffer(layer)
    }

    /// Access all isolated layer framebuffers.
    pub fn get_layer_framebuffers(&self) -> Option<&ppu::layers::PpuLayerBuffers> {
        self.mmu.ppu.get_layer_framebuffers()
    }

    /// Access recorded draw commands for the current frame.
    pub fn get_draw_commands(&self) -> &[ppu::layers::LayerDrawCommand] {
        self.mmu.ppu.get_draw_commands()
    }

    /// Configure HD Mode 7 high-resolution affine rendering (ROADMAP M6).
    pub fn set_hd_mode7_config(&mut self, config: ppu::hd_mode7::HdMode7Config) {
        self.mmu.ppu.set_hd_mode7_config(config);
    }

    /// Load an HD Sprite and Tile replacement pack (ROADMAP M7).
    pub fn load_hd_pack(&mut self, pack: hd_pack::HdPack) {
        self.mmu.ppu.load_hd_pack(pack);
    }

    /// Toggle HD replacement pack active state.
    pub fn set_hd_pack_enabled(&mut self, enabled: bool) {
        self.mmu.ppu.set_hd_pack_enabled(enabled);
    }

    /// Query if an HD replacement pack is actively loaded and enabled.
    pub fn is_hd_pack_enabled(&self) -> bool {
        self.mmu.ppu.is_hd_pack_enabled()
    }

    /// Borrow loaded HD replacement pack if present.
    pub fn hd_pack(&self) -> Option<&hd_pack::HdPack> {
        self.mmu.ppu.hd_pack()
    }

    /// Mutably borrow loaded HD replacement pack if present.
    pub fn hd_pack_mut(&mut self) -> Option<&mut hd_pack::HdPack> {
        self.mmu.ppu.hd_pack_mut()
    }

    /// Dump visible tiles and sprites from the core into PNGs and create a template manifest.
    pub fn dump_tiles_and_sprites<P: AsRef<Path>>(&self, output_dir: P) -> std::io::Result<hd_pack::HdPackManifest> {
        hd_pack::dump_tiles_and_sprites(
            &self.mmu.ppu.vram[..],
            &self.mmu.ppu.oam[..],
            &self.mmu.ppu.palette_ram[..],
            self.mmu.ppu.dispcnt,
            output_dir,
        )
    }

    /// Render HD Mode 7, HD Pack, or Widescreen frame if active.
    pub fn render_hd_frame(&self) -> Option<ppu::hd_mode7::HdFrame> {
        self.mmu.ppu.render_hd_frame()
    }

    /// Configure Widescreen rendering (ROADMAP M8).
    pub fn set_widescreen_config(&mut self, config: widescreen::WidescreenConfig) {
        self.mmu.ppu.set_widescreen_config(config);
    }

    /// Toggle widescreen active state.
    pub fn set_widescreen_enabled(&mut self, enabled: bool) {
        self.mmu.ppu.set_widescreen_enabled(enabled);
    }

    /// Query if widescreen is actively enabled.
    pub fn is_widescreen_enabled(&self) -> bool {
        self.mmu.ppu.is_widescreen_enabled()
    }

    /// Borrow active widescreen config.
    pub fn widescreen_config(&self) -> &widescreen::WidescreenConfig {
        self.mmu.ppu.widescreen_config()
    }

    /// Mutably borrow active widescreen config.
    pub fn widescreen_config_mut(&mut self) -> &mut widescreen::WidescreenConfig {
        self.mmu.ppu.widescreen_config_mut()
    }

    /// Render widescreen frame directly if active.
    pub fn render_widescreen_frame(&self) -> Option<ppu::hd_mode7::HdFrame> {
        self.mmu.ppu.render_widescreen_frame()
    }

    /// Automatically configure widescreen settings and apply patches for loaded cartridge.
    pub fn configure_widescreen_for_loaded_cartridge(&mut self) -> Option<&'static widescreen::WidescreenGameProfile> {
        if let Some(ref mut cart) = self.mmu.cartridge {
            if let Some(profile) = widescreen::WidescreenDatabase::lookup(&cart.game_code, &cart.title) {
                let mut config = profile.to_config();
                config.enabled = self.mmu.ppu.widescreen_config.enabled;
                self.mmu.ppu.set_widescreen_config(config);
                profile.apply_patches(&mut cart.rom);
                log::info!("Auto-configured widescreen profile for '{}' [{}]: {}", profile.title, profile.game_code, profile.notes);
                return Some(profile);
            }
        }
        None
    }

    pub fn load_rom<P: AsRef<Path>>(&mut self, path: P) -> std::io::Result<()> {
        let cart = Cartridge::from_file(path)?;
        self.mmu.load_cartridge(cart);
        self.configure_widescreen_for_loaded_cartridge();
        if let Some(ref cart) = self.mmu.cartridge {
            self.m4a.detect_and_init(&cart.rom, &cart.game_code, &cart.title);
        }
        self.reset();
        Ok(())
    }

    pub fn load_rom_bytes(&mut self, rom: Vec<u8>) {
        let cart = Cartridge::from_bytes(rom);
        self.mmu.load_cartridge(cart);
        self.configure_widescreen_for_loaded_cartridge();
        if let Some(ref cart) = self.mmu.cartridge {
            self.m4a.detect_and_init(&cart.rom, &cart.game_code, &cart.title);
        }
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
        self.m4a.sampler.stop_all();
        self.mmu.apu.hd_sample_stream.clear();
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
            if self.halt_skip {
                self.cycles_to_next_event()
            } else {
                1
            }
        } else if self.cpu.is_thumb() {
            step_thumb(&mut self.cpu, &mut self.mmu)
        } else {
            step_arm(&mut self.cpu, &mut self.mmu)
        };

        // DMA triggered by this instruction (or by the previous step's
        // HBlank/VBlank/FIFO events) held the CPU off the bus.
        let cycles = cycles + self.mmu.take_dma_stall();
        self.cpu.cycles += cycles as u64;
        self.mmu.pending_cycles += cycles;

        // JIT stage 2 (docs/JIT.md): the PPU, timers, APU and serial port
        // only need to catch up when something can happen, i.e. at the next
        // scheduled event. Until then, keep running instructions and let
        // the cycles pile up. Anything that could observe or change the
        // hardware mid-run (IO register access, SWI, DMA) catches up first.
        if self.batch_peripherals && self.can_defer() {
            return cycles;
        }
        let (stepped, overflows) = self.mmu.catch_up_timed(self.cpu.cycles);

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

        self.mmu.catch_up_apu(stepped, overflows);

        cycles
    }

    /// Bring every peripheral up to the CPU (end of a frame, or before
    /// anything outside the CPU loop looks at the hardware).
    pub fn catch_up_peripherals(&mut self) {
        if self.mmu.pending_cycles != 0 {
            let (c, ov) = self.mmu.catch_up_timed(self.cpu.cycles);
            self.mmu.catch_up_apu(c, ov);
        }
    }

    /// Whether the peripherals may stay behind after this instruction:
    /// nothing is due before the event horizon, and no CPU-side state
    /// (sleeping CPU, IntrWait, a raised or pending IRQ, a queued DMA
    /// stall, a keypad IRQ condition) needs per-instruction attention.
    #[inline(always)]
    fn can_defer(&mut self) -> bool {
        if self.cpu.halted
            || self.mmu.intr_wait_mask.is_some()
            || (self.mmu.ie & self.mmu.if_reg) != 0
            || self.mmu.irq_assert_time.is_some()
            || self.mmu.pending_dma_stall() != 0
            || self.mmu.keypad.check_irq()
        {
            return false;
        }
        if self.mmu.defer_horizon == 0 {
            // Start of a run: the peripherals are current up to
            // `cpu.cycles - pending_cycles`; the horizon counts from there.
            self.mmu.defer_horizon = self.peripheral_horizon();
        }
        self.mmu.pending_cycles < self.mmu.defer_horizon
    }

    /// Cycles from the last catch-up to the first thing that can happen:
    /// the same event sources as the sleep skip, plus the DMG sound frame
    /// sequencer (it changes channel frequencies). The frame end needs no
    /// entry: `run_frame` catches up after its loop.
    fn peripheral_horizon(&self) -> u32 {
        let caught_up_at = self.cpu.cycles - self.mmu.pending_cycles as u64;
        let mut n = self.mmu.ppu.cycles_to_next_boundary();
        n = n.min(self.mmu.timers.cycles_to_next_overflow(caught_up_at));
        n = n.min(self.mmu.apu.cycles_to_next_sample());
        n = n.min(self.mmu.apu.dmg.cycles_to_next_sequencer_step());
        if self.mmu.sio.transfer_cycles_left > 0 {
            n = n.min(self.mmu.sio.transfer_cycles_left);
        }
        n.max(1)
    }

    /// Cycles a halted CPU can sleep before anything can happen: the next
    /// PPU boundary (HBlank/line end), timer overflow, audio sample, serial
    /// transfer end or frame end, whichever is first. Nothing observable
    /// changes in between, so the result matches stepping 1 cycle at a
    /// time; games that wait in HALT (most of them, most of each frame)
    /// then cost a few dozen steps per frame instead of ~100k.
    fn cycles_to_next_event(&self) -> u32 {
        let mut n = self.halt_budget.max(1);
        // IRQ line already raised (HALT breaks this step) or an IRQ pending
        // its dispatch delay: keep cycle resolution.
        // A DMA stall from the last step is added on top of this step's
        // cycles, so skipping would overshoot the next event by that much.
        if (self.mmu.ie & self.mmu.if_reg) != 0
            || self.mmu.irq_assert_time.is_some()
            || self.mmu.pending_dma_stall() != 0
        {
            return 1;
        }
        n = n.min(self.mmu.ppu.cycles_to_next_boundary());
        n = n.min(self.mmu.timers.cycles_to_next_overflow(self.cpu.cycles));
        n = n.min(self.mmu.apu.cycles_to_next_sample());
        if self.mmu.sio.transfer_cycles_left > 0 {
            n = n.min(self.mmu.sio.transfer_cycles_left);
        }
        n.max(1)
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
        self.mmu.apu.speculative = self.speculative;

        // Synchronize M4A audio re-synthesis from WRAM and refill sample stream (ROADMAP M9)
        if !self.speculative
            && self.mmu.apu.hd_audio_mode == m4a::AudioEngineMode::HdReSynthesis
            && self.m4a.is_active()
        {
            if let Some(ref cart) = self.mmu.cartridge {
                self.m4a.sync_from_wram(&self.mmu.ewram[..], &self.mmu.iwram[..], &cart.rom);
                let has_hd = self.m4a.is_playing || self.m4a.sampler.voices.iter().any(|v| v.is_active);
                if has_hd {
                    while self.mmu.apu.hd_sample_stream.len() < 1024 {
                        let s = self.m4a.render_sample(&cart.rom);
                        self.mmu.apu.hd_sample_stream.push_back(s);
                    }
                }
            }
        }

        let mut frame_cycles = 0;
        self.batch_peripherals = self.batch_peripherals_enabled;
        while frame_cycles < CYCLES_PER_FRAME {
            self.halt_budget = CYCLES_PER_FRAME - frame_cycles;
            let c = self.step_instruction();
            frame_cycles += c;
        }
        self.halt_budget = 1;
        self.batch_peripherals = false;
        self.catch_up_peripherals();
        self.mmu.ppu.frame_ready = false;
        self.frame_counter += 1;

        // Emulated wall time for a deterministic RTC (whole seconds are all
        // the RTC reports, so once per frame is precise enough).
        if let Some(cart) = self.mmu.cartridge.as_mut() {
            cart.rtc.emulated_secs = (self.cpu.cycles / CPU_HZ) as i64;
        }

        // Apply active cheats on VBlank
        self.cheats.apply(&mut self.mmu);

        // Speculative (run-ahead) frames are silent and leave no trace.
        if self.speculative {
            self.mmu.apu.discard_samples();
            return;
        }

        // Flush frame audio samples and feed diagnostic linter
        self.mmu.apu.flush_samples();
        self.mmu.apu.audio_output.recover_lost_device();
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

    /// Dump current framebuffer as PNG (dumps HD frame if active, otherwise native 240x160)
    pub fn dump_frame_png<P: AsRef<Path>>(&self, path: P) -> std::io::Result<()> {
        if let Some(hd) = self.render_hd_frame() {
            let mut raw_bytes = Vec::with_capacity(hd.width * hd.height * 4);
            for &pixel in hd.pixels.iter() {
                raw_bytes.push((pixel & 0xFF) as u8);         // R
                raw_bytes.push(((pixel >> 8) & 0xFF) as u8);  // G
                raw_bytes.push(((pixel >> 16) & 0xFF) as u8); // B
                raw_bytes.push(((pixel >> 24) & 0xFF) as u8); // A
            }
            return image::save_buffer(
                path,
                &raw_bytes,
                hd.width as u32,
                hd.height as u32,
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));
        }

        let mut raw_bytes = Vec::with_capacity(SCREEN_WIDTH * SCREEN_HEIGHT * 4);
        for &pixel in self.mmu.ppu.completed_frame.iter() {
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

    /// The last complete frame (latched at VBlank), for display.
    pub fn get_framebuffer(&self) -> &[u32; SCREEN_WIDTH * SCREEN_HEIGHT] {
        &self.mmu.ppu.completed_frame
    }

    /// The live framebuffer, including lines drawn since the last VBlank.
    pub fn live_framebuffer(&self) -> &[u32; SCREEN_WIDTH * SCREEN_HEIGHT] {
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
        // Optional tail: the live framebuffer. `run_frame` stops mid-screen,
        // so its lower rows are still the previous frame, which the next
        // VBlank shows; without it they'd come from whatever was on screen
        // before loading. (The displayed frame is derived, not saved: run-
        // ahead replaces it, and the timeline must stay byte-identical.)
        w.bytes(STATE_FB_MAGIC);
        w.bytes(&self.mmu.ppu.framebuffer.iter().flat_map(|p| p.to_le_bytes()).collect::<Vec<u8>>());
        w.buf
    }

    /// Restore a state from `save_state`. On `false` (too short, corrupt, or
    /// malformed) the machine is left exactly as it was: loading writes RAM
    /// and registers before it reaches the later sections, so a failure
    /// part-way would otherwise leave a running game with a mix of both.
    pub fn load_state(&mut self, data: &[u8]) -> bool {
        let backup = self.save_state();
        let (pipe_valid, irq_assert_time) = (self.cpu.pipe_valid, self.mmu.irq_assert_time);
        if self.load_state_unchecked(data) {
            return true;
        }
        let restored = self.load_state_unchecked(&backup);
        debug_assert!(restored, "a state this core just saved must load");
        self.cpu.pipe_valid = pipe_valid;
        self.mmu.irq_assert_time = irq_assert_time;
        false
    }

    fn load_state_unchecked(&mut self, data: &[u8]) -> bool {
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

            // IME, IE, IF, WAITCNT; 4 DMA channels (sad, dad, count,
            // cnt_h, internal sad/dad/count, enabled); 4 timers (reload,
            // counter, cnt_h, enabled, prescaler, accumulator, cascade,
            // irq_enable).
            const DMA_CHANNEL: usize = 4 + 4 + 2 + 2 + 4 + 4 + 4 + 1;
            const TIMER: usize = 2 + 2 + 2 + 1 + 4 + 4 + 1 + 1;
            let need = 1 + 2 + 2 + 2 + 4 * DMA_CHANNEL + 4 * TIMER;
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

        self.m4a.sampler.stop_all();
        self.mmu.apu.hd_sample_stream.clear();
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
        // Optional framebuffer tail (see save_state).
        if r.remaining() > 0 && r.bytes()? == STATE_FB_MAGIC {
            let src = r.bytes()?;
            let fb = &mut self.mmu.ppu.framebuffer;
            if src.len() != fb.len() * 4 {
                return None;
            }
            for (px, b) in fb.iter_mut().zip(src.chunks_exact(4)) {
                *px = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            }
        }
        // Until the next VBlank, show the loaded picture as-is.
        let ppu = &mut self.mmu.ppu;
        ppu.completed_frame.copy_from_slice(&ppu.framebuffer[..]);
        Some(())
    }

    // ---- M4A Audio Re-synthesis (ROADMAP M9) -------------------------------

    /// Check whether the currently loaded game uses Nintendo's M4A sound engine
    pub fn is_m4a_game(&self) -> bool {
        self.m4a.is_m4a_game()
    }

    /// Set audio engine mode (HardwareOnly vs HdReSynthesis)
    pub fn set_hd_audio_mode(&mut self, mode: m4a::AudioEngineMode) {
        self.mmu.apu.set_hd_audio_mode(mode);
    }

    /// Current audio engine mode
    pub fn hd_audio_mode(&self) -> m4a::AudioEngineMode {
        self.mmu.apu.hd_audio_mode()
    }

    /// Play an M4A song by ID from the game's song table via the standalone HD sequencer
    pub fn play_m4a_song(&mut self, song_id: u16) -> bool {
        if let Some(ref cart) = self.mmu.cartridge {
            let res = self.m4a.play_song(&cart.rom, song_id);
            if res {
                self.mmu.apu.set_hd_audio_mode(m4a::AudioEngineMode::HdReSynthesis);
            }
            res
        } else {
            false
        }
    }

    /// Stop standalone HD sequencer playback
    pub fn stop_m4a_song(&mut self) {
        self.m4a.stop();
        self.mmu.apu.hd_sample_stream.clear();
        self.mmu.apu.set_hd_audio_mode(m4a::AudioEngineMode::HardwareOnly);
    }

    /// Export an M4A song as Standard MIDI File Type 1 (.mid)
    pub fn export_m4a_song_midi(&self, song_id: u16) -> Result<Vec<u8>, String> {
        let cart = self.mmu.cartridge.as_ref()
            .ok_or_else(|| "No cartridge loaded".to_string())?;
        self.m4a.export_midi(&cart.rom, song_id)
    }

    /// Export an M4A song as per-instrument multi-track 48 kHz WAV stems
    pub fn export_m4a_song_stems(&self, song_id: u16, duration_secs: f32) -> Result<Vec<m4a::StemTrack>, String> {
        let cart = self.mmu.cartridge.as_ref()
            .ok_or_else(|| "No cartridge loaded".to_string())?;
        self.m4a.export_stems(&cart.rom, song_id, duration_secs)
    }
}
