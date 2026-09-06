//! Persistent Multi-Slot Save State Manager with Disk Serialization

use crate::gba::Gba;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub struct SlotMetadata {
    pub slot: usize,
    pub exists: bool,
    pub size_bytes: u64,
    pub modified_str: String,
}

pub struct SaveStateManager {
    pub active_slot: usize,
    memory_cache: [Option<Vec<u8>>; 10],
    saves_dir: PathBuf,
}

impl Default for SaveStateManager {
    fn default() -> Self {
        Self::new("saves")
    }
}

impl SaveStateManager {
    pub fn new<P: AsRef<Path>>(saves_dir: P) -> Self {
        let path = saves_dir.as_ref().to_path_buf();
        let _ = fs::create_dir_all(&path);
        Self {
            active_slot: 0,
            memory_cache: Default::default(),
            saves_dir: path,
        }
    }

    fn sanitize_rom_name(rom_name: &str) -> String {
        rom_name
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
            .collect()
    }

    pub fn slot_file_path(&self, rom_name: &str, slot: usize) -> PathBuf {
        let safe_name = Self::sanitize_rom_name(rom_name);
        self.saves_dir.join(format!("{}_slot{}.state", safe_name, slot))
    }

    pub fn save_slot(&mut self, slot: usize, gba: &Gba, rom_name: &str) -> Result<(), String> {
        let slot_idx = slot.min(9);
        let data = gba.save_state();

        let _ = fs::create_dir_all(&self.saves_dir);
        let path = self.slot_file_path(rom_name, slot_idx);

        if let Err(e) = fs::write(&path, &data) {
            return Err(format!("Failed to write save state to disk: {}", e));
        }

        self.memory_cache[slot_idx] = Some(data);
        Ok(())
    }

    pub fn load_slot(&mut self, slot: usize, gba: &mut Gba, rom_name: &str) -> Result<(), String> {
        let slot_idx = slot.min(9);

        // Check memory cache first
        if let Some(ref data) = self.memory_cache[slot_idx] {
            if gba.load_state(data) {
                return Ok(());
            }
        }

        // Fall back to reading from disk
        let path = self.slot_file_path(rom_name, slot_idx);
        if !path.exists() {
            return Err(format!("Save Slot {} is empty", slot_idx));
        }

        let data = fs::read(&path).map_err(|e| format!("Failed to read save state: {}", e))?;
        if gba.load_state(&data) {
            self.memory_cache[slot_idx] = Some(data);
            Ok(())
        } else {
            Err(format!("Corrupted or incompatible save state in Slot {}", slot_idx))
        }
    }

    pub fn get_slot_metadata(&self, slot: usize, rom_name: &str) -> SlotMetadata {
        let slot_idx = slot.min(9);
        let path = self.slot_file_path(rom_name, slot_idx);

        if let Ok(meta) = fs::metadata(&path) {
            let size = meta.len();
            let mod_time = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let duration = SystemTime::now().duration_since(mod_time).unwrap_or_default();
            let secs_ago = duration.as_secs();

            let time_desc = if secs_ago < 60 {
                "Just now".to_string()
            } else if secs_ago < 3600 {
                format!("{} min ago", secs_ago / 60)
            } else if secs_ago < 86400 {
                format!("{} hrs ago", secs_ago / 3600)
            } else {
                format!("{} days ago", secs_ago / 86400)
            };

            SlotMetadata {
                slot: slot_idx,
                exists: true,
                size_bytes: size,
                modified_str: time_desc,
            }
        } else if self.memory_cache[slot_idx].is_some() {
            SlotMetadata {
                slot: slot_idx,
                exists: true,
                size_bytes: self.memory_cache[slot_idx].as_ref().unwrap().len() as u64,
                modified_str: "In-Memory".to_string(),
            }
        } else {
            SlotMetadata {
                slot: slot_idx,
                exists: false,
                size_bytes: 0,
                modified_str: "[Empty]".to_string(),
            }
        }
    }

    pub fn delete_slot(&mut self, slot: usize, rom_name: &str) {
        let slot_idx = slot.min(9);
        self.memory_cache[slot_idx] = None;
        let path = self.slot_file_path(rom_name, slot_idx);
        let _ = fs::remove_file(path);
    }
}
