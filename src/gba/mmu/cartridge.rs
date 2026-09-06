//! GBA Cartridge (Game Pak) Handling

use std::fs;
use std::path::Path;
use super::flash::Flash;
use super::rtc::Rtc;
use super::sensors::{CartridgeSensors, SensorType};

pub struct Cartridge {
    pub rom: Vec<u8>,
    pub title: String,
    pub game_code: String,
    pub flash: Flash,
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

        let flash = Flash::new(save_path);
        let rtc = Rtc::new();

        let mut sensors = CartridgeSensors::new();
        sensors.detect_from_cartridge(&game_code, &title);

        Self {
            rom,
            title,
            game_code,
            flash,
            rtc,
            has_rtc,
            sensors,
        }
    }

    pub fn read8(&self, addr: u32) -> u8 {
        let region = (addr >> 24) & 0xFF;
        match region {
            0x08 | 0x09 | 0x0A | 0x0B | 0x0C | 0x0D => {
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
                    0
                }
            }
            0x0E | 0x0F => {
                // Flash / SRAM
                self.flash.read(addr)
            }
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
            0x0E | 0x0F => {
                // Flash / SRAM write
                self.flash.write(addr, val);
            }
            _ => {}
        }
    }
}
