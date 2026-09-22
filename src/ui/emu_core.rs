//! Shared core abstraction so UI subsystems (rewind, save-state manager)
//! work against either the GBA core or the Game Boy / Game Boy Color core
//! without duplicating their logic.

use crate::dmg::GameBoy;
use crate::gba::Gba;

/// The minimum a core must provide to participate in save states and rewind.
pub trait SnapshotCore {
    fn save_state(&self) -> Vec<u8>;
    fn load_state(&mut self, data: &[u8]) -> bool;
}

impl SnapshotCore for Gba {
    fn save_state(&self) -> Vec<u8> {
        Gba::save_state(self)
    }
    fn load_state(&mut self, data: &[u8]) -> bool {
        Gba::load_state(self, data)
    }
}

impl SnapshotCore for GameBoy {
    fn save_state(&self) -> Vec<u8> {
        GameBoy::save_state(self)
    }
    fn load_state(&mut self, data: &[u8]) -> bool {
        GameBoy::load_state(self, data)
    }
}

/// Which console the currently-loaded ROM runs on. Chosen from the file
/// extension at load time, which is the only reliable signal: a `.gb`/`.gbc`
/// header is laid out completely differently from a `.gba` one, so sniffing
/// content would mean guessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleKind {
    Gba,
    GameBoy,
}

impl ConsoleKind {
    pub fn from_extension(path: &std::path::Path) -> Self {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref()
        {
            Some("gb") | Some("gbc") | Some("sgb") | Some("cgb") => ConsoleKind::GameBoy,
            _ => ConsoleKind::Gba,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            ConsoleKind::Gba => "GBA",
            ConsoleKind::GameBoy => "GB/GBC",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn extension_selects_the_console() {
        assert_eq!(ConsoleKind::from_extension(Path::new("x.gb")), ConsoleKind::GameBoy);
        assert_eq!(ConsoleKind::from_extension(Path::new("x.GBC")), ConsoleKind::GameBoy);
        assert_eq!(ConsoleKind::from_extension(Path::new("x.gba")), ConsoleKind::Gba);
        assert_eq!(ConsoleKind::from_extension(Path::new("x.bin")), ConsoleKind::Gba);
    }
}
