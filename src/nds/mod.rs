//! Nintendo DS System Coordinator
//!
//! Orchestrates the dual ARM9/ARM7 CPUs, memory bus, IPC, 2D/3D graphics,
//! audio, SPI (touch/firmware/PMIC), and Slot-1 game card controllers.

pub mod bios;
pub mod bus;
pub mod card;
pub mod cpu;
pub mod ipc;
pub mod math;
pub mod ppu;
pub mod spi;
pub mod spu;

use bus::NdsBus;
use card::NdsCard;
use cpu::{step_arm7, step_arm9, Arm7Tdmi, Arm946eS};
use ipc::Ipc;
use std::path::Path;

pub const SCREEN_WIDTH: usize = 256;
pub const SCREEN_HEIGHT: usize = 192;
pub const DUAL_SCREEN_HEIGHT: usize = SCREEN_HEIGHT * 2; // 384
pub const TOTAL_SCANLINES: usize = 263;

// Clock frequencies and per-line cycle budgets
pub const ARM9_CYCLES_PER_LINE: u32 = 4260; // 67.028 MHz
pub const ARM7_CYCLES_PER_LINE: u32 = 2130; // 33.514 MHz
pub const SYS_CYCLES_PER_LINE: u32 = 2130;
pub const HBLANK_SYS_CYCLE: u32 = 1680;     // ~pixel 256

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NdsKey {
    A,
    B,
    Select,
    Start,
    Right,
    Left,
    Up,
    Down,
    R,
    L,
    X,
    Y,
}

pub struct Nds {
    pub arm9: Arm946eS,
    pub arm7: Arm7Tdmi,
    pub bus: NdsBus,
    pub frame_counter: u64,
    pub is_running: bool,
}

impl Default for Nds {
    fn default() -> Self {
        Self::new()
    }
}

impl Nds {
    pub fn new() -> Self {
        Self {
            arm9: Arm946eS::new(),
            arm7: Arm7Tdmi::new(),
            bus: NdsBus::new(),
            frame_counter: 0,
            is_running: true,
        }
    }

    #[inline]
    pub fn card(&self) -> &NdsCard {
        &self.bus.card
    }

    #[inline]
    pub fn card_mut(&mut self) -> &mut NdsCard {
        &mut self.bus.card
    }

    #[inline]
    pub fn ipc(&self) -> &Ipc {
        &self.bus.ipc
    }

    #[inline]
    pub fn ipc_mut(&mut self) -> &mut Ipc {
        &mut self.bus.ipc
    }

    pub fn load_rom(&mut self, path: &Path) -> Result<(), String> {
        self.bus.card.load_from_file(path)?;
        let h = self.bus.card.header.clone();

        // Copy ARM9 executable into Main RAM or ITCM
        let arm9_rom_start = h.arm9_rom_offset as usize;
        let arm9_rom_end = arm9_rom_start + (h.arm9_size as usize);
        if arm9_rom_end <= self.bus.card.rom.len() {
            let arm9_slice = &self.bus.card.rom[arm9_rom_start..arm9_rom_end];
            if (0x0200_0000..0x0240_0000).contains(&h.arm9_ram_addr) {
                let ram_offset = (h.arm9_ram_addr - 0x0200_0000) as usize;
                if ram_offset + arm9_slice.len() <= self.bus.main_ram.len() {
                    self.bus.main_ram[ram_offset..ram_offset + arm9_slice.len()].copy_from_slice(arm9_slice);
                    log::info!("Loaded ARM9 payload ({} bytes) to Main RAM offset 0x{:06X}", arm9_slice.len(), ram_offset);
                }
            } else if (0x0000_0000..0x0000_8000).contains(&h.arm9_ram_addr) {
                let itcm_offset = h.arm9_ram_addr as usize;
                if itcm_offset + arm9_slice.len() <= self.bus.itcm.len() {
                    self.bus.itcm[itcm_offset..itcm_offset + arm9_slice.len()].copy_from_slice(arm9_slice);
                    log::info!("Loaded ARM9 payload ({} bytes) to ITCM offset 0x{:06X}", arm9_slice.len(), itcm_offset);
                }
            }
        }
        self.arm9.regs[15] = h.arm9_entry_addr;

        // Copy ARM7 executable into Main RAM, WRAM, or ARM7 WRAM
        let arm7_rom_start = h.arm7_rom_offset as usize;
        let arm7_rom_end = arm7_rom_start + (h.arm7_size as usize);
        if arm7_rom_end <= self.bus.card.rom.len() {
            let arm7_slice = &self.bus.card.rom[arm7_rom_start..arm7_rom_end];
            if (0x0200_0000..0x0240_0000).contains(&h.arm7_ram_addr) {
                let ram_offset = (h.arm7_ram_addr - 0x0200_0000) as usize;
                if ram_offset + arm7_slice.len() <= self.bus.main_ram.len() {
                    self.bus.main_ram[ram_offset..ram_offset + arm7_slice.len()].copy_from_slice(arm7_slice);
                    log::info!("Loaded ARM7 payload ({} bytes) to Main RAM offset 0x{:06X}", arm7_slice.len(), ram_offset);
                }
            } else if (0x0380_0000..0x0381_0000).contains(&h.arm7_ram_addr) {
                let wram_offset = (h.arm7_ram_addr - 0x0380_0000) as usize;
                if wram_offset + arm7_slice.len() <= self.bus.arm7_wram.len() {
                    self.bus.arm7_wram[wram_offset..wram_offset + arm7_slice.len()].copy_from_slice(arm7_slice);
                    log::info!("Loaded ARM7 payload ({} bytes) to ARM7 WRAM offset 0x{:06X}", arm7_slice.len(), wram_offset);
                }
            } else if (0x037F_8000..0x0380_0000).contains(&h.arm7_ram_addr) {
                let wram_offset = (h.arm7_ram_addr - 0x037F_8000) as usize;
                if wram_offset + arm7_slice.len() <= self.bus.shared_wram.len() {
                    self.bus.shared_wram[wram_offset..wram_offset + arm7_slice.len()].copy_from_slice(arm7_slice);
                    log::info!("Loaded ARM7 payload ({} bytes) to Shared WRAM offset 0x{:06X}", arm7_slice.len(), wram_offset);
                }
            }
        }
        self.arm7.regs[15] = h.arm7_entry_addr;

        // Set post-boot flags to bypass splash firmware
        self.bus.postflg_arm9 = 1;
        self.bus.postflg_arm7 = 1;
        self.direct_boot_setup();

        Ok(())
    }

    /// Recreate the state the DS BIOS and firmware leave for a game (GBATEK
    /// "DS Firmware - Boot"): cartridge header and chip IDs at the top of main
    /// RAM, firmware user settings, shared WRAM given to the ARM7, the card
    /// ready to serve KEY2 reads, and each CPU's stacks.
    fn direct_boot_setup(&mut self) {
        let b = &mut self.bus;
        let put32 = |ram: &mut [u8], addr: u32, v: u32| {
            let o = (addr & 0x3F_FFFF) as usize;
            ram[o..o + 4].copy_from_slice(&v.to_le_bytes());
        };
        let put16 = |ram: &mut [u8], addr: u32, v: u16| {
            let o = (addr & 0x3F_FFFF) as usize;
            ram[o..o + 2].copy_from_slice(&v.to_le_bytes());
        };

        // Cartridge header copy (0x027FFE00, 0x170 bytes).
        let hdr_len = 0x170.min(b.card.rom.len());
        let hdr = b.card.rom[..hdr_len].to_vec();
        let o = 0x3F_FE00;
        b.main_ram[o..o + hdr_len].copy_from_slice(&hdr);

        // Card chip IDs and boot indicators.
        let chip = b.card.chip_id();
        for a in [0x027F_F800, 0x027F_F804, 0x027F_FC00, 0x027F_FC04] {
            put32(&mut b.main_ram[..], a, chip);
        }
        let header_crc = u16::from_le_bytes([hdr.get(0x15E).copied().unwrap_or(0), hdr.get(0x15F).copied().unwrap_or(0)]);
        put16(&mut b.main_ram[..], 0x027F_F808, header_crc);
        put16(&mut b.main_ram[..], 0x027F_FC08, header_crc);
        let sec_crc = u16::from_le_bytes([hdr.get(0x6C).copied().unwrap_or(0), hdr.get(0x6D).copied().unwrap_or(0)]);
        put16(&mut b.main_ram[..], 0x027F_F80A, sec_crc);
        put16(&mut b.main_ram[..], 0x027F_FC0A, sec_crc);
        put16(&mut b.main_ram[..], 0x027F_F850, 0x5835); // secure area disable pattern seen after boot
        put16(&mut b.main_ram[..], 0x027F_FC10, 0x5835);
        put16(&mut b.main_ram[..], 0x027F_FC30, 0xFFFF);
        put16(&mut b.main_ram[..], 0x027F_FC40, 1); // boot indicator: normal card boot

        // Firmware user settings (0x027FFC80, 0x70 bytes) from the firmware image.
        let settings: Vec<u8> = b.spi.nvram.user_settings()[..0x70].to_vec();
        b.main_ram[0x3F_FC80..0x3F_FC80 + 0x70].copy_from_slice(&settings);

        // Memory control as left by the BIOS.
        b.vramcnt[7] = 3; // WRAMCNT: all 32 KiB shared WRAM to the ARM7
        b.exmemcnt = 0x6000;
        b.card.romctrl = 0x0000_6000 | (1 << 29); // KEY2 mode ready

        // Stacks (the BIOS sets these; many games only set SP_usr/sys).
        // ARM9 IRQ/SVC stacks keep the constructor defaults; SP_sys in DTCM.
        self.arm9.regs[13] = 0x027C_3E00; // top of DTCM, below the IRQ area
        let a7 = &mut self.arm7;
        a7.r13_irq = 0x0380_FF80;
        a7.r13_svc = 0x0380_FFC0;
        a7.regs[13] = 0x0380_FD80;
        a7.r13_usr = 0x0380_FD80;
        let (e9, e7) = (self.bus.card.header.arm9_entry_addr, self.bus.card.header.arm7_entry_addr);
        self.arm9.regs[12] = e9;
        self.arm9.regs[14] = e9;
        self.arm7.regs[12] = e7;
        self.arm7.regs[14] = e7;
    }

    /// Set standard GBA-compatible keypad button
    pub fn set_key(&mut self, key: crate::gba::keypad::Key, pressed: bool) {
        use crate::gba::keypad::Key;
        let bit = match key {
            Key::A => 0,
            Key::B => 1,
            Key::Select => 2,
            Key::Start => 3,
            Key::Right => 4,
            Key::Left => 5,
            Key::Up => 6,
            Key::Down => 7,
            Key::R => 8,
            Key::L => 9,
        };
        if pressed {
            self.bus.keyinput &= !(1 << bit);
        } else {
            self.bus.keyinput |= 1 << bit;
        }
    }

    /// Set full NDS keypad buttons including X and Y
    pub fn set_nds_key(&mut self, key: NdsKey, pressed: bool) {
        match key {
            NdsKey::A => self.set_key(crate::gba::keypad::Key::A, pressed),
            NdsKey::B => self.set_key(crate::gba::keypad::Key::B, pressed),
            NdsKey::Select => self.set_key(crate::gba::keypad::Key::Select, pressed),
            NdsKey::Start => self.set_key(crate::gba::keypad::Key::Start, pressed),
            NdsKey::Right => self.set_key(crate::gba::keypad::Key::Right, pressed),
            NdsKey::Left => self.set_key(crate::gba::keypad::Key::Left, pressed),
            NdsKey::Up => self.set_key(crate::gba::keypad::Key::Up, pressed),
            NdsKey::Down => self.set_key(crate::gba::keypad::Key::Down, pressed),
            NdsKey::R => self.set_key(crate::gba::keypad::Key::R, pressed),
            NdsKey::L => self.set_key(crate::gba::keypad::Key::L, pressed),
            NdsKey::X => {
                if pressed {
                    self.bus.extkeyin &= !(1 << 0);
                } else {
                    self.bus.extkeyin |= 1 << 0;
                }
            }
            NdsKey::Y => {
                if pressed {
                    self.bus.extkeyin &= !(1 << 1);
                } else {
                    self.bus.extkeyin |= 1 << 1;
                }
            }
        }
    }

    /// Set touchscreen stylus position (x: 0..255, y: 0..191)
    pub fn set_touch(&mut self, coords: Option<(u16, u16)>) {
        self.bus.spi.touch.set_touch(coords);
        if coords.is_some() {
            // Pen down bit (bit 6 of EXTKEYIN is 0 when touching)
            self.bus.extkeyin &= !(1 << 6);
        } else {
            // Pen up bit
            self.bus.extkeyin |= 1 << 6;
        }
    }

    /// Runs one complete frame (263 scanlines, ~59.83 Hz) with interleaved dual-CPU execution
    pub fn run_frame(&mut self) {
        if !self.is_running {
            return;
        }

        for line in 0..TOTAL_SCANLINES {
            self.bus.ppu.set_scanline(line as u16);

            // V-Counter Match IRQ
            if (self.bus.ppu.dispstat_a & (1 << 2)) != 0 && (self.bus.ppu.dispstat_a & (1 << 5)) != 0 {
                self.bus.if_arm9 |= 1 << 2;
            }
            if (self.bus.ppu.dispstat_b & (1 << 2)) != 0 && (self.bus.ppu.dispstat_b & (1 << 5)) != 0 {
                self.bus.if_arm7 |= 1 << 2;
            }

            if line == 0 {
                self.bus.trigger_dma(crate::nds::bus::DmaEvent::DisplayStart);
            }

            // VBlank Start IRQ (Line 192)
            if line == 192 {
                self.bus.trigger_dma(crate::nds::bus::DmaEvent::VBlank);
                self.bus.ppu.engine_3d.on_vblank(&self.bus.vram_a[..], &self.bus.ppu.engine_a.palette);
                if (self.bus.ppu.dispstat_a & (1 << 3)) != 0 {
                    self.bus.if_arm9 |= 1 << 0;
                }
                if (self.bus.ppu.dispstat_b & (1 << 3)) != 0 {
                    self.bus.if_arm7 |= 1 << 0;
                }
            }

            // Stepping loop across the scanline (2130 sys cycles, 4260 ARM9 cycles, 2130 ARM7 cycles)
            let mut sys_cycles_done = 0;
            let mut entered_hblank = false;

            while sys_cycles_done < SYS_CYCLES_PER_LINE {
                let chunk_sys = 32.min(SYS_CYCLES_PER_LINE - sys_cycles_done);
                let chunk_arm9 = chunk_sys * 2;

                // Step ARM9 core for chunk_arm9 cycles
                let mut a9_cycles = 0;
                while a9_cycles < chunk_arm9 {
                    let c = step_arm9(&mut self.arm9, &mut self.bus);
                    a9_cycles += c;
                }

                // Step ARM7 core for chunk_sys cycles
                let mut a7_cycles = 0;
                while a7_cycles < chunk_sys {
                    let c = step_arm7(&mut self.arm7, &mut self.bus);
                    a7_cycles += c;
                }

                // Tick timers
                self.bus.step_timers(chunk_sys, true);  // ARM9 timers
                self.bus.step_timers(chunk_sys, false); // ARM7 timers

                // Step SPU (Sound Processing Unit)
                self.bus.step_spu(chunk_sys);

                sys_cycles_done += chunk_sys;

                // Check HBlank entry
                if !entered_hblank && sys_cycles_done >= HBLANK_SYS_CYCLE {
                    entered_hblank = true;
                    if line < SCREEN_HEIGHT {
                        self.bus.trigger_dma(crate::nds::bus::DmaEvent::HBlank);
                    }
                    self.bus.ppu.dispstat_a |= 1 << 1;
                    self.bus.ppu.dispstat_b |= 1 << 1;
                    if (self.bus.ppu.dispstat_a & (1 << 4)) != 0 {
                        self.bus.if_arm9 |= 1 << 1;
                    }
                    if (self.bus.ppu.dispstat_b & (1 << 4)) != 0 {
                        self.bus.if_arm7 |= 1 << 1;
                    }
                }

                // Wake halted CPUs on pending interrupts & dispatch IRQ if enabled
                if (self.bus.if_arm9 & self.bus.ie_arm9) != 0 {
                    self.arm9.halted = false;
                    if self.bus.ime_arm9 && (self.arm9.cpsr & crate::gba::cpu::FLAG_I) == 0 {
                        self.arm9.trigger_irq();
                    }
                }

                if (self.bus.if_arm7 & self.bus.ie_arm7) != 0 {
                    self.arm7.halted = false;
                    if self.bus.ime_arm7 && (self.arm7.cpsr & crate::gba::cpu::FLAG_I) == 0 {
                        self.arm7.trigger_irq();
                    }
                }
            }

            // Clear HBlank at end of scanline
            self.bus.ppu.dispstat_a &= !(1 << 1);
            self.bus.ppu.dispstat_b &= !(1 << 1);

            // Render visible scanlines (lines 0..191)
            if line < SCREEN_HEIGHT {
                if line == 0 {
                    // Games update VRAM during VBlank; one flatten per frame.
                    self.bus.build_vram_views();
                }
                let bus = &mut self.bus;
                let lcdc = [&bus.vram_a[..], &bus.vram_b[..], &bus.vram_c[..], &bus.vram_d[..]];
                bus.ppu.render_scanline(line, lcdc, &bus.vram_views);
            }
        }

        self.bus.spu.flush_samples();
        self.frame_counter += 1;
    }

    pub fn get_framebuffer(&self) -> &[u32] {
        &self.bus.ppu.framebuffer
    }

    pub fn title(&self) -> &str {
        if self.bus.card.header.title.is_empty() {
            "Nintendo DS Game"
        } else {
            &self.bus.card.header.title
        }
    }

    pub fn game_code(&self) -> &str {
        &self.bus.card.header.game_code
    }

    pub fn flush_save(&mut self) {
        self.bus.card.backup.flush();
    }

    pub fn save_state(&self) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(b"NDS1");
        data.extend_from_slice(&self.frame_counter.to_le_bytes());
        data
    }

    pub fn load_state(&mut self, data: &[u8]) -> bool {
        if data.len() < 12 || &data[0..4] != b"NDS1" {
            return false;
        }
        self.frame_counter = u64::from_le_bytes(data[4..12].try_into().unwrap());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nds::card::NdsHeader;

    #[test]
    fn test_ipc_sync_handshake() {
        let mut sync = ipc::IpcSync::default();
        sync.arm7_irq_enable = true;

        // ARM9 writes nibble 0xA to ARM7 with IRQ
        sync.write_arm9(0x2A00); // bit 13 set, data = 0xA
        assert_eq!(sync.read_arm7() & 0x0F, 0x0A);
        assert!(sync.irq_to_arm7);
        sync.irq_to_arm7 = false;

        // ARM7 writes nibble 0x5 to ARM9
        sync.arm9_irq_enable = true;
        sync.write_arm7(0x2500); // bit 13 set, data = 0x5
        assert_eq!(sync.read_arm9() & 0x0F, 0x05);
        assert!(sync.irq_to_arm9);
    }

    #[test]
    fn test_ipc_fifo_queue() {
        let mut fifo = ipc::IpcFifo::default();
        fifo.write_cnt_arm9(1 << 15); // Enable ARM9
        fifo.write_cnt_arm7(1 << 15); // Enable ARM7

        // Send 3 values from ARM9 to ARM7
        fifo.write_data_arm9(0x12345678);
        fifo.write_data_arm9(0xDEADBEEF);
        fifo.write_data_arm9(0xCAFEBABE);

        assert_eq!(fifo.queue_9_to_7.len(), 3);
        assert_eq!(fifo.read_data_arm7(), 0x12345678);
        assert_eq!(fifo.read_data_arm7(), 0xDEADBEEF);
        assert_eq!(fifo.read_data_arm7(), 0xCAFEBABE);
        assert_eq!(fifo.queue_9_to_7.len(), 0);
    }

    #[test]
    fn test_nds_header_parser() {
        let mut dummy_rom = vec![0u8; 1024];
        dummy_rom[0x00..0x0A].copy_from_slice(b"MARIO KART");
        dummy_rom[0x0C..0x10].copy_from_slice(b"AMKE");
        dummy_rom[0x10..0x12].copy_from_slice(b"01");
        dummy_rom[0x20..0x24].copy_from_slice(&0x4000u32.to_le_bytes()); // ARM9 ROM offset
        dummy_rom[0x24..0x28].copy_from_slice(&0x02000800u32.to_le_bytes()); // ARM9 entry
        dummy_rom[0x28..0x2C].copy_from_slice(&0x02000000u32.to_le_bytes()); // ARM9 RAM
        dummy_rom[0x2C..0x30].copy_from_slice(&0x20000u32.to_le_bytes()); // ARM9 size

        let header = NdsHeader::parse(&dummy_rom).expect("header parse failed");
        assert_eq!(header.title, "MARIO KART");
        assert_eq!(header.game_code, "AMKE");
        assert_eq!(header.maker_code, "01");
        assert_eq!(header.arm9_rom_offset, 0x4000);
        assert_eq!(header.arm9_entry_addr, 0x02000800);
        assert_eq!(header.arm9_ram_addr, 0x02000000);
        assert_eq!(header.arm9_size, 0x20000);
    }

    #[test]
    fn test_arm9_instructions() {
        let mut arm9 = cpu::Arm946eS::new();

        // CLZ test
        arm9.regs[1] = 0x00000001;
        arm9.op_clz(0, 1);
        assert_eq!(arm9.regs[0], 31);

        arm9.regs[1] = 0x80000000;
        arm9.op_clz(0, 1);
        assert_eq!(arm9.regs[0], 0);

        // QADD saturating add test
        arm9.regs[1] = 0x7FFF_FFFF;
        arm9.regs[2] = 0x0000_0001;
        arm9.op_qadd(0, 1, 2);
        assert_eq!(arm9.regs[0], 0x7FFF_FFFF); // saturated
        assert_ne!(arm9.cpsr & cpu::executor::FLAG_Q, 0);

        // SMULxy signed halfword multiply test
        arm9.regs[1] = 0x0003_0005; // top=3, bottom=5
        arm9.regs[2] = 0x0004_0002; // top=4, bottom=2
        arm9.op_smulxy(0, 1, 2, false, false); // 5 * 2 = 10
        assert_eq!(arm9.regs[0], 10);

        arm9.op_smulxy(0, 1, 2, true, true); // 3 * 4 = 12
        assert_eq!(arm9.regs[0], 12);
    }

    #[test]
    fn test_spi_touchscreen_adc() {
        let mut touch = spi::Tsc2046::new();
        // Initially untouched
        assert!(!touch.is_touching());
        touch.transfer(0x90); // Y measurement command
        let _ = touch.transfer(0x00);
        let _ = touch.transfer(0x00);

        // Touch at center (128, 96)
        touch.set_touch(Some((128, 96)));
        assert!(touch.is_touching());

        // Measure X: channel 0b101 (control byte 0b11010000 = 0xD0)
        touch.transfer(0xD0);
        let b1 = touch.transfer(0x00);
        let b2 = touch.transfer(0x00);
        let adc_x = (((b1 as u16) & 0x7F) << 5) | (((b2 as u16) >> 3) & 0x1F);
        // ((128 * (3800 - 300)) / 255) + 300 ≈ 2058
        assert!((adc_x as i32 - 2058).abs() < 10);
    }

    #[test]
    fn test_spi_nvram_firmware() {
        let mut nvram = spi::FirmwareNvram::new();
        // Nickname at user settings + 06h (GBATEK), copy at 0x3FE00
        nvram.transfer(0x03); // READ
        nvram.transfer(0x03); // Addr MSB
        nvram.transfer(0xFE); // Addr Mid
        nvram.transfer(0x06); // Addr LSB

        let mut name_bytes = [0u8; 14];
        for b in name_bytes.iter_mut() {
            *b = nvram.transfer(0x00);
        }
        let chars: Vec<u16> = name_bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let name = String::from_utf16(&chars).unwrap();
        assert_eq!(name, "CrabBoy");
    }

    #[test]
    fn firmware_user_settings_have_valid_crcs() {
        let nvram = spi::FirmwareNvram::new();
        let m = &nvram.memory;
        let us_off = u16::from_le_bytes([m[0x20], m[0x21]]) as usize * 8;
        assert_eq!(us_off, m.len() - 0x200);
        for copy in [us_off, us_off + 0x100] {
            let us = &m[copy..copy + 0x100];
            let crc = u16::from_le_bytes([us[0x72], us[0x73]]);
            assert_eq!(crc, spi::crc16(0xFFFF, &us[..0x70]));
        }
        let len = u16::from_le_bytes([m[0x2C], m[0x2D]]) as usize;
        assert_eq!(u16::from_le_bytes([m[0x2A], m[0x2B]]), spi::crc16(0, &m[0x2C..0x2C + len]));
        // Newer copy (counter 1) is the one reported.
        assert_eq!(nvram.user_settings()[0x70], 1);
    }

    #[test]
    fn test_keypad_and_touch_bits() {
        let mut nds = Nds::new();
        assert_eq!(nds.bus.keyinput, 0x03FF); // all 10 keys released
        assert_eq!(nds.bus.extkeyin & (1 << 6), 1 << 6); // pen up

        nds.set_nds_key(NdsKey::A, true);
        assert_eq!(nds.bus.keyinput & 1, 0); // A pressed

        nds.set_nds_key(NdsKey::X, true);
        assert_eq!(nds.bus.extkeyin & 1, 0); // X pressed

        nds.set_touch(Some((100, 100)));
        assert_eq!(nds.bus.extkeyin & (1 << 6), 0); // Pen down

        nds.set_touch(None);
        assert_ne!(nds.bus.extkeyin & (1 << 6), 0); // Pen up
    }

    #[test]
    fn test_ppu_scanline_rendering_and_swap() {
        let mut ppu = ppu::NdsPpu::new();
        // Set backdrop color in Engine A palette to red (0x001F)
        ppu.engine_a.palette[0] = 0x1F;
        ppu.engine_a.palette[1] = 0x00;

        // Set backdrop color in Engine B palette to blue (0x7C00)
        ppu.engine_b.palette[0] = 0x00;
        ppu.engine_b.palette[1] = 0x7C;

        // Display mode 1 (graphics) on both engines.
        ppu.engine_a.dispcnt = 1 << 16;
        ppu.engine_b.dispcnt = 1 << 16;
        ppu.powcnt1 = 1 << 15;

        let dummy_vram = [0u8; 16];
        // Render scanline 0 (POWCNT1 bit 15 set: Engine A top, Engine B bottom)
        ppu.render_scanline(0, [&dummy_vram; 4], &crate::nds::bus::VramViews::default());

        // Top screen pixel (0, 0) should be Red (0xFF0000FF)
        assert_eq!(ppu.framebuffer[0] & 0x00FFFFFF, 0x000000FF);
        // Bottom screen pixel (0, 192) should be Blue (0xFFFF0000)
        assert_eq!(ppu.framebuffer[192 * SCREEN_WIDTH] & 0x00FFFFFF, 0x00FF0000);

        // Clear POWCNT1 bit 15: Engine A goes to the lower screen
        ppu.powcnt1 &= !(1 << 15);
        ppu.render_scanline(0, [&dummy_vram; 4], &crate::nds::bus::VramViews::default());

        // Now Top screen pixel (0, 0) should be Blue
        assert_eq!(ppu.framebuffer[0] & 0x00FFFFFF, 0x00FF0000);
        // Bottom screen pixel (0, 192) should be Red
        assert_eq!(ppu.framebuffer[192 * SCREEN_WIDTH] & 0x00FFFFFF, 0x000000FF);
    }

    #[test]
    fn test_nds_arm9_execution() {
        let mut nds = Nds::new();
        // Place instructions in Main RAM at 0x0200_0000:
        // 1. MOV R1, #42        (0xE3A0102A)
        // 2. ADD R2, R1, #10    (0xE281200A)
        // 3. STR R2, [R0]       (0xE5802000) -> stores 52 at [R0]
        let ram_base = 0x0200_0000;
        let target_addr = ram_base + 0x100;
        nds.arm9.regs[0] = target_addr;

        nds.bus.write_arm9_u32(ram_base, 0xE3A0102A);
        nds.bus.write_arm9_u32(ram_base + 4, 0xE281200A);
        nds.bus.write_arm9_u32(ram_base + 8, 0xE5802000);

        nds.arm9.regs[15] = ram_base;

        // Step 1: MOV R1, #42
        step_arm9(&mut nds.arm9, &mut nds.bus);
        assert_eq!(nds.arm9.regs[1], 42);

        // Step 2: ADD R2, R1, #10
        step_arm9(&mut nds.arm9, &mut nds.bus);
        assert_eq!(nds.arm9.regs[2], 52);

        // Step 3: STR R2, [R0]
        step_arm9(&mut nds.arm9, &mut nds.bus);
        assert_eq!(nds.bus.read_arm9_u32(target_addr), 52);
    }

    #[test]
    fn test_nds_arm7_execution() {
        let mut nds = Nds::new();
        // Place instructions in ARM7 dedicated WRAM at 0x0380_0000:
        // 1. MOV R0, #100      (0xE3A00064)
        // 2. SUB R1, R0, #25   (0xE2401019)
        let wram_base = 0x0380_0000;
        nds.bus.write_arm7_u32(wram_base, 0xE3A00064);
        nds.bus.write_arm7_u32(wram_base + 4, 0xE2401019);

        nds.arm7.regs[15] = wram_base;

        step_arm7(&mut nds.arm7, &mut nds.bus);
        assert_eq!(nds.arm7.regs[0], 100);

        step_arm7(&mut nds.arm7, &mut nds.bus);
        assert_eq!(nds.arm7.regs[1], 75);
    }

    #[test]
    fn test_nds_timer_cascade_and_irq() {
        let mut bus = NdsBus::new();
        // Timer 0: prescaler 0 (1 tick per cycle), reload 0xFFFE, enable, IRQ enable
        bus.timers_arm9[0].reload = 0xFFFE;
        bus.timers_arm9[0].counter = 0xFFFE;
        bus.timers_arm9[0].write_control(0x00C0); // bits 6, 7 set

        // Timer 1: count-up (cascade), reload 0x0000, enable
        bus.timers_arm9[1].reload = 0x0000;
        bus.timers_arm9[1].counter = 0x0000;
        bus.timers_arm9[1].write_control(0x0084); // bits 2, 7 set

        // Step 2 sys cycles -> Timer 0 should overflow
        bus.step_timers(2, true);

        // Timer 0 should have reloaded to 0xFFFE
        assert_eq!(bus.timers_arm9[0].counter, 0xFFFE);
        // Timer 1 should have cascaded and incremented to 1
        assert_eq!(bus.timers_arm9[1].counter, 1);
        // Timer 0 IRQ bit (bit 3) should be set in if_arm9
        assert_ne!(bus.if_arm9 & (1 << 3), 0);
    }

    #[test]
    fn test_nds_dma_immediate_transfer() {
        let mut bus = NdsBus::new();
        // Fill 4 words in source at 0x0200_1000
        for i in 0..4 {
            bus.write_arm9_u32(0x0200_1000 + (i * 4), 0x1111_1111 * (i + 1));
        }

        // Configure DMA0: 4 words, 32-bit, immediate start, IRQ enable
        // Bit 31: enable, Bit 30: IRQ, Bit 26: 32-bit, count = 4
        bus.dma_arm9[0].sad = 0x0200_1000;
        bus.dma_arm9[0].dad = 0x0200_2000;
        bus.write_arm9_u32(0x0400_00B8, 0xC400_0004);

        // Destination should contain transferred words
        for i in 0..4 {
            assert_eq!(bus.read_arm9_u32(0x0200_2000 + (i * 4)), 0x1111_1111 * (i + 1));
        }
        // DMA 0 IRQ bit (bit 8) should be set
        assert_ne!(bus.if_arm9 & (1 << 8), 0);
    }

    #[test]
    fn test_nds_full_frame_execution() {
        let mut nds = Nds::new();
        // Set ARM9 and ARM7 to execute an infinite loop: B . (0xEAFFFFFE)
        let ram_base = 0x0200_0000;
        nds.bus.write_arm9_u32(ram_base, 0xEAFFFFFE);
        nds.arm9.regs[15] = ram_base;

        let wram_base = 0x0380_0000;
        nds.bus.write_arm7_u32(wram_base, 0xEAFFFFFE);
        nds.arm7.regs[15] = wram_base;

        // Enable VBlank IRQ on Engine A DISPSTAT
        nds.bus.ppu.dispstat_a |= 1 << 3;
        nds.bus.ie_arm9 |= 1 << 0;
        nds.bus.ime_arm9 = true;

        assert_eq!(nds.frame_counter, 0);
        nds.run_frame();

        assert_eq!(nds.frame_counter, 1);
        // VBlank IRQ bit (bit 0) should have fired during the frame
        assert_ne!(nds.bus.if_arm9 & (1 << 0), 0);
    }

    #[test]
    fn test_nds_3d_matrix_stack_and_transformations() {
        let mut bus = NdsBus::new();

        // 1. Set Matrix Mode to 2 (Position & Vector Simultaneous) via port 0x0400_0440
        bus.write_arm9_u32(0x0400_0440, 2);
        assert_eq!(bus.read_arm9_u32(0x0400_0600) & (3 << 25), 2 << 25);

        // 2. Load Identity Matrix (Cmd 0x15 via port 0x0400_0454)
        bus.write_arm9_u32(0x0400_0454, 0);

        // 3. Push to Stack (Cmd 0x11 via port 0x0400_0444)
        bus.write_arm9_u32(0x0400_0444, 0);
        // pos_sp should now be 1 (bits 28..31 of GXSTAT)
        assert_eq!((bus.read_arm9_u32(0x0400_0600) >> 28) & 0x1F, 1);

        // 4. Scale position: Scale(2.0, 3.0, 4.0) (Cmd 0x1B via port 0x0400_046C)
        // 2.0 = 8192, 3.0 = 12288, 4.0 = 16384 in 12-bit fixed point
        bus.write_arm9_u32(0x0400_046C, 8192);
        bus.write_arm9_u32(0x0400_046C, 12288);
        bus.write_arm9_u32(0x0400_046C, 16384);

        // Verify Position matrix has scale applied, but Vector matrix remains Identity!
        assert_eq!(bus.ppu.engine_3d.geom.pos_mtx.m[0], 8192);
        assert_eq!(bus.ppu.engine_3d.geom.pos_mtx.m[5], 12288);
        assert_eq!(bus.ppu.engine_3d.geom.pos_mtx.m[10], 16384);
        assert_eq!(bus.ppu.engine_3d.geom.vec_mtx.m[0], 4096);
        assert_eq!(bus.ppu.engine_3d.geom.vec_mtx.m[5], 4096);

        // 5. Pop from Stack (Cmd 0x12 via port 0x0400_0448)
        bus.write_arm9_u32(0x0400_0448, 1);
        assert_eq!((bus.read_arm9_u32(0x0400_0600) >> 28) & 0x1F, 0);
        // Position matrix should be restored to Identity
        assert_eq!(bus.ppu.engine_3d.geom.pos_mtx.m[0], 4096);
    }

    #[test]
    fn test_nds_3d_geometry_commands_and_rasterizer() {
        let mut bus = NdsBus::new();

        // 1. Set Viewport: (0, 0) to (255, 191) via port 0x0400_0580
        // Viewport: X0=0, Y0=0, X1=255, Y1=191 -> 0xBF_FF_00_00
        bus.write_arm9_u32(0x0400_0580, 0xBFFF0000);

        // 2. Set Matrix Mode to 0 (Projection) and set Identity
        bus.write_arm9_u32(0x0400_0440, 0);
        bus.write_arm9_u32(0x0400_0454, 0);

        // 3. Set Matrix Mode to 1 (Position) and set Identity
        bus.write_arm9_u32(0x0400_0440, 1);
        bus.write_arm9_u32(0x0400_0454, 0);

        // 4. Set Polygon Attributes (Cmd 0x29 via port 0x0400_04A4)
        // Culling: None (bits 6..7 = 0)
        bus.write_arm9_u32(0x0400_04A4, 0x0000_0000);

        // 5. Begin Triangles (Cmd 0x40 via port 0x0400_0500)
        bus.write_arm9_u32(0x0400_0500, 0); // 0 = Triangles

        // Vertex 0: Top Center (0.0, 0.5, 0.0), Color: Red (RGB555 0x001F)
        bus.write_arm9_u32(0x0400_0480, 0x001F);
        // VTX_16: X=0, Y=2048 (0.5), Z=0
        bus.write_arm9_u32(0x0400_048C, (2048 << 16) | 0);
        bus.write_arm9_u32(0x0400_048C, 0);

        // Vertex 1: Bottom Left (-0.5, -0.5, 0.0), Color: Green (RGB555 0x03E0)
        bus.write_arm9_u32(0x0400_0480, 0x03E0);
        // VTX_16: X=-2048 as u16, Y=-2048 as u16, Z=0
        let neg_half = (-2048i16) as u16 as u32;
        bus.write_arm9_u32(0x0400_048C, (neg_half << 16) | neg_half);
        bus.write_arm9_u32(0x0400_048C, 0);

        // Vertex 2: Bottom Right (0.5, -0.5, 0.0), Color: Blue (RGB555 0x7C00)
        bus.write_arm9_u32(0x0400_0480, 0x7C00);
        // VTX_16: X=2048, Y=-2048, Z=0
        bus.write_arm9_u32(0x0400_048C, (neg_half << 16) | 2048);
        bus.write_arm9_u32(0x0400_048C, 0);

        // End Primitive (Cmd 0x41 via port 0x0400_0504)
        bus.write_arm9_u32(0x0400_0504, 0);

        // Verify polygon was assembled in back buffer
        assert_eq!(bus.ppu.engine_3d.geom.polygons_back.len(), 1);

        // 6. Request Swap Buffers (Cmd 0x50 via port 0x0400_0540)
        bus.write_arm9_u32(0x0400_0540, 0);
        assert!(bus.ppu.engine_3d.geom.swap_buffers_requested);

        // 7. Trigger VBlank processing
        bus.ppu.engine_3d.on_vblank(&bus.vram_a[..], &bus.ppu.engine_a.palette);

        // Buffers should have swapped: back buffer empty, front buffer has the polygon
        assert_eq!(bus.ppu.engine_3d.geom.polygons_back.len(), 0);
        assert_eq!(bus.ppu.engine_3d.geom.polygons_front.len(), 1);

        // The rasterizer should have drawn pixels into color_buffer!
        // Check center of the screen (approx x: 128, y: 96)
        let center_pixel = bus.ppu.engine_3d.rasterizer.color_buffer[96 * 256 + 128];
        // Center pixel should be non-black (non-cleared) and opaque (alpha 0xFF)
        assert_eq!(center_pixel & 0xFF00_0000, 0xFF00_0000);
        assert_ne!(center_pixel & 0x00FF_FFFF, 0); // has color
    }
}
