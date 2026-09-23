//! Persistent user configuration (controller mappings, pad preferences).
//!
//! Lives in the platform's standard per-user config directory so a fresh
//! install picks up the previous machine's remaps, and so an uninstall of the
//! binary does not silently discard them:
//!
//! - Linux/BSD: `$XDG_CONFIG_HOME/crabboy-advance/config.json`
//!              (falls back to `~/.config/crabboy-advance/config.json`)
//! - Windows:   `%APPDATA%\CrabBoy Advance\config.json`
//! - macOS:     `~/Library/Application Support/CrabBoy Advance/config.json`
//!
//! Resolution is hand-rolled rather than pulling in the `dirs` crate: it is
//! three `env::var` lookups and keeps the dependency graph honest.
//!
//! Writes are atomic (temp file + rename) because the emulator saves config on
//! exit and on every remap; a power cut mid-write must not leave the user with
//! a truncated JSON file that resets all their bindings.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::controls::{ControllerSettings, KeyBindings};

/// Bumped when the on-disk shape changes incompatibly. Unknown/newer versions
/// are not parsed with a best-effort guess -- we fall back to defaults and keep
/// the old file untouched so downgrading does not destroy a newer config.
pub const CONFIG_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct AppConfig {
    pub version: u32,
    /// Gamepad bindings, per-controller profiles and pad selection.
    pub controllers: ControllerSettings,
    /// Keyboard bindings, stored by egui `Key` name.
    pub keyboard: KeyBindings,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            controllers: ControllerSettings::default(),
            keyboard: KeyBindings::default(),
        }
    }
}

/// Returns the directory config lives in, creating it if needed.
pub fn config_dir() -> Option<PathBuf> {
    let dir = platform_config_dir()?;
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!("Could not create config dir {}: {}", dir.display(), e);
        return None;
    }
    Some(dir)
}

#[cfg(target_os = "windows")]
fn platform_config_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .map(|p| p.join("CrabBoy Advance"))
}

#[cfg(target_os = "macos")]
fn platform_config_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|p| p.join("Library/Application Support/CrabBoy Advance"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_config_dir() -> Option<PathBuf> {
    // XDG says an empty or relative XDG_CONFIG_HOME must be ignored, not
    // treated as a path relative to the cwd.
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        let p = PathBuf::from(xdg);
        if p.is_absolute() {
            return Some(p.join("crabboy-advance"));
        }
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|p| p.join(".config/crabboy-advance"))
}

pub fn config_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("config.json"))
}

impl AppConfig {
    /// Loads config, returning defaults when absent or unreadable.
    ///
    /// A corrupt file is renamed to `config.json.bak` rather than overwritten,
    /// so a user who hand-edited it and made a typo can recover their work
    /// instead of finding it silently replaced with defaults.
    pub fn load() -> Self {
        let Some(path) = config_path() else {
            return Self::default();
        };
        if !path.exists() {
            return Self::default();
        }

        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                log::warn!("Could not read {}: {}", path.display(), e);
                return Self::default();
            }
        };

        match serde_json::from_str::<AppConfig>(&text) {
            Ok(cfg) if cfg.version <= CONFIG_VERSION => {
                log::info!("Loaded config from {}", path.display());
                cfg
            }
            Ok(cfg) => {
                log::warn!(
                    "Config at {} is version {} (this build understands {}); using defaults \
                     and leaving the file alone.",
                    path.display(),
                    cfg.version,
                    CONFIG_VERSION
                );
                Self::default()
            }
            Err(e) => {
                log::warn!("Config at {} is invalid ({}); backing it up.", path.display(), e);
                let backup = path.with_extension("json.bak");
                let _ = std::fs::rename(&path, &backup);
                Self::default()
            }
        }
    }

    pub fn save(&self) -> Result<PathBuf, String> {
        let path = config_path().ok_or_else(|| {
            "Could not determine a config directory (no HOME/APPDATA set)".to_string()
        })?;
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;
        atomic_write(&path, json.as_bytes())?;
        Ok(path)
    }
}

/// Writes via a sibling temp file + rename so readers never observe a partial
/// file. The temp file is a sibling (not in /tmp) because rename across
/// filesystems fails with EXDEV.
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)
        .map_err(|e| format!("Failed to write {}: {}", tmp.display(), e))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("Failed to replace {}: {}", path.display(), e)
    })
}
