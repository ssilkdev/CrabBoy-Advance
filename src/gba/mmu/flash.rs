//! GBA 128KB Flash Memory Emulation (Macronix MX29L010 / Sanyo)
//! Used by compatible 128KB Flash cartridges for game saves.

use std::fs;
use std::path::PathBuf;

const FLASH_SIZE: usize = 128 * 1024; // 128 KB
const SECTOR_SIZE: usize = 4096;      // 4 KB per sector (32 sectors)

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
    pub data: Box<[u8; FLASH_SIZE]>,
    current_bank: usize,
    state: FlashState,
    command: FlashCommand,
    pub dirty: bool,
    save_path: Option<PathBuf>,
}

impl Flash {
    pub fn new(save_path: Option<PathBuf>) -> Self {
        let mut data = Box::new([0xFFu8; FLASH_SIZE]);

        if let Some(ref path) = save_path {
            if path.exists() {
                if let Ok(file_data) = fs::read(path) {
                    let len = file_data.len().min(FLASH_SIZE);
                    data[..len].copy_from_slice(&file_data[..len]);
                    log::info!("Loaded {} bytes save file from {:?}", len, path);
                }
            }
        }

        Self {
            data,
            current_bank: 0,
            state: FlashState::Raw,
            command: FlashCommand::None,
            dirty: false,
            save_path,
        }
    }

    pub fn read(&self, addr: u32) -> u8 {
        let offset = (addr & 0xFFFF) as usize;

        if self.command == FlashCommand::Id {
            if offset == 0 {
                return 0xC2; // Macronix Manufacturer ID
            } else if offset == 1 {
                return 0x09; // MX29L010 Device ID (128 KB)
            }
        }

        let full_addr = (self.current_bank * 0x10000) + offset;
        self.data[full_addr % FLASH_SIZE]
    }

    pub fn write(&mut self, addr: u32, val: u8) {
        let offset = (addr & 0xFFFF) as usize;

        match self.state {
            FlashState::Raw => {
                match self.command {
                    FlashCommand::Program => {
                        let full_addr = (self.current_bank * 0x10000) + offset;
                        if full_addr < FLASH_SIZE {
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
                } else if self.command == FlashCommand::Erase {
                    if val == 0x30 {
                        // Erase 4KB sector
                        let sector_base = (self.current_bank * 0x10000) + (offset & !(SECTOR_SIZE - 1));
                        if sector_base + SECTOR_SIZE <= FLASH_SIZE {
                            self.data[sector_base..sector_base + SECTOR_SIZE].fill(0xFF);
                            self.dirty = true;
                        }
                        self.command = FlashCommand::None;
                    }
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
