//! Game Boy / Game Boy Color system coordinator.
//!
//! Mirrors the structure of `gba::Gba` (`step_instruction` / `run_frame` /
//! `save_state` / `get_framebuffer`) so the UI, save manager, rewind buffer
//! and TAS engine can drive either core through the same shape.

pub mod apu;
pub mod cartridge;
pub mod cpu;
pub mod mmu;
pub mod ppu;

use cartridge::{CgbFlag, GbCartridge};
use cpu::Sm83;
use mmu::{GbKey, GbMmu};
use ppu::{GbPpu, GB_HEIGHT, GB_WIDTH};
use std::path::Path;

/// 4,194,304 Hz / 59.7275 fps. In double-speed mode the CPU executes twice
/// as many cycles per frame, but the PPU still draws 70224 dots, which is
/// why `run_frame` counts *PPU* cycles rather than CPU cycles.
pub const CYCLES_PER_FRAME: u32 = 70_224;

pub const STATE_MAGIC: &[u8; 4] = b"CBGB";

/// Which hardware model the loaded cartridge is being run as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GbModel {
    Dmg,
    Cgb,
}

pub struct GameBoy {
    pub cpu: Sm83,
    pub mmu: GbMmu,
    pub model: GbModel,
    pub is_running: bool,
    pub frame_counter: u64,
    /// Cycles left over from the previous frame, so pacing does not drift.
    cycle_debt: i64,
}

impl GameBoy {
    /// Build a system for `cart`. `force_dmg` runs a CGB-enhanced cartridge
    /// in original Game Boy mode (some games look better that way, and it is
    /// the only way to run a CGB-enhanced cart's DMG code path).
    pub fn with_cartridge(cart: GbCartridge, force_dmg: bool) -> Self {
        let cgb = !force_dmg && cart.cgb_flag != CgbFlag::None;
        let model = if cgb { GbModel::Cgb } else { GbModel::Dmg };
        let mut sys = Self {
            cpu: Sm83::new(),
            mmu: GbMmu::with_cartridge(cart, cgb),
            model,
            is_running: true,
            frame_counter: 0,
            cycle_debt: 0,
        };
        sys.reset();
        sys
    }

    pub fn from_file<P: AsRef<Path>>(path: P, force_dmg: bool) -> std::io::Result<Self> {
        let cart = GbCartridge::from_file(path)?;
        Ok(Self::with_cartridge(cart, force_dmg))
    }

    pub fn from_bytes(rom: Vec<u8>, force_dmg: bool) -> Self {
        Self::with_cartridge(GbCartridge::from_bytes(rom), force_dmg)
    }

    pub fn is_cgb(&self) -> bool {
        self.model == GbModel::Cgb
    }

    pub fn reset(&mut self) {
        let cgb = self.is_cgb();
        self.cpu = Sm83::new();
        self.cpu.reset_post_boot(cgb);
        self.mmu.ppu = GbPpu::new(cgb);
        self.mmu.if_reg = 0xE1;
        self.mmu.ie_reg = 0;
        self.mmu.div_counter = 0xABCC;
        self.mmu.tima = 0;
        self.mmu.tma = 0;
        self.mmu.tac = 0xF8;
        self.mmu.double_speed = false;
        self.mmu.key1 = 0;
        self.mmu.hdma_active = false;
        self.mmu.wram_bank = 1;
        self.frame_counter = 0;
        self.cycle_debt = 0;

        // Without a boot ROM the DMG palettes must be primed, otherwise a
        // game that never writes BGP renders as solid black.
        if !cgb {
            self.mmu.ppu.bgp = 0xFC;
            self.mmu.ppu.obp0 = 0xFF;
            self.mmu.ppu.obp1 = 0xFF;
        } else {
            // The CGB boot ROM leaves BG palette 0 as a greyscale ramp; a
            // CGB-enhanced game that only sets its own palettes still needs
            // sane defaults for anything it leaves untouched.
            for pal in 0..8 {
                for (c, shade) in [0x7FFFu16, 0x56B5, 0x294A, 0x0000].iter().enumerate() {
                    let idx = pal * 8 + c * 2;
                    self.mmu.ppu.bg_cram[idx] = (*shade & 0xFF) as u8;
                    self.mmu.ppu.bg_cram[idx + 1] = (*shade >> 8) as u8;
                    self.mmu.ppu.obj_cram[idx] = (*shade & 0xFF) as u8;
                    self.mmu.ppu.obj_cram[idx + 1] = (*shade >> 8) as u8;
                }
            }
        }
    }

    pub fn set_key(&mut self, key: GbKey, pressed: bool) {
        self.mmu.set_key(key, pressed);
    }

    /// Execute one instruction and advance every peripheral.
    /// Returns the number of **PPU dots** consumed (CPU cycles halved in
    /// double-speed mode), which is the unit `run_frame` paces on.
    pub fn step_instruction(&mut self) -> u32 {
        let cpu_cycles = self.cpu.step(&mut self.mmu);

        // In CGB double-speed the CPU and the timer/serial run twice as fast
        // as the PPU and APU. Dividing here (rather than doubling the frame
        // budget) keeps video and audio at their fixed real-time rates.
        let ppu_cycles = if self.mmu.double_speed {
            cpu_cycles / 2
        } else {
            cpu_cycles
        };

        if self.mmu.step_timer(cpu_cycles) {
            self.mmu.if_reg |= 0x04;
        }
        if self.mmu.step_serial(cpu_cycles) {
            self.mmu.if_reg |= 0x08;
        }

        let prev_mode = self.mmu.ppu.mode;
        let (vblank, stat) = self.mmu.ppu.step(ppu_cycles);
        if vblank {
            self.mmu.if_reg |= 0x01;
        }
        if stat {
            self.mmu.if_reg |= 0x02;
        }
        // HBlank DMA advances one 16-byte block per entry into mode 0.
        if self.mmu.hdma_active
            && prev_mode != ppu::PpuMode::HBlank
            && self.mmu.ppu.mode == ppu::PpuMode::HBlank
        {
            self.mmu.hdma_step_hblank();
        }

        self.mmu.apu.step(ppu_cycles);

        // STOP with no pending speed switch parks the CPU until a button is
        // pressed; treat the joypad IRQ flag as the wake condition.
        if self.cpu.stopped && (self.mmu.if_reg & 0x10) != 0 {
            self.cpu.stopped = false;
        }

        ppu_cycles.max(1)
    }

    pub fn run_frame(&mut self) {
        let budget = CYCLES_PER_FRAME as i64 - self.cycle_debt;
        let mut spent: i64 = 0;
        while spent < budget {
            spent += self.step_instruction() as i64;
        }
        self.cycle_debt = spent - budget;
        self.mmu.ppu.frame_ready = false;
        self.frame_counter += 1;

        self.mmu.apu.flush_samples();
        self.mmu.apu.audio_output.recover_lost_device();

        if self.frame_counter.is_multiple_of(60) {
            self.mmu.cart.sync_to_disk();
        }
    }

    /// The last complete frame (latched at VBlank), for display.
    pub fn get_framebuffer(&self) -> &[u32; GB_WIDTH * GB_HEIGHT] {
        &self.mmu.ppu.completed_frame
    }

    /// The live framebuffer, including lines drawn since the last VBlank.
    pub fn live_framebuffer(&self) -> &[u32; GB_WIDTH * GB_HEIGHT] {
        &self.mmu.ppu.framebuffer
    }

    /// Upscale the 160x144 GB image into a 240x160 GBA-sized buffer with a
    /// centered 1:1 letterbox. Lets every existing consumer of the GBA
    /// framebuffer (screen widget, GIF recorder, screenshot, AI agent vision)
    /// work unchanged against a GB core.
    pub fn framebuffer_gba_sized(&self) -> Vec<u32> {
        let (gw, gh) = (crate::gba::SCREEN_WIDTH, crate::gba::SCREEN_HEIGHT);
        let mut out = vec![0xFF00_0000u32; gw * gh];
        let x_off = (gw - GB_WIDTH) / 2; // 40
        let y_off = (gh - GB_HEIGHT) / 2; // 8
        for y in 0..GB_HEIGHT {
            let src = y * GB_WIDTH;
            let dst = (y + y_off) * gw + x_off;
            out[dst..dst + GB_WIDTH].copy_from_slice(&self.mmu.ppu.completed_frame[src..src + GB_WIDTH]);
        }
        out
    }

    pub fn dump_frame_png<P: AsRef<Path>>(&self, path: P) -> std::io::Result<()> {
        let mut raw = Vec::with_capacity(GB_WIDTH * GB_HEIGHT * 4);
        for &px in self.mmu.ppu.completed_frame.iter() {
            raw.push((px & 0xFF) as u8);
            raw.push(((px >> 8) & 0xFF) as u8);
            raw.push(((px >> 16) & 0xFF) as u8);
            raw.push(((px >> 24) & 0xFF) as u8);
        }
        image::save_buffer(
            path,
            &raw,
            GB_WIDTH as u32,
            GB_HEIGHT as u32,
            image::ExtendedColorType::Rgba8,
        )
        .map_err(std::io::Error::other)
    }

    // --- save states ----------------------------------------------------

    pub fn save_state(&self) -> Vec<u8> {
        let mut d = Vec::with_capacity(128 * 1024);
        d.extend_from_slice(STATE_MAGIC);
        d.push(self.is_cgb() as u8);

        // CPU
        d.extend_from_slice(&[
            self.cpu.a, self.cpu.f, self.cpu.b, self.cpu.c, self.cpu.d, self.cpu.e, self.cpu.h,
            self.cpu.l,
        ]);
        d.extend_from_slice(&self.cpu.sp.to_le_bytes());
        d.extend_from_slice(&self.cpu.pc.to_le_bytes());
        d.push(self.cpu.ime as u8);
        d.push(self.cpu.ime_pending as u8);
        d.push(self.cpu.halted as u8);
        d.push(self.cpu.halt_bug as u8);
        d.push(self.cpu.stopped as u8);
        d.extend_from_slice(&self.cpu.cycles.to_le_bytes());

        // Memory
        d.extend_from_slice(&(self.mmu.wram.len() as u32).to_le_bytes());
        d.extend_from_slice(&self.mmu.wram);
        d.extend_from_slice(&self.mmu.hram);
        d.extend_from_slice(&self.mmu.ppu.vram[0]);
        d.extend_from_slice(&self.mmu.ppu.vram[1]);
        d.extend_from_slice(&self.mmu.ppu.oam);
        d.extend_from_slice(&(self.mmu.cart.ram.len() as u32).to_le_bytes());
        d.extend_from_slice(&self.mmu.cart.ram);

        // MMU / timer / joypad
        d.push(self.mmu.ie_reg);
        d.push(self.mmu.if_reg);
        d.extend_from_slice(&self.mmu.div_counter.to_le_bytes());
        d.push(self.mmu.tima);
        d.push(self.mmu.tma);
        d.push(self.mmu.tac);
        d.push(self.mmu.wram_bank as u8);
        d.push(self.mmu.double_speed as u8);
        d.push(self.mmu.key1);
        d.extend_from_slice(&self.mmu.hdma_src.to_le_bytes());
        d.extend_from_slice(&self.mmu.hdma_dst.to_le_bytes());
        d.push(self.mmu.hdma_len);
        d.push(self.mmu.hdma_active as u8);

        // Cartridge bank state
        d.extend_from_slice(&(self.mmu.cart.rom_bank as u32).to_le_bytes());
        d.extend_from_slice(&(self.mmu.cart.ram_bank as u32).to_le_bytes());
        d.push(self.mmu.cart.ram_enabled as u8);
        d.push(self.mmu.cart.mbc1_mode);
        d.extend_from_slice(&self.mmu.cart.rtc_regs);

        // PPU
        d.extend_from_slice(&[
            self.mmu.ppu.lcdc,
            self.mmu.ppu.stat,
            self.mmu.ppu.scy,
            self.mmu.ppu.scx,
            self.mmu.ppu.ly,
            self.mmu.ppu.lyc,
            self.mmu.ppu.bgp,
            self.mmu.ppu.obp0,
            self.mmu.ppu.obp1,
            self.mmu.ppu.wy,
            self.mmu.ppu.wx,
            self.mmu.ppu.bcps,
            self.mmu.ppu.ocps,
            self.mmu.ppu.vram_bank as u8,
        ]);
        d.extend_from_slice(&self.mmu.ppu.bg_cram);
        d.extend_from_slice(&self.mmu.ppu.obj_cram);
        d.extend_from_slice(&self.mmu.ppu.dot.to_le_bytes());
        d.extend_from_slice(&self.frame_counter.to_le_bytes());

        // APU registers (channel state is re-derived from the next trigger;
        // storing NR50/51/power keeps mixing correct across a load).
        d.push(self.mmu.apu.nr50);
        d.push(self.mmu.apu.nr51);
        d.push(self.mmu.apu.power as u8);

        d
    }

    /// Restore a state from `save_state`. On `false` the machine is left
    /// exactly as it was (see `Gba::load_state`).
    pub fn load_state(&mut self, data: &[u8]) -> bool {
        let backup = self.save_state();
        if self.load_state_unchecked(data) {
            return true;
        }
        let restored = self.load_state_unchecked(&backup);
        debug_assert!(restored, "a state this core just saved must load");
        false
    }

    fn load_state_unchecked(&mut self, data: &[u8]) -> bool {
        if data.len() < 5 || &data[0..4] != STATE_MAGIC {
            return false;
        }
        let mut o = 4;
        let cgb = data[o] != 0;
        o += 1;
        if cgb != self.is_cgb() {
            log::warn!("GB save state model mismatch (state cgb={}, running cgb={})", cgb, self.is_cgb());
            return false;
        }

        macro_rules! take {
            ($n:expr) => {{
                if data.len() < o + $n {
                    return false;
                }
                let s = &data[o..o + $n];
                o += $n;
                s
            }};
        }

        let r = take!(8);
        self.cpu.a = r[0];
        self.cpu.f = r[1] & 0xF0;
        self.cpu.b = r[2];
        self.cpu.c = r[3];
        self.cpu.d = r[4];
        self.cpu.e = r[5];
        self.cpu.h = r[6];
        self.cpu.l = r[7];
        self.cpu.sp = u16::from_le_bytes(take!(2).try_into().unwrap());
        self.cpu.pc = u16::from_le_bytes(take!(2).try_into().unwrap());
        self.cpu.ime = take!(1)[0] != 0;
        self.cpu.ime_pending = take!(1)[0] != 0;
        self.cpu.halted = take!(1)[0] != 0;
        self.cpu.halt_bug = take!(1)[0] != 0;
        self.cpu.stopped = take!(1)[0] != 0;
        self.cpu.cycles = u64::from_le_bytes(take!(8).try_into().unwrap());

        let wram_len = u32::from_le_bytes(take!(4).try_into().unwrap()) as usize;
        if wram_len != self.mmu.wram.len() {
            return false;
        }
        self.mmu.wram.copy_from_slice(take!(wram_len));
        self.mmu.hram.copy_from_slice(take!(0x7F));
        self.mmu.ppu.vram[0].copy_from_slice(take!(0x2000));
        self.mmu.ppu.vram[1].copy_from_slice(take!(0x2000));
        self.mmu.ppu.oam.copy_from_slice(take!(0xA0));
        let ram_len = u32::from_le_bytes(take!(4).try_into().unwrap()) as usize;
        if ram_len != self.mmu.cart.ram.len() {
            return false;
        }
        if ram_len > 0 {
            self.mmu.cart.ram.copy_from_slice(take!(ram_len));
        }

        self.mmu.ie_reg = take!(1)[0];
        self.mmu.if_reg = take!(1)[0];
        self.mmu.div_counter = u16::from_le_bytes(take!(2).try_into().unwrap());
        self.mmu.tima = take!(1)[0];
        self.mmu.tma = take!(1)[0];
        self.mmu.tac = take!(1)[0];
        self.mmu.wram_bank = take!(1)[0] as usize;
        self.mmu.double_speed = take!(1)[0] != 0;
        self.mmu.key1 = take!(1)[0];
        self.mmu.hdma_src = u16::from_le_bytes(take!(2).try_into().unwrap());
        self.mmu.hdma_dst = u16::from_le_bytes(take!(2).try_into().unwrap());
        self.mmu.hdma_len = take!(1)[0];
        self.mmu.hdma_active = take!(1)[0] != 0;

        self.mmu.cart.rom_bank = u32::from_le_bytes(take!(4).try_into().unwrap()) as usize;
        self.mmu.cart.ram_bank = u32::from_le_bytes(take!(4).try_into().unwrap()) as usize;
        self.mmu.cart.ram_enabled = take!(1)[0] != 0;
        self.mmu.cart.mbc1_mode = take!(1)[0];
        self.mmu.cart.rtc_regs.copy_from_slice(take!(5));

        let p = take!(14);
        self.mmu.ppu.lcdc = p[0];
        self.mmu.ppu.stat = p[1];
        self.mmu.ppu.scy = p[2];
        self.mmu.ppu.scx = p[3];
        self.mmu.ppu.ly = p[4];
        self.mmu.ppu.lyc = p[5];
        self.mmu.ppu.bgp = p[6];
        self.mmu.ppu.obp0 = p[7];
        self.mmu.ppu.obp1 = p[8];
        self.mmu.ppu.wy = p[9];
        self.mmu.ppu.wx = p[10];
        self.mmu.ppu.bcps = p[11];
        self.mmu.ppu.ocps = p[12];
        self.mmu.ppu.vram_bank = p[13] as usize;
        self.mmu.ppu.bg_cram.copy_from_slice(take!(64));
        self.mmu.ppu.obj_cram.copy_from_slice(take!(64));
        self.mmu.ppu.dot = u32::from_le_bytes(take!(4).try_into().unwrap());
        self.frame_counter = u64::from_le_bytes(take!(8).try_into().unwrap());

        self.mmu.apu.nr50 = take!(1)[0];
        self.mmu.apu.nr51 = take!(1)[0];
        self.mmu.apu.power = take!(1)[0] != 0;
        let _ = o; // final cursor position is intentionally unused

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal ROM: infinite loop at 0x0150 reached via a jump from 0x0100.
    fn spin_rom(cgb: bool) -> Vec<u8> {
        let mut rom = vec![0u8; 0x8000];
        rom[0x0143] = if cgb { 0xC0 } else { 0x00 };
        rom[0x0147] = 0x00;
        rom[0x0100] = 0xC3; // JP 0x0150
        rom[0x0101] = 0x50;
        rom[0x0102] = 0x01;
        rom[0x0150] = 0x18; // JR -2
        rom[0x0151] = 0xFE;
        rom
    }

    #[test]
    fn dmg_cart_boots_with_dmg_register_state() {
        let gb = GameBoy::from_bytes(spin_rom(false), false);
        assert_eq!(gb.model, GbModel::Dmg);
        assert_eq!(gb.cpu.a, 0x01, "DMG boot leaves A=0x01");
        assert_eq!(gb.cpu.pc, 0x0100);
    }

    #[test]
    fn cgb_cart_boots_in_cgb_mode_with_a_eq_11() {
        let gb = GameBoy::from_bytes(spin_rom(true), false);
        assert_eq!(gb.model, GbModel::Cgb);
        assert_eq!(gb.cpu.a, 0x11, "CGB boot leaves A=0x11 for detection");
        assert!(gb.is_cgb());
    }

    #[test]
    fn force_dmg_runs_a_cgb_cart_as_dmg() {
        let gb = GameBoy::from_bytes(spin_rom(true), true);
        assert_eq!(gb.model, GbModel::Dmg);
        assert_eq!(gb.cpu.a, 0x01);
    }

    #[test]
    fn a_frame_advances_the_ppu_through_one_full_vblank() {
        let mut gb = GameBoy::from_bytes(spin_rom(false), false);
        let before = gb.mmu.ppu.frame_counter;
        gb.run_frame();
        assert_eq!(gb.mmu.ppu.frame_counter, before + 1);
        assert_eq!(gb.frame_counter, 1);
    }

    #[test]
    fn sixty_frames_advance_about_one_second_of_cpu_cycles() {
        let mut gb = GameBoy::from_bytes(spin_rom(false), false);
        for _ in 0..60 {
            gb.run_frame();
        }
        let expected = 4_194_304f64;
        let actual = gb.cpu.cycles as f64;
        assert!(
            (actual - expected).abs() / expected < 0.02,
            "60 frames should be ~1s of cycles, got {actual}"
        );
    }

    #[test]
    fn save_state_round_trips() {
        let mut gb = GameBoy::from_bytes(spin_rom(false), false);
        for _ in 0..10 {
            gb.run_frame();
        }
        gb.mmu.write(0xC123, 0x5A);
        let state = gb.save_state();
        let (pc, sp, wram) = (gb.cpu.pc, gb.cpu.sp, gb.mmu.read(0xC123));

        for _ in 0..10 {
            gb.run_frame();
        }
        gb.mmu.write(0xC123, 0x00);

        assert!(gb.load_state(&state));
        assert_eq!(gb.cpu.pc, pc);
        assert_eq!(gb.cpu.sp, sp);
        assert_eq!(gb.mmu.read(0xC123), wram);
        assert_eq!(gb.frame_counter, 10);
    }

    #[test]
    fn a_truncated_state_leaves_the_game_untouched() {
        let mut src = GameBoy::from_bytes(spin_rom(false), false);
        src.run_frame();
        src.mmu.write(0xC123, 0x5A);
        let state = src.save_state();
        let mut gb = GameBoy::from_bytes(spin_rom(false), false);
        gb.run_frame();
        let before = gb.save_state();
        for cut in (0..state.len()).step_by(97) {
            assert!(!gb.load_state(&state[..cut]), "cut {cut}");
            assert!(gb.save_state() == before, "cut {cut}: failed load changed the game");
        }
    }

    #[test]
    fn save_state_rejects_a_model_mismatch() {
        let dmg_state = GameBoy::from_bytes(spin_rom(false), false).save_state();
        let mut cgb = GameBoy::from_bytes(spin_rom(true), false);
        assert!(!cgb.load_state(&dmg_state), "DMG state must not load into CGB");
    }

    #[test]
    fn gba_sized_framebuffer_letterboxes_the_gb_image() {
        let gb = GameBoy::from_bytes(spin_rom(false), false);
        let fb = gb.framebuffer_gba_sized();
        assert_eq!(fb.len(), crate::gba::SCREEN_WIDTH * crate::gba::SCREEN_HEIGHT);
        // Top-left must be letterbox black, center must be GB content.
        assert_eq!(fb[0], 0xFF00_0000);
        let center = (8 + 72) * crate::gba::SCREEN_WIDTH + 40 + 80;
        assert_eq!(fb[center], gb.get_framebuffer()[72 * GB_WIDTH + 80]);
    }

    #[test]
    fn double_speed_halves_ppu_cycles_per_instruction() {
        let mut gb = GameBoy::from_bytes(spin_rom(true), false);
        gb.mmu.double_speed = true;
        let before = gb.cpu.cycles;
        let dots = gb.step_instruction();
        let cpu = gb.cpu.cycles - before;
        assert_eq!(dots as u64, cpu / 2, "PPU runs at half CPU rate in 2x mode");
    }
}
