//! Game Boy cartridge: header parsing, MBC1/2/3/5 bank controllers and
//! battery-backed SRAM persistence.
//!
//! The header byte at 0x0147 selects the mapper. Everything else about a
//! cartridge (ROM size, RAM size, CGB support, title) is derived from the
//! header too, so a wrong parse here surfaces as "game boots to white" long
//! before any CPU bug does.

use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MbcKind {
    None,
    Mbc1,
    Mbc2,
    Mbc3,
    Mbc5,
}

/// CGB support flag from header byte 0x0143.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CgbFlag {
    /// DMG-only cartridge (0x00 or any non-CGB value).
    None,
    /// 0x80: enhanced for CGB but still runs on DMG.
    Enhanced,
    /// 0xC0: CGB required.
    Only,
}

pub struct GbCartridge {
    pub rom: Vec<u8>,
    pub ram: Vec<u8>,
    pub mbc: MbcKind,
    pub title: String,
    pub cgb_flag: CgbFlag,
    pub has_battery: bool,
    pub has_rtc: bool,
    pub rom_banks: usize,
    pub ram_banks: usize,

    // --- bank controller state ---
    pub rom_bank: usize,
    pub ram_bank: usize,
    pub ram_enabled: bool,
    /// MBC1 only: 0 = ROM banking (upper bits extend ROM bank),
    /// 1 = RAM banking (upper bits select RAM bank / ROM bank 0 region).
    pub mbc1_mode: u8,
    mbc1_bank_hi: usize,

    // --- MBC3 RTC ---
    pub rtc_regs: [u8; 5],
    rtc_latch_prev: u8,
    pub rtc_latched: [u8; 5],
    rtc_base_secs: u64,

    pub save_path: Option<PathBuf>,
    save_dirty: bool,
}

impl GbCartridge {
    pub fn from_file<P: AsRef<Path>>(path: P) -> std::io::Result<Self> {
        let rom = fs::read(path.as_ref())?;
        let mut cart = Self::from_bytes(rom);
        let save_path = path.as_ref().with_extension("sav");
        if cart.has_battery {
            if let Ok(data) = fs::read(&save_path) {
                let n = data.len().min(cart.ram.len());
                cart.ram[..n].copy_from_slice(&data[..n]);
                log::info!("Loaded GB save RAM from {}", save_path.display());
            }
            cart.save_path = Some(save_path);
        }
        Ok(cart)
    }

    pub fn from_bytes(rom: Vec<u8>) -> Self {
        let cart_type = *rom.get(0x0147).unwrap_or(&0);
        let (mbc, has_battery, has_rtc) = match cart_type {
            0x00 => (MbcKind::None, false, false),
            0x01..=0x03 => (MbcKind::Mbc1, cart_type == 0x03, false),
            0x05 | 0x06 => (MbcKind::Mbc2, cart_type == 0x06, false),
            0x0F | 0x10 => (MbcKind::Mbc3, true, true),
            0x11..=0x13 => (MbcKind::Mbc3, cart_type == 0x13, false),
            0x19..=0x1E => (MbcKind::Mbc5, matches!(cart_type, 0x1B | 0x1E), false),
            other => {
                log::warn!("Unknown GB cartridge type {:#04X}, assuming MBC1", other);
                (MbcKind::Mbc1, true, false)
            }
        };

        // ROM size: header 0x0148 gives 32 KiB << n. Trust the file length
        // when the header disagrees (homebrew/test ROMs often lie).
        let header_banks = 2usize << (*rom.get(0x0148).unwrap_or(&0)).min(8);
        let actual_banks = (rom.len() / 0x4000).max(2);
        let rom_banks = header_banks.min(actual_banks).max(2);

        let ram_banks = match *rom.get(0x0149).unwrap_or(&0) {
            0x00 => 0,
            0x01 | 0x02 => 1,
            0x03 => 4,
            0x04 => 16,
            0x05 => 8,
            _ => 1,
        };
        // MBC2 has 512 x 4 bits of on-chip RAM regardless of the header.
        let ram_size = if mbc == MbcKind::Mbc2 {
            512
        } else {
            ram_banks * 0x2000
        };

        let title: String = rom
            .get(0x0134..0x0143)
            .map(|s| {
                s.iter()
                    .take_while(|&&b| b != 0)
                    .filter(|&&b| b.is_ascii_graphic() || b == b' ')
                    .map(|&b| b as char)
                    .collect()
            })
            .unwrap_or_default();

        let cgb_flag = match *rom.get(0x0143).unwrap_or(&0) {
            0xC0 => CgbFlag::Only,
            0x80 => CgbFlag::Enhanced,
            _ => CgbFlag::None,
        };

        log::info!(
            "GB cartridge: '{}' type={:#04X} ({:?}) rom_banks={} ram={}B cgb={:?}",
            title,
            cart_type,
            mbc,
            rom_banks,
            ram_size,
            cgb_flag
        );

        Self {
            rom,
            ram: vec![0; ram_size],
            mbc,
            title,
            cgb_flag,
            has_battery,
            has_rtc,
            rom_banks,
            ram_banks,
            rom_bank: 1,
            ram_bank: 0,
            ram_enabled: false,
            mbc1_mode: 0,
            mbc1_bank_hi: 0,
            rtc_regs: [0; 5],
            rtc_latch_prev: 0xFF,
            rtc_latched: [0; 5],
            rtc_base_secs: 0,
            save_path: None,
            save_dirty: false,
        }
    }

    /// Effective ROM bank for the 0x4000-0x7FFF window.
    fn effective_rom_bank(&self) -> usize {
        let bank = match self.mbc {
            MbcKind::None => 1,
            MbcKind::Mbc1 => {
                let lo = if self.rom_bank == 0 { 1 } else { self.rom_bank };
                if self.mbc1_mode == 0 {
                    (self.mbc1_bank_hi << 5) | lo
                } else {
                    lo
                }
            }
            MbcKind::Mbc2 => self.rom_bank.max(1),
            MbcKind::Mbc3 => self.rom_bank.max(1),
            MbcKind::Mbc5 => self.rom_bank,
        };
        bank % self.rom_banks.max(1)
    }

    /// ROM bank mapped at 0x0000-0x3FFF. Always 0 except in MBC1 advanced
    /// (mode 1) banking on >= 1 MiB carts, where the high bits apply here too.
    fn base_rom_bank(&self) -> usize {
        if self.mbc == MbcKind::Mbc1 && self.mbc1_mode == 1 {
            ((self.mbc1_bank_hi << 5) % self.rom_banks.max(1)) & !0
        } else {
            0
        }
    }

    pub fn read_rom(&self, addr: u16) -> u8 {
        let offset = if addr < 0x4000 {
            self.base_rom_bank() * 0x4000 + addr as usize
        } else {
            self.effective_rom_bank() * 0x4000 + (addr as usize - 0x4000)
        };
        *self.rom.get(offset).unwrap_or(&0xFF)
    }

    pub fn read_ram(&self, addr: u16) -> u8 {
        if !self.ram_enabled {
            return 0xFF;
        }
        // MBC3 maps RTC registers over the RAM window for bank >= 0x08.
        if self.mbc == MbcKind::Mbc3 && self.ram_bank >= 0x08 {
            let idx = self.ram_bank - 0x08;
            return *self.rtc_latched.get(idx).unwrap_or(&0xFF);
        }
        if self.mbc == MbcKind::Mbc2 {
            // 512 nibbles, mirrored through the whole A000-BFFF window.
            let idx = (addr as usize - 0xA000) & 0x1FF;
            return self.ram.get(idx).map_or(0xFF, |v| v | 0xF0);
        }
        let offset = self.ram_bank * 0x2000 + (addr as usize - 0xA000);
        *self.ram.get(offset).unwrap_or(&0xFF)
    }

    pub fn write_ram(&mut self, addr: u16, val: u8) {
        if !self.ram_enabled {
            return;
        }
        if self.mbc == MbcKind::Mbc3 && self.ram_bank >= 0x08 {
            let idx = self.ram_bank - 0x08;
            if let Some(r) = self.rtc_regs.get_mut(idx) {
                *r = val;
                self.save_dirty = true;
            }
            return;
        }
        if self.mbc == MbcKind::Mbc2 {
            let idx = (addr as usize - 0xA000) & 0x1FF;
            if let Some(b) = self.ram.get_mut(idx) {
                *b = val & 0x0F;
                self.save_dirty = true;
            }
            return;
        }
        let offset = self.ram_bank * 0x2000 + (addr as usize - 0xA000);
        if let Some(b) = self.ram.get_mut(offset) {
            *b = val;
            self.save_dirty = true;
        }
    }

    /// Writes into 0x0000-0x7FFF are mapper control, not memory.
    pub fn write_control(&mut self, addr: u16, val: u8) {
        match self.mbc {
            MbcKind::None => {}
            MbcKind::Mbc1 => match addr {
                0x0000..=0x1FFF => self.ram_enabled = (val & 0x0F) == 0x0A,
                0x2000..=0x3FFF => {
                    let lo = (val & 0x1F) as usize;
                    self.rom_bank = if lo == 0 { 1 } else { lo };
                }
                0x4000..=0x5FFF => {
                    self.mbc1_bank_hi = (val & 0x03) as usize;
                    if self.mbc1_mode == 1 {
                        self.ram_bank = self.mbc1_bank_hi % self.ram_banks.max(1);
                    }
                }
                _ => {
                    self.mbc1_mode = val & 0x01;
                    self.ram_bank = if self.mbc1_mode == 1 {
                        self.mbc1_bank_hi % self.ram_banks.max(1)
                    } else {
                        0
                    };
                }
            },
            MbcKind::Mbc2 => {
                // Bit 8 of the address selects between RAM enable and bank.
                if addr < 0x4000 {
                    if (addr & 0x0100) == 0 {
                        self.ram_enabled = (val & 0x0F) == 0x0A;
                    } else {
                        let b = (val & 0x0F) as usize;
                        self.rom_bank = if b == 0 { 1 } else { b };
                    }
                }
            }
            MbcKind::Mbc3 => match addr {
                0x0000..=0x1FFF => self.ram_enabled = (val & 0x0F) == 0x0A,
                0x2000..=0x3FFF => {
                    let b = (val & 0x7F) as usize;
                    self.rom_bank = if b == 0 { 1 } else { b };
                }
                0x4000..=0x5FFF => self.ram_bank = val as usize & 0x0F,
                _ => {
                    // 0 -> 1 transition latches the RTC into the shadow regs.
                    if self.rtc_latch_prev == 0 && val == 1 {
                        self.latch_rtc();
                    }
                    self.rtc_latch_prev = val;
                }
            },
            MbcKind::Mbc5 => match addr {
                0x0000..=0x1FFF => self.ram_enabled = (val & 0x0F) == 0x0A,
                0x2000..=0x2FFF => self.rom_bank = (self.rom_bank & 0x100) | val as usize,
                0x3000..=0x3FFF => {
                    self.rom_bank = (self.rom_bank & 0xFF) | ((val as usize & 1) << 8)
                }
                0x4000..=0x5FFF => self.ram_bank = (val & 0x0F) as usize,
                _ => {}
            },
        }
    }

    fn latch_rtc(&mut self) {
        if !self.has_rtc {
            self.rtc_latched = self.rtc_regs;
            return;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if self.rtc_base_secs == 0 {
            self.rtc_base_secs = now;
        }
        let elapsed = now.saturating_sub(self.rtc_base_secs);
        let days = elapsed / 86400;
        self.rtc_regs[0] = (elapsed % 60) as u8;
        self.rtc_regs[1] = ((elapsed / 60) % 60) as u8;
        self.rtc_regs[2] = ((elapsed / 3600) % 24) as u8;
        self.rtc_regs[3] = (days & 0xFF) as u8;
        self.rtc_regs[4] = ((days >> 8) & 1) as u8;
        self.rtc_latched = self.rtc_regs;
    }

    /// Flush battery-backed RAM to disk. Called once a second by the frame
    /// loop; a no-op when nothing changed, so it is cheap to call often.
    pub fn sync_to_disk(&mut self) {
        if !self.save_dirty || !self.has_battery {
            return;
        }
        if let Some(ref path) = self.save_path {
            if let Err(e) = crate::fs_util::write_atomic(path, &self.ram) {
                log::warn!("Failed to write GB save {}: {}", path.display(), e);
                return;
            }
            self.save_dirty = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rom_with(cart_type: u8, rom_size_code: u8, banks: usize) -> Vec<u8> {
        let mut rom = vec![0u8; banks * 0x4000];
        rom[0x0147] = cart_type;
        rom[0x0148] = rom_size_code;
        rom[0x0149] = 0x03; // 32 KiB RAM
        // Stamp each bank with its index so bank switching is observable.
        for b in 0..banks {
            rom[b * 0x4000] = b as u8;
        }
        rom
    }

    #[test]
    fn mbc1_bank_zero_reads_as_bank_one() {
        let mut c = GbCartridge::from_bytes(rom_with(0x03, 0x04, 32));
        c.write_control(0x2000, 0x00);
        assert_eq!(c.read_rom(0x4000), 1, "bank 0 selects bank 1 on MBC1");
    }

    #[test]
    fn mbc1_high_bits_extend_the_rom_bank() {
        let mut c = GbCartridge::from_bytes(rom_with(0x03, 0x05, 64));
        c.write_control(0x2000, 0x05); // low bits = 5
        c.write_control(0x4000, 0x01); // high bits = 1 -> bank 0x25
        assert_eq!(c.read_rom(0x4000), 0x25);
    }

    #[test]
    fn mbc5_supports_bank_256() {
        let mut c = GbCartridge::from_bytes(rom_with(0x1B, 0x08, 512));
        c.write_control(0x2000, 0x00);
        c.write_control(0x3000, 0x01); // bit 8 set -> bank 0x100
        assert_eq!(c.read_rom(0x4000), 0x00, "bank 256 stamps low byte 0");
        c.write_control(0x2000, 0x05);
        assert_eq!(c.read_rom(0x4000), 0x05);
    }

    #[test]
    fn ram_is_locked_until_enabled() {
        let mut c = GbCartridge::from_bytes(rom_with(0x03, 0x04, 32));
        c.write_ram(0xA000, 0x42);
        assert_eq!(c.read_ram(0xA000), 0xFF, "disabled RAM reads open bus");
        c.write_control(0x0000, 0x0A);
        c.write_ram(0xA000, 0x42);
        assert_eq!(c.read_ram(0xA000), 0x42);
    }

    #[test]
    fn mbc2_ram_is_four_bit_and_mirrored() {
        let mut rom = vec![0u8; 0x8000];
        rom[0x0147] = 0x06; // MBC2+BATTERY
        let mut c = GbCartridge::from_bytes(rom);
        c.write_control(0x0000, 0x0A);
        c.write_ram(0xA000, 0xFF);
        assert_eq!(c.read_ram(0xA000), 0xFF, "upper nibble reads as 1s");
        assert_eq!(c.ram[0], 0x0F, "only the low nibble is stored");
        assert_eq!(c.read_ram(0xA200), 0xFF, "512-byte mirror");
    }

    #[test]
    fn cgb_flag_is_parsed_from_header() {
        let mut rom = vec![0u8; 0x8000];
        rom[0x0143] = 0xC0;
        assert_eq!(GbCartridge::from_bytes(rom).cgb_flag, CgbFlag::Only);
    }
}
