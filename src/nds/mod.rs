//! Nintendo DS System Coordinator
//!
//! Orchestrates the dual ARM9/ARM7 CPUs, memory bus, IPC, 2D/3D graphics,
//! audio, SPI (touch/firmware/PMIC), and Slot-1 game card controllers.

pub mod bus;
pub mod card;
pub mod cpu;
pub mod ipc;
pub mod ppu;
pub mod spi;

use bus::NdsBus;
use card::NdsCard;
use cpu::{Arm7Tdmi, Arm946eS};
use ipc::Ipc;
use std::path::Path;

pub const SCREEN_WIDTH: usize = 256;
pub const SCREEN_HEIGHT: usize = 192;
pub const DUAL_SCREEN_HEIGHT: usize = SCREEN_HEIGHT * 2; // 384

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
    pub ipc: Ipc,
    pub card: NdsCard,
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
            ipc: Ipc::default(),
            card: NdsCard::new(),
            frame_counter: 0,
            is_running: true,
        }
    }

    pub fn load_rom(&mut self, path: &Path) -> Result<(), String> {
        self.card.load_from_file(path)?;
        let h = &self.card.header;

        // Copy ARM9 executable into Main RAM
        let arm9_rom_start = h.arm9_rom_offset as usize;
        let arm9_rom_end = arm9_rom_start + (h.arm9_size as usize);
        if arm9_rom_end <= self.card.rom.len() {
            let arm9_slice = &self.card.rom[arm9_rom_start..arm9_rom_end];
            let ram_offset = (h.arm9_ram_addr.wrapping_sub(0x0200_0000) & 0x3F_FFFF) as usize;
            if ram_offset + arm9_slice.len() <= self.bus.main_ram.len() {
                self.bus.main_ram[ram_offset..ram_offset + arm9_slice.len()].copy_from_slice(arm9_slice);
                log::info!("Loaded ARM9 payload ({} bytes) to RAM offset 0x{:06X}", arm9_slice.len(), ram_offset);
            }
        }
        self.arm9.regs[15] = h.arm9_entry_addr;

        // Copy ARM7 executable into Main RAM / WRAM
        let arm7_rom_start = h.arm7_rom_offset as usize;
        let arm7_rom_end = arm7_rom_start + (h.arm7_size as usize);
        if arm7_rom_end <= self.card.rom.len() {
            let arm7_slice = &self.card.rom[arm7_rom_start..arm7_rom_end];
            let ram_offset = (h.arm7_ram_addr.wrapping_sub(0x0200_0000) & 0x3F_FFFF) as usize;
            if ram_offset + arm7_slice.len() <= self.bus.main_ram.len() {
                self.bus.main_ram[ram_offset..ram_offset + arm7_slice.len()].copy_from_slice(arm7_slice);
                log::info!("Loaded ARM7 payload ({} bytes) to RAM offset 0x{:06X}", arm7_slice.len(), ram_offset);
            }
        }
        self.arm7.regs[15] = h.arm7_entry_addr;

        // Set post-boot flags to skip splash screen directly into game
        self.bus.postflg_arm9 = 1;
        self.bus.postflg_arm7 = 1;

        Ok(())
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

    pub fn run_frame(&mut self) {
        if !self.is_running {
            return;
        }

        // Each NDS frame comprises 263 scanlines (~59.83 Hz).
        for line in 0..263 {
            self.bus.ppu.set_scanline(line as u16);
            if line < 192 {
                let vram_a = &self.bus.vram_a[..];
                let vram_b = &self.bus.vram_b[..];
                let vram_c = &self.bus.vram_c[..];
                let vram_d = &self.bus.vram_d[..];
                self.bus.ppu.render_scanline(line, vram_a, vram_b, vram_c, vram_d);
            }
        }

        self.frame_counter += 1;
    }

    pub fn get_framebuffer(&self) -> &[u32] {
        &self.bus.ppu.framebuffer
    }

    pub fn title(&self) -> &str {
        if self.card.header.title.is_empty() {
            "Nintendo DS Game"
        } else {
            &self.card.header.title
        }
    }

    pub fn game_code(&self) -> &str {
        &self.card.header.game_code
    }

    pub fn flush_save(&mut self) {
        // Save battery / flash sync to disk
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
        assert_ne!(arm9.cpsr & cpu::arm9::FLAG_Q, 0);

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
        // Read user name at 0x3FE04
        nvram.transfer(0x03); // READ
        nvram.transfer(0x03); // Addr MSB
        nvram.transfer(0xFE); // Addr Mid
        nvram.transfer(0x04); // Addr LSB

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

        let dummy_vram = [0u8; 16];
        // Render scanline 0 (normal: Engine A top, Engine B bottom)
        ppu.render_scanline(0, &dummy_vram, &dummy_vram, &dummy_vram, &dummy_vram);

        // Top screen pixel (0, 0) should be Red (0xFF0000FF)
        assert_eq!(ppu.framebuffer[0] & 0x00FFFFFF, 0x000000FF);
        // Bottom screen pixel (0, 192) should be Blue (0xFFFF0000)
        assert_eq!(ppu.framebuffer[192 * SCREEN_WIDTH] & 0x00FFFFFF, 0x00FF0000);

        // Swap screens via POWCNT1 bit 15
        ppu.powcnt1 |= 1 << 15;
        ppu.render_scanline(0, &dummy_vram, &dummy_vram, &dummy_vram, &dummy_vram);

        // Now Top screen pixel (0, 0) should be Blue
        assert_eq!(ppu.framebuffer[0] & 0x00FFFFFF, 0x00FF0000);
        // Bottom screen pixel (0, 192) should be Red
        assert_eq!(ppu.framebuffer[192 * SCREEN_WIDTH] & 0x00FFFFFF, 0x000000FF);
    }
}
