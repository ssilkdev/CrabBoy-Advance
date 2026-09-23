//! GBA Flash Memory Emulation (64KB Panasonic / 128KB Macronix)
//! Used by compatible Flash cartridges for game saves.

use std::fs;
use std::path::PathBuf;

const SECTOR_SIZE: usize = 4096; // 4 KB per sector

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashSize {
    Kb64,
    Kb128,
}

impl FlashSize {
    fn bytes(self) -> usize {
        match self {
            FlashSize::Kb64 => 64 * 1024,
            FlashSize::Kb128 => 128 * 1024,
        }
    }

    /// (manufacturer_id, device_id) as returned in Flash ID-read mode.
    fn chip_id(self) -> (u8, u8) {
        match self {
            // Panasonic MN63F805MNP: common 64KB Flash chip.
            FlashSize::Kb64 => (0x32, 0x1B),
            // Macronix MX29L010: common 128KB Flash chip.
            FlashSize::Kb128 => (0xC2, 0x09),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlashState {
    Raw,
    Start,
    Continue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlashCommand {
    None,
    Erase,
    Id,
    Program,
    SwitchBank,
}

pub struct Flash {
    pub data: Vec<u8>,
    size: FlashSize,
    current_bank: usize,
    state: FlashState,
    command: FlashCommand,
    pub dirty: bool,
    save_path: Option<PathBuf>,
}

impl Flash {
    pub fn new(save_path: Option<PathBuf>, size: FlashSize) -> Self {
        let flash_size = size.bytes();
        let mut data = vec![0xFFu8; flash_size];

        if let Some(ref path) = save_path {
            if path.exists() {
                if let Ok(file_data) = fs::read(path) {
                    let len = file_data.len().min(flash_size);
                    data[..len].copy_from_slice(&file_data[..len]);
                    log::info!("Loaded {} bytes save file from {:?}", len, path);
                }
            }
        }

        Self {
            data,
            size,
            current_bank: 0,
            state: FlashState::Raw,
            command: FlashCommand::None,
            dirty: false,
            save_path,
        }
    }

    pub fn save_path(&self) -> Option<&std::path::Path> {
        self.save_path.as_deref()
    }

    pub fn read(&self, addr: u32) -> u8 {
        let offset = (addr & 0xFFFF) as usize;

        if self.command == FlashCommand::Id {
            let (manufacturer, device) = self.size.chip_id();
            if offset == 0 {
                return manufacturer;
            } else if offset == 1 {
                return device;
            }
        }

        let flash_size = self.size.bytes();
        let full_addr = (self.current_bank * 0x10000) + offset;
        self.data[full_addr % flash_size]
    }

    pub fn write(&mut self, addr: u32, val: u8) {
        let offset = (addr & 0xFFFF) as usize;
        let flash_size = self.size.bytes();

        match self.state {
            FlashState::Raw => {
                match self.command {
                    FlashCommand::Program => {
                        let full_addr = (self.current_bank * 0x10000) + offset;
                        if full_addr < flash_size {
                            self.data[full_addr] = val;
                            self.dirty = true;
                        }
                        self.command = FlashCommand::None;
                    }
                    FlashCommand::SwitchBank => {
                        if offset == 0 && val < 2 {
                            self.current_bank = val as usize;
                        }
                        self.command = FlashCommand::None;
                    }
                    _ => {
                        if offset == 0x5555 && val == 0xAA {
                            self.state = FlashState::Start;
                        } else if val == 0xF0 {
                            self.command = FlashCommand::None;
                        }
                    }
                }
            }
            FlashState::Start => {
                if offset == 0x2AAA && val == 0x55 {
                    self.state = FlashState::Continue;
                } else {
                    self.state = FlashState::Raw;
                }
            }
            FlashState::Continue => {
                self.state = FlashState::Raw;
                if offset == 0x5555 {
                    match self.command {
                        FlashCommand::None => {
                            match val {
                                0x80 => self.command = FlashCommand::Erase,
                                0x90 => self.command = FlashCommand::Id,
                                0xA0 => self.command = FlashCommand::Program,
                                0xB0 => self.command = FlashCommand::SwitchBank,
                                0xF0 => self.command = FlashCommand::None,
                                _ => {}
                            }
                        }
                        FlashCommand::Erase => {
                            if val == 0x10 {
                                // Erase entire chip
                                self.data.fill(0xFF);
                                self.dirty = true;
                            }
                            self.command = FlashCommand::None;
                        }
                        FlashCommand::Id => {
                            if val == 0xF0 {
                                self.command = FlashCommand::None;
                            }
                        }
                        _ => {
                            self.command = FlashCommand::None;
                        }
                    }
                } else if self.command == FlashCommand::Erase
                    && val == 0x30 {
                        // Erase 4KB sector
                        let sector_base = (self.current_bank * 0x10000) + (offset & !(SECTOR_SIZE - 1));
                        if sector_base + SECTOR_SIZE <= flash_size {
                            self.data[sector_base..sector_base + SECTOR_SIZE].fill(0xFF);
                            self.dirty = true;
                        }
                        self.command = FlashCommand::None;
                    }
            }
        }
    }

    pub fn sync_to_disk(&mut self) {
        if !self.dirty {
            return;
        }
        if let Some(ref path) = self.save_path {
            if let Ok(()) = fs::write(path, &self.data[..]) {
                self.dirty = false;
                log::info!("Flushed save data to {:?}", path);
            }
        }
    }
}

/// Flash chip state and contents (ROADMAP M2). Contents are part of the
/// state so a replay that saves mid-way restores exactly; loading marks the
/// chip dirty so the .sav on disk follows the restored contents.
impl crate::gba::state::Snapshot for Flash {
    fn save(&self, w: &mut crate::gba::state::StateWriter) {
        w.bytes(&self.data);
        w.u32(self.current_bank as u32);
        w.u8(match self.state { FlashState::Raw => 0, FlashState::Start => 1, FlashState::Continue => 2 });
        w.u8(match self.command {
            FlashCommand::None => 0, FlashCommand::Erase => 1, FlashCommand::Id => 2,
            FlashCommand::Program => 3, FlashCommand::SwitchBank => 4,
        });
    }
    fn load(&mut self, r: &mut crate::gba::state::StateReader) -> Option<()> {
        r.bytes_into(&mut self.data)?;
        self.current_bank = (r.u32()? & 1) as usize;
        self.state = match r.u8()? { 0 => FlashState::Raw, 1 => FlashState::Start, 2 => FlashState::Continue, _ => return None };
        self.command = match r.u8()? {
            0 => FlashCommand::None, 1 => FlashCommand::Erase, 2 => FlashCommand::Id,
            3 => FlashCommand::Program, 4 => FlashCommand::SwitchBank, _ => return None,
        };
        self.dirty = true;
        Some(())
    }
}
