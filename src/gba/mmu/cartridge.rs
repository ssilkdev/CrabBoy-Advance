//! GBA Cartridge (Game Pak) Handling

use std::fs;
use std::path::Path;
use super::rtc::Rtc;
use super::save_backend::{detect_save_type, SaveBackend, SaveType};
use super::sensors::{CartridgeSensors, SensorType};

pub struct Cartridge {
    pub rom: Vec<u8>,
    pub title: String,
    pub game_code: String,
    pub save: SaveBackend,
    pub save_type: SaveType,
    pub rtc: Rtc,
    pub has_rtc: bool,
    pub sensors: CartridgeSensors,
}

impl Cartridge {
    pub fn from_file<P: AsRef<Path>>(path: P) -> std::io::Result<Self> {
        let path = path.as_ref();
        let rom = fs::read(path)?;
        let save_path = path.with_extension("sav");
        let cart = Self::from_bytes_with_save(rom, Some(save_path));
        log::info!(
            "Loaded Cartridge: '{}' [{}] (ROM Size: {} bytes, RTC: {})",
            cart.title,
            cart.game_code,
            cart.rom.len(),
            cart.has_rtc
        );
        Ok(cart)
    }

    pub fn from_bytes(rom: Vec<u8>) -> Self {
        Self::from_bytes_with_save(rom, None)
    }

    pub fn from_bytes_with_save(rom: Vec<u8>, save_path: Option<std::path::PathBuf>) -> Self {
        let title = if rom.len() >= 0xAC {
            String::from_utf8_lossy(&rom[0xA0..0xAC])
                .trim_matches(char::from(0))
                .trim()
                .to_string()
        } else {
            "UNKNOWN".to_string()
        };

        let game_code = if rom.len() >= 0xB0 {
            String::from_utf8_lossy(&rom[0xAC..0xB0]).to_string()
        } else {
            "????".to_string()
        };

        // Check if cartridge has RTC (via game code prefix or SIIRTC string)
        let has_rtc = rom.windows(6).any(|w| w == b"SIIRTC")
            || game_code.starts_with("BPE")
            || game_code.starts_with("AXP")
            || game_code.starts_with("AXV");

        let save_type = detect_save_type(&rom);
        let save = SaveBackend::new(save_type, save_path);
        let rtc = Rtc::new();

        let mut sensors = CartridgeSensors::new();
        sensors.detect_from_cartridge(&game_code, &title);

        Self {
            rom,
            title,
            game_code,
            save,
            save_type,
            rtc,
            has_rtc,
            sensors,
        }
    }

    /// True if `addr` (a full 32-bit address) falls in this cartridge's
    /// EEPROM window: for ROMs <= 16MB, the whole 0x0D000000-0x0DFFFFFF
    /// mirror (since the real ROM data never reaches that far); for larger
    /// ROMs, only the top 256 bytes of that window (0x0DFFFF00-0x0DFFFFFF),
    /// matching real hardware.
    pub fn is_eeprom_address(&self, addr: u32) -> bool {
        if self.save_type != SaveType::Eeprom || ((addr >> 24) & 0xFF) != 0x0D {
            return false;
        }
        if self.rom.len() <= 16 * 1024 * 1024 {
            true
        } else {
            (addr & 0x01FF_FFFF) >= 0x01FF_FF00
        }
    }

    pub fn read8(&self, addr: u32) -> u8 {
        let region = (addr >> 24) & 0xFF;
        match region {
            0x08..=0x0D => {
                // ROM waitstates
                let rom_offset = (addr & 0x01FF_FFFF) as usize;
                // Check if RTC or Sensor GPIO is addressed at 0x080000C4..0x080000C8
                if region == 0x08 && (0x00C4..=0x00C8).contains(&rom_offset) {
                    if self.has_rtc && (self.rtc.enabled || rom_offset == 0x00C8) {
                        return self.rtc.read8(addr);
                    } else if self.sensors.sensor_type != SensorType::None {
                        return self.sensors.read_gpio(addr);
                    }
                }

                if rom_offset < self.rom.len() {
                    self.rom[rom_offset]
                } else {
                    // Past the end of the ROM chip, the cartridge bus returns
                    // the halfword address it was driven with: (addr / 2) &
                    // 0xFFFF. Some games and copy protections probe this
                    // (jsmolka unsafe.gba test #2).
                    let half = (addr >> 1) & 0xFFFF;
                    (half >> ((addr & 1) * 8)) as u8
                }
            }
            0x0E | 0x0F => self.save.read(addr),
            _ => 0,
        }
    }

    pub fn write8(&mut self, addr: u32, val: u8) {
        let region = (addr >> 24) & 0xFF;
        match region {
            0x08 => {
                // Check if RTC or Sensor GPIO register write
                let rom_offset = (addr & 0x01FF_FFFF) as usize;
                if (0x00C4..=0x00C8).contains(&rom_offset) {
                    if self.has_rtc {
                        self.rtc.write8(addr, val);
                    }
                    if self.sensors.sensor_type != SensorType::None {
                        self.sensors.write_gpio(addr, val);
                    }
                }
            }
            0x0E | 0x0F => self.save.write(addr, val),
            _ => {}
        }
    }
}

/// Cartridge-side state (ROADMAP M2): save chip, RTC and sensors. The ROM
/// image is not saved; a state only loads on the same game.
impl crate::gba::state::Snapshot for Cartridge {
    fn save(&self, w: &mut crate::gba::state::StateWriter) {
        self.save.save(w);
        self.rtc.save(w);
        self.sensors.save(w);
    }
    fn load(&mut self, r: &mut crate::gba::state::StateReader) -> Option<()> {
        self.save.load(r)?;
        self.rtc.load(r)?;
        self.sensors.load(r)
    }
}
