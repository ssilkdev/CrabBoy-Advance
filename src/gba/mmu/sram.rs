//! GBA 32KB Battery-Backed SRAM Emulation
//! Used by plain-SRAM cartridges with no Flash command protocol -- reads
//! and writes pass straight through to the backing byte array.

use std::fs;
use std::path::PathBuf;

const SRAM_SIZE: usize = 32 * 1024; // 32 KB

pub struct Sram {
    pub data: Box<[u8; SRAM_SIZE]>,
    pub dirty: bool,
    save_path: Option<PathBuf>,
}

impl Sram {
    pub fn new(save_path: Option<PathBuf>) -> Self {
        let mut data = Box::new([0xFFu8; SRAM_SIZE]);

        if let Some(ref path) = save_path {
            if path.exists() {
                if let Ok(file_data) = fs::read(path) {
                    let len = file_data.len().min(SRAM_SIZE);
                    data[..len].copy_from_slice(&file_data[..len]);
                    log::info!("Loaded {} bytes SRAM save file from {:?}", len, path);
                }
            }
        }

        Self {
            data,
            dirty: false,
            save_path,
        }
    }

    pub fn save_path(&self) -> Option<&std::path::Path> {
        self.save_path.as_deref()
    }

    pub fn read(&self, addr: u32) -> u8 {
        let offset = (addr & 0x7FFF) as usize;
        self.data[offset]
    }

    pub fn write(&mut self, addr: u32, val: u8) {
        let offset = (addr & 0x7FFF) as usize;
        if self.data[offset] != val {
            self.data[offset] = val;
            self.dirty = true;
        }
    }

    pub fn sync_to_disk(&mut self) {
        if !self.dirty {
            return;
        }
        if let Some(ref path) = self.save_path {
            if let Ok(()) = crate::fs_util::write_atomic(path, &self.data[..]) {
                self.dirty = false;
                log::info!("Flushed SRAM save data to {:?}", path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_byte_writes_are_stored_directly() {
        let mut sram = Sram::new(None);
        // Unlike Flash, a plain SRAM chip has no command protocol: any byte
        // write at any offset must be stored immediately.
        sram.write(0x0E00_1234, 0x42);
        assert_eq!(sram.read(0x0E00_1234), 0x42);
        assert!(sram.dirty);
    }

    #[test]
    fn address_wraps_within_32kb() {
        let mut sram = Sram::new(None);
        sram.write(0x0E00_0000, 0xAB);
        assert_eq!(sram.read(0x0E00_8000), 0xAB); // mirrors every 32KB
    }
}

/// SRAM contents (ROADMAP M2).
impl crate::gba::state::Snapshot for Sram {
    fn save(&self, w: &mut crate::gba::state::StateWriter) {
        w.bytes(&self.data[..]);
    }
    fn load(&mut self, r: &mut crate::gba::state::StateReader) -> Option<()> {
        r.bytes_into(&mut self.data[..])?;
        self.dirty = true;
        Some(())
    }
}
