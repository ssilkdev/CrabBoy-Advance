//! GBA cartridge save-type detection and the unified save-backend wrapper.
//!
//! Real GBA games declare their save chip via one of a small set of ASCII
//! marker strings the Nintendo SDK's linker embeds directly in the ROM
//! image (this is exactly how real emulators and flash carts detect it too).
//! Without this, a fixed backend can only work for games that happen to use
//! that exact chip -- everything else silently loses save data.

use std::path::PathBuf;

use super::eeprom::Eeprom;
use super::flash::{Flash, FlashSize};
use super::sram::Sram;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveType {
    None,
    Sram,
    Flash64K,
    Flash128K,
    Eeprom,
}

/// Scans the ROM image for the Nintendo SDK save-type marker strings.
/// Falls back to plain SRAM (the historical convention used by other GBA
/// emulators) when no marker is found.
pub fn detect_save_type(rom: &[u8]) -> SaveType {
    fn contains(rom: &[u8], marker: &[u8]) -> bool {
        !marker.is_empty() && rom.len() >= marker.len() && rom.windows(marker.len()).any(|w| w == marker)
    }

    if contains(rom, b"EEPROM_V") {
        SaveType::Eeprom
    } else if contains(rom, b"FLASH1M_V") {
        SaveType::Flash128K
    } else if contains(rom, b"FLASH512_V") {
        SaveType::Flash64K
    } else if contains(rom, b"FLASH_V") {
        SaveType::Flash64K
    } else if contains(rom, b"SRAM_V") {
        SaveType::Sram
    } else {
        SaveType::Sram
    }
}

pub enum SaveBackend {
    None,
    Sram(Sram),
    Flash(Flash),
    Eeprom(Eeprom),
}

impl SaveBackend {
    pub fn new(save_type: SaveType, save_path: Option<PathBuf>) -> Self {
        match save_type {
            SaveType::None => SaveBackend::None,
            SaveType::Sram => SaveBackend::Sram(Sram::new(save_path)),
            SaveType::Flash64K => SaveBackend::Flash(Flash::new(save_path, FlashSize::Kb64)),
            SaveType::Flash128K => SaveBackend::Flash(Flash::new(save_path, FlashSize::Kb128)),
            SaveType::Eeprom => SaveBackend::Eeprom(Eeprom::new(save_path)),
        }
    }

    /// Read from the 0x0E/0x0F (SRAM/Flash) address window. EEPROM is not
    /// addressed here -- it lives in the 0x0D window and is handled by the
    /// DMA engine directly (see `Mmu::execute_dma_channel`).
    pub fn read(&self, addr: u32) -> u8 {
        match self {
            SaveBackend::None => 0xFF,
            SaveBackend::Sram(s) => s.read(addr),
            SaveBackend::Flash(f) => f.read(addr),
            SaveBackend::Eeprom(_) => 0xFF,
        }
    }

    pub fn write(&mut self, addr: u32, val: u8) {
        match self {
            SaveBackend::None => {}
            SaveBackend::Sram(s) => s.write(addr, val),
            SaveBackend::Flash(f) => f.write(addr, val),
            SaveBackend::Eeprom(_) => {}
        }
    }

    /// Whether the save chip has unsaved changes.
    pub fn is_dirty(&self) -> bool {
        match self {
            SaveBackend::None => false,
            SaveBackend::Sram(s) => s.dirty,
            SaveBackend::Flash(f) => f.dirty,
            SaveBackend::Eeprom(e) => e.dirty,
        }
    }

    pub fn set_dirty(&mut self, dirty: bool) {
        match self {
            SaveBackend::None => {}
            SaveBackend::Sram(s) => s.dirty = dirty,
            SaveBackend::Flash(f) => f.dirty = dirty,
            SaveBackend::Eeprom(e) => e.dirty = dirty,
        }
    }

    pub fn sync_to_disk(&mut self) {
        match self {
            SaveBackend::None => {}
            SaveBackend::Sram(s) => s.sync_to_disk(),
            SaveBackend::Flash(f) => f.sync_to_disk(),
            SaveBackend::Eeprom(e) => e.sync_to_disk(),
        }
    }

    pub fn as_eeprom_mut(&mut self) -> Option<&mut Eeprom> {
        match self {
            SaveBackend::Eeprom(e) => Some(e),
            _ => None,
        }
    }

    pub fn is_eeprom(&self) -> bool {
        matches!(self, SaveBackend::Eeprom(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_each_marker() {
        assert_eq!(detect_save_type(b"padding EEPROM_V120 more"), SaveType::Eeprom);
        assert_eq!(detect_save_type(b"padding FLASH1M_V102 more"), SaveType::Flash128K);
        assert_eq!(detect_save_type(b"padding FLASH512_V130 more"), SaveType::Flash64K);
        assert_eq!(detect_save_type(b"padding FLASH_V124 more"), SaveType::Flash64K);
        assert_eq!(detect_save_type(b"padding SRAM_V113 more"), SaveType::Sram);
    }

    #[test]
    fn falls_back_to_sram_when_no_marker_found() {
        assert_eq!(detect_save_type(b"no markers here at all"), SaveType::Sram);
    }

    #[test]
    fn eeprom_takes_priority_when_multiple_markers_present() {
        // Some ROM headers embed more than one string (unused SDK leftovers);
        // EEPROM correctness matters most since it's the format most likely
        // to be silently broken by misdetection, so it wins ties.
        assert_eq!(detect_save_type(b"SRAM_V113 ... EEPROM_V120"), SaveType::Eeprom);
    }
}

/// The save chip's state (ROADMAP M2). The backend kind is fixed by the ROM,
/// so a tag mismatch means the state belongs to a different game.
impl crate::gba::state::Snapshot for SaveBackend {
    fn save(&self, w: &mut crate::gba::state::StateWriter) {
        match self {
            SaveBackend::None => w.u8(0),
            SaveBackend::Sram(s) => { w.u8(1); s.save(w) }
            SaveBackend::Flash(f) => { w.u8(2); f.save(w) }
            SaveBackend::Eeprom(e) => { w.u8(3); e.save(w) }
        }
    }
    fn load(&mut self, r: &mut crate::gba::state::StateReader) -> Option<()> {
        let tag = r.u8()?;
        match (self, tag) {
            (SaveBackend::None, 0) => Some(()),
            (SaveBackend::Sram(s), 1) => s.load(r),
            (SaveBackend::Flash(f), 2) => f.load(r),
            (SaveBackend::Eeprom(e), 3) => e.load(r),
            _ => None,
        }
    }
}
