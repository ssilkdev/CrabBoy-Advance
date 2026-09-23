//! Save synchronization between desktop and Android (ROADMAP M4).
//!
//! Enables seamless cross-platform play across desktop and Android through a
//! user-selected sync folder (Syncthing, Google Drive, Dropbox, Nextcloud).
//!
//! - **Newest wins**: file modification times decide which side is ahead.
//! - **Automatic backups**: whenever an older copy is replaced, it is archived
//!   as a timestamped `.bak` file before being overwritten, so nothing is ever lost.
//! - **Portable save states**: format v3 states (M2) load bit-identically on
//!   Linux, Windows and Android.
//! - **Bidirectional**: handles battery saves (`.sav`) and save states
//!   (`.state`), linking desktop slot 0 with Android's primary state file.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Result of syncing an individual file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncStatus {
    /// Both copies exist and have identical contents; no action needed.
    UpToDate,
    /// File only existed locally; copied to the sync folder.
    CopiedToSync,
    /// File only existed in the sync folder; copied to local storage.
    CopiedFromSync,
    /// Local file was newer; older sync copy was backed up and replaced.
    UpdatedSyncFromLocal { backed_up: PathBuf },
    /// Sync folder file was newer; older local copy was backed up and replaced.
    UpdatedLocalFromSync { backed_up: PathBuf },
    /// Same timestamp but different content; conflict resolved with backup.
    ConflictResolved { kept: PathBuf, backed_up: PathBuf },
}

/// A single sync action report.
#[derive(Debug, Clone)]
pub struct FileSyncReport {
    pub file_name: String,
    pub status: SyncStatus,
}

/// Comprehensive summary of a sync operation.
#[derive(Debug, Default, Clone)]
pub struct SyncReport {
    pub actions: Vec<FileSyncReport>,
    pub errors: Vec<(PathBuf, String)>,
}

impl SyncReport {
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty() && self.errors.is_empty()
    }

    pub fn total_transferred(&self) -> usize {
        self.actions
            .iter()
            .filter(|a| !matches!(a.status, SyncStatus::UpToDate))
            .count()
    }

    pub fn total_backups(&self) -> usize {
        self.actions
            .iter()
            .filter(|a| {
                matches!(
                    a.status,
                    SyncStatus::UpdatedSyncFromLocal { .. }
                        | SyncStatus::UpdatedLocalFromSync { .. }
                        | SyncStatus::ConflictResolved { .. }
                )
            })
            .count()
    }

    pub fn summary(&self) -> String {
        let transferred = self.total_transferred();
        let backups = self.total_backups();
        if self.actions.is_empty() && self.errors.is_empty() {
            "No save files found to sync.".to_string()
        } else if self.errors.is_empty() && transferred == 0 {
            format!("All {} file(s) are already up-to-date.", self.actions.len())
        } else {
            format!(
                "Synced {} file(s): {} updated, {} backup(s) created, {} error(s).",
                self.actions.len(),
                transferred,
                backups,
                self.errors.len()
            )
        }
    }
}

/// Save synchronization coordinator.
pub struct SaveSync {
    pub sync_dir: PathBuf,
    pub local_dirs: Vec<PathBuf>,
    pub max_backups: usize,
}

impl SaveSync {
    pub fn new<P: AsRef<Path>>(sync_dir: P, local_dirs: Vec<PathBuf>) -> Self {
        Self {
            sync_dir: sync_dir.as_ref().to_path_buf(),
            local_dirs,
            max_backups: 3,
        }
    }

    pub fn with_max_backups(mut self, max: usize) -> Self {
        self.max_backups = max.max(1);
        self
    }

    /// Sync all recognized save files (.sav, .state) between local directories
    /// and the sync folder.
    pub fn sync_all(&self) -> SyncReport {
        let mut report = SyncReport::default();
        if let Err(e) = fs::create_dir_all(&self.sync_dir) {
            report.errors.push((self.sync_dir.clone(), e.to_string()));
            return report;
        }

        let mut seen_names = std::collections::HashSet::new();

        // 1. Scan all local directories
        for local_dir in &self.local_dirs {
            if !local_dir.is_dir() {
                continue;
            }
            let entries = match fs::read_dir(local_dir) {
                Ok(entries) => entries,
                Err(e) => {
                    report.errors.push((local_dir.clone(), e.to_string()));
                    continue;
                }
            };

            for entry in entries.flatten() {
                let path = entry.path();
                if !is_syncable_file(&path) {
                    continue;
                }
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if is_backup_file(name) {
                    continue;
                }

                seen_names.insert(name.to_string());
                let sync_path = self.sync_dir.join(name);
                match self.sync_file_pair(&path, &sync_path) {
                    Ok(status) => report.actions.push(FileSyncReport {
                        file_name: name.to_string(),
                        status,
                    }),
                    Err(e) => report.errors.push((path.clone(), e)),
                }

                // If this is a desktop slot0 state, link/sync with Android primary state
                self.sync_slot0_alias(&path, name, &mut report, &mut seen_names);
            }
        }

        // 2. Scan sync folder for files that might only exist on the remote side
        if let Ok(entries) = fs::read_dir(&self.sync_dir) {
            for entry in entries.flatten() {
                let sync_path = entry.path();
                if !is_syncable_file(&sync_path) {
                    continue;
                }
                let Some(name) = sync_path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if is_backup_file(name) || seen_names.contains(name) {
                    continue;
                }

                if let Some(local_dir) = target_dir_for_file(&self.local_dirs, name) {
                    let local_path = local_dir.join(name);
                    match self.sync_file_pair(&local_path, &sync_path) {
                        Ok(status) => {
                            report.actions.push(FileSyncReport {
                                file_name: name.to_string(),
                                status,
                            });
                            // If we just downloaded an Android primary state (`{stem}.state`),
                            // mirror to desktop `{stem}_slot0.state` if applicable
                            if name.ends_with(".state") && !name.contains("_slot") {
                                let stem = &name[..name.len() - ".state".len()];
                                let slot0_name = format!("{stem}_slot0.state");
                                let slot0_path = local_dir.join(&slot0_name);
                                if !slot0_path.exists() || get_mtime(&local_path) > get_mtime(&slot0_path) {
                                    let _ = copy_preserving_mtime(&local_path, &slot0_path);
                                }
                            }
                        }
                        Err(e) => report.errors.push((sync_path, e)),
                    }
                }
            }
        }

        report
    }

    /// Sync a specific game's saves (.sav and .state files).
    pub fn sync_game(&self, rom_stem: &str) -> SyncReport {
        let mut report = SyncReport::default();
        if let Err(e) = fs::create_dir_all(&self.sync_dir) {
            report.errors.push((self.sync_dir.clone(), e.to_string()));
            return report;
        }

        let clean_stem = sanitize_name(rom_stem);
        let patterns = [
            format!("{rom_stem}.sav"),
            format!("{clean_stem}.sav"),
            format!("{rom_stem}.state"),
            format!("{clean_stem}.state"),
        ];

        let mut seen = std::collections::HashSet::new();

        for local_dir in &self.local_dirs {
            for pattern in &patterns {
                let local_path = local_dir.join(pattern);
                let sync_path = self.sync_dir.join(pattern);
                if (local_path.exists() || sync_path.exists()) && seen.insert(pattern.clone()) {
                    match self.sync_file_pair(&local_path, &sync_path) {
                        Ok(status) => report.actions.push(FileSyncReport {
                            file_name: pattern.clone(),
                            status,
                        }),
                        Err(e) => report.errors.push((local_path, e)),
                    }
                }
            }

            // Also check numbered slots 0..=9
            for slot in 0..=9 {
                let slot_names = [
                    format!("{rom_stem}_slot{slot}.state"),
                    format!("{clean_stem}_slot{slot}.state"),
                ];
                for sname in slot_names {
                    let local_path = local_dir.join(&sname);
                    let sync_path = self.sync_dir.join(&sname);
                    if (local_path.exists() || sync_path.exists()) && seen.insert(sname.clone()) {
                        match self.sync_file_pair(&local_path, &sync_path) {
                            Ok(status) => report.actions.push(FileSyncReport {
                                file_name: sname.clone(),
                                status,
                            }),
                            Err(e) => report.errors.push((local_path, e)),
                        }
                    }
                }
            }
        }

        report
    }

    /// Synchronize a single file pair between local and sync folder.
    ///
    /// "Newest wins": the file with the most recent mtime overwrites the older
    /// one after backing the older one up.
    pub fn sync_file_pair(&self, local_path: &Path, sync_path: &Path) -> Result<SyncStatus, String> {
        let local_exists = local_path.is_file();
        let sync_exists = sync_path.is_file();

        match (local_exists, sync_exists) {
            (false, false) => Err("Neither local nor sync file exists".to_string()),
            (true, false) => {
                if let Some(parent) = sync_path.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                copy_preserving_mtime(local_path, sync_path)
                    .map_err(|e| format!("Failed to copy to sync: {e}"))?;
                Ok(SyncStatus::CopiedToSync)
            }
            (false, true) => {
                if let Some(parent) = local_path.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                copy_preserving_mtime(sync_path, local_path)
                    .map_err(|e| format!("Failed to copy from sync: {e}"))?;
                Ok(SyncStatus::CopiedFromSync)
            }
            (true, true) => {
                // If contents are already identical, nothing to do.
                if are_files_identical(local_path, sync_path).unwrap_or(false) {
                    return Ok(SyncStatus::UpToDate);
                }

                let local_mtime = get_mtime(local_path);
                let sync_mtime = get_mtime(sync_path);

                if sync_mtime > local_mtime {
                    // Sync is newer: backup local, then copy sync -> local
                    let backed_up = backup_file(local_path, self.max_backups)
                        .map_err(|e| format!("Failed to backup local file: {e}"))?;
                    copy_preserving_mtime(sync_path, local_path)
                        .map_err(|e| format!("Failed to update local file: {e}"))?;
                    Ok(SyncStatus::UpdatedLocalFromSync { backed_up })
                } else if local_mtime > sync_mtime {
                    // Local is newer: backup sync, then copy local -> sync
                    let backed_up = backup_file(sync_path, self.max_backups)
                        .map_err(|e| format!("Failed to backup sync file: {e}"))?;
                    copy_preserving_mtime(local_path, sync_path)
                        .map_err(|e| format!("Failed to update sync file: {e}"))?;
                    Ok(SyncStatus::UpdatedSyncFromLocal { backed_up })
                } else {
                    // Mtimes match but contents differ: conflict resolution
                    let backed_up = backup_file(sync_path, self.max_backups)
                        .map_err(|e| format!("Conflict backup failed: {e}"))?;
                    copy_preserving_mtime(local_path, sync_path)
                        .map_err(|e| format!("Conflict resolution failed: {e}"))?;
                    Ok(SyncStatus::ConflictResolved {
                        kept: local_path.to_path_buf(),
                        backed_up,
                    })
                }
            }
        }
    }

    /// Links desktop slot 0 (`{stem}_slot0.state`) with Android's primary state (`{stem}.state`).
    fn sync_slot0_alias(
        &self,
        local_path: &Path,
        file_name: &str,
        report: &mut SyncReport,
        seen_names: &mut std::collections::HashSet<String>,
    ) {
        if file_name.ends_with("_slot0.state") {
            let stem = &file_name[..file_name.len() - "_slot0.state".len()];
            let android_name = format!("{stem}.state");
            seen_names.insert(android_name.clone());
            let sync_android_path = self.sync_dir.join(&android_name);
            if let Ok(status) = self.sync_file_pair(local_path, &sync_android_path) {
                if !matches!(status, SyncStatus::UpToDate) {
                    report.actions.push(FileSyncReport {
                        file_name: android_name.clone(),
                        status: status.clone(),
                    });
                }
                // If local slot0 was updated from sync_android_path, also update desktop's slot0 in sync_dir
                if matches!(status, SyncStatus::UpdatedLocalFromSync { .. }) {
                    let sync_slot0_path = self.sync_dir.join(file_name);
                    let _ = copy_preserving_mtime(local_path, &sync_slot0_path);
                }
            }
        }
    }
}

static BACKUP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Create a timestamped backup of `path`, rotating old backups to stay within `max_backups`.
pub fn backup_file(path: &Path, max_backups: usize) -> std::io::Result<PathBuf> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros();
    let seq = BACKUP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("backup");
    let backup_name = format!("{file_name}.bak.{now:020}_{seq:06}");
    let backup_path = path.with_file_name(backup_name);

    copy_preserving_mtime(path, &backup_path)?;
    rotate_backups(path, max_backups);
    Ok(backup_path)
}

/// Enforces the backup limit by deleting the oldest backups.
fn rotate_backups(original_path: &Path, max_backups: usize) {
    let backups = list_backups_for(original_path);
    if backups.len() > max_backups {
        for old in &backups[..backups.len() - max_backups] {
            let _ = fs::remove_file(old);
        }
    }
}

/// Finds all backups belonging to `path`, sorted by creation timestamp (oldest first).
pub fn list_backups_for(path: &Path) -> Vec<PathBuf> {
    let Some(parent) = path.parent() else {
        return Vec::new();
    };
    let Some(orig_name) = path.file_name().and_then(|n| n.to_str()) else {
        return Vec::new();
    };
    let prefix = format!("{orig_name}.bak.");

    let mut backups = Vec::new();
    if let Ok(entries) = fs::read_dir(parent) {
        for entry in entries.flatten() {
            let p = entry.path();
            if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                if name.starts_with(&prefix) {
                    backups.push(p);
                }
            }
        }
    }
    backups.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    backups
}

/// Copy `src` to `dst`, preserving the modification time.
pub fn copy_preserving_mtime(src: &Path, dst: &Path) -> std::io::Result<()> {
    let meta = fs::metadata(src)?;
    fs::copy(src, dst)?;
    if let Ok(mtime) = meta.modified() {
        if let Ok(file) = fs::OpenOptions::new().write(true).open(dst) {
            let _ = file.set_modified(mtime);
        }
    }
    Ok(())
}

/// Compares two files byte-by-byte for exact equality.
pub fn are_files_identical(a: &Path, b: &Path) -> std::io::Result<bool> {
    let meta_a = fs::metadata(a)?;
    let meta_b = fs::metadata(b)?;
    if meta_a.len() != meta_b.len() {
        return Ok(false);
    }
    let mut f_a = fs::File::open(a)?;
    let mut f_b = fs::File::open(b)?;
    let mut buf_a = [0u8; 8192];
    let mut buf_b = [0u8; 8192];

    loop {
        let n_a = f_a.read(&mut buf_a)?;
        let n_b = f_b.read(&mut buf_b)?;
        if n_a != n_b {
            return Ok(false);
        }
        if n_a == 0 {
            return Ok(true);
        }
        if buf_a[..n_a] != buf_b[..n_b] {
            return Ok(false);
        }
    }
}

pub fn is_syncable_file(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    ext.eq_ignore_ascii_case("sav") || ext.eq_ignore_ascii_case("state")
}

pub fn is_backup_file(name: &str) -> bool {
    name.contains(".bak.") || name.ends_with(".bak")
}

pub fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect()
}

fn get_mtime(path: &Path) -> SystemTime {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(UNIX_EPOCH)
}

/// Determines the most suitable local directory for an incoming sync file.
pub fn target_dir_for_file<'a>(local_dirs: &'a [PathBuf], file_name: &str) -> Option<&'a PathBuf> {
    if local_dirs.is_empty() {
        return None;
    }
    if local_dirs.len() == 1 {
        return local_dirs.first();
    }

    let is_state = file_name.ends_with(".state");
    let is_sav = file_name.ends_with(".sav");
    let stem = if is_state {
        file_name.strip_suffix(".state").unwrap_or(file_name)
    } else if is_sav {
        file_name.strip_suffix(".sav").unwrap_or(file_name)
    } else {
        file_name
    };
    let clean_stem = if let Some(idx) = stem.rfind("_slot") {
        &stem[..idx]
    } else {
        stem
    };

    // 1. If syncing a .sav, check if any local directory contains a corresponding ROM file
    if is_sav {
        for dir in local_dirs {
            for ext in &["gba", "gbc", "gb", "GBA", "GBC", "GB"] {
                if dir.join(format!("{clean_stem}.{ext}")).is_file() {
                    return Some(dir);
                }
            }
        }
    }

    // 2. Check path components / directory names for semantic matches
    if is_state {
        for dir in local_dirs {
            let s = dir.to_string_lossy().to_lowercase();
            if s.contains("state") {
                return Some(dir);
            }
        }
    } else if is_sav {
        for dir in local_dirs {
            let s = dir.to_string_lossy().to_lowercase();
            if s.contains("rom") || s.contains("save") {
                return Some(dir);
            }
        }
    }

    local_dirs.first()
}

