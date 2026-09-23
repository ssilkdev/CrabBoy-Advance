//! Save Synchronization Dialog (ROADMAP M4).
//!
//! Provides a UI for configuring cross-platform save sync between desktop and Android,
//! picking the sync folder (Syncthing, Dropbox, Google Drive), triggering manual syncs,
//! and viewing sync reports and backup history.

use crate::gba::save_sync::{SaveSync, SyncReport, SyncStatus};
use crate::ui::config::SaveSyncConfig;
use egui::{Color32, RichText, ScrollArea, Window};
use std::path::PathBuf;

pub struct SaveSyncDialog {
    pub is_open: bool,
    pub last_report: Option<SyncReport>,
    pub last_sync_time: Option<String>,
}

impl Default for SaveSyncDialog {
    fn default() -> Self {
        Self::new()
    }
}

impl SaveSyncDialog {
    pub fn new() -> Self {
        Self {
            is_open: false,
            last_report: None,
            last_sync_time: None,
        }
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        config: &mut SaveSyncConfig,
        config_dirty: &mut bool,
        local_dirs: &[PathBuf],
        current_rom: &str,
        toast: &mut Option<String>,
    ) {
        if !self.is_open {
            return;
        }

        let mut open = self.is_open;
        Window::new("🔄 Cloud & Cross-Platform Save Sync")
            .open(&mut open)
            .default_width(520.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.heading("Save Sync (Desktop & Android)");
                ui.label(
                    RichText::new(
                        "Synchronize battery saves (.sav) and save states (.state) seamlessly \
                         across devices via any shared folder (Syncthing, Google Drive, Dropbox, Nextcloud).",
                    )
                    .weak()
                    .small(),
                );
                ui.label(
                    RichText::new("⭐ Newest save wins automatically. Older copies are backed up so nothing is lost.")
                        .color(Color32::LIGHT_BLUE)
                        .small(),
                );
                ui.separator();

                // Folder Selection
                ui.label(RichText::new("Sync Folder:").strong());
                ui.horizontal(|ui| {
                    let path_display = config
                        .sync_dir
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "No folder selected".to_string());
                    ui.monospace(path_display);

                    if ui.button("Browse...").clicked() {
                        if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                            config.sync_dir = Some(folder);
                            config.enabled = true;
                            *config_dirty = true;
                            *toast = Some("Sync folder updated".to_string());
                        }
                    }

                    if config.sync_dir.is_some() && ui.button("Clear").clicked() {
                        config.sync_dir = None;
                        config.enabled = false;
                        *config_dirty = true;
                        *toast = Some("Sync folder cleared".to_string());
                    }
                });

                ui.separator();

                // Options
                ui.horizontal(|ui| {
                    if ui
                        .checkbox(&mut config.enabled, "Enable automatic sync on game load & save")
                        .changed()
                    {
                        *config_dirty = true;
                    }
                });

                ui.horizontal(|ui| {
                    ui.label("Backups to retain per save file:");
                    if ui
                        .add(egui::DragValue::new(&mut config.max_backups).range(1..=10))
                        .changed()
                    {
                        *config_dirty = true;
                    }
                });

                ui.separator();

                // Sync Actions
                ui.horizontal(|ui| {
                    let can_sync = config.sync_dir.is_some();
                    if ui.add_enabled(can_sync, egui::Button::new("⚡ Sync All Saves Now")).clicked() {
                        if let Some(ref sync_dir) = config.sync_dir {
                            let syncer = SaveSync::new(sync_dir, local_dirs.to_vec())
                                .with_max_backups(config.max_backups);
                            let report = syncer.sync_all();
                            *toast = Some(report.summary());
                            self.last_sync_time = Some(chrono_now_str());
                            self.last_report = Some(report);
                        }
                    }

                    let has_game = current_rom != "No ROM Loaded";
                    let game_btn_label = if has_game {
                        format!("Sync '{current_rom}'")
                    } else {
                        "Sync Current Game".to_string()
                    };
                    if ui
                        .add_enabled(can_sync && has_game, egui::Button::new(game_btn_label))
                        .clicked()
                    {
                        if let Some(ref sync_dir) = config.sync_dir {
                            let syncer = SaveSync::new(sync_dir, local_dirs.to_vec())
                                .with_max_backups(config.max_backups);
                            let report = syncer.sync_game(current_rom);
                            *toast = Some(report.summary());
                            self.last_sync_time = Some(chrono_now_str());
                            self.last_report = Some(report);
                        }
                    }
                });

                if let Some(ref time) = self.last_sync_time {
                    ui.label(RichText::new(format!("Last synchronized: {time}")).weak().small());
                }

                // Sync Results Report
                if let Some(ref report) = self.last_report {
                    ui.separator();
                    ui.label(RichText::new("Last Sync Report:").strong());
                    ui.label(report.summary());

                    ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                        for action in &report.actions {
                            ui.horizontal(|ui| {
                                match &action.status {
                                    SyncStatus::UpToDate => {
                                        ui.label(RichText::new("✔ In Sync:").color(Color32::GREEN).small());
                                    }
                                    SyncStatus::CopiedToSync => {
                                        ui.label(RichText::new("⬆ Uploaded to sync:").color(Color32::LIGHT_BLUE).small());
                                    }
                                    SyncStatus::CopiedFromSync => {
                                        ui.label(RichText::new("⬇ Downloaded from sync:").color(Color32::LIGHT_BLUE).small());
                                    }
                                    SyncStatus::UpdatedSyncFromLocal { backed_up } => {
                                        ui.label(RichText::new("⬆ Updated sync (local newer):").color(Color32::YELLOW).small());
                                        ui.label(RichText::new(format!("backup: {}", backed_up.display())).weak().small());
                                    }
                                    SyncStatus::UpdatedLocalFromSync { backed_up } => {
                                        ui.label(RichText::new("⬇ Updated local (sync newer):").color(Color32::YELLOW).small());
                                        ui.label(RichText::new(format!("backup: {}", backed_up.display())).weak().small());
                                    }
                                    SyncStatus::ConflictResolved { backed_up, .. } => {
                                        ui.label(RichText::new("⚠ Conflict resolved:").color(Color32::GOLD).small());
                                        ui.label(RichText::new(format!("backup: {}", backed_up.display())).weak().small());
                                    }
                                }
                                ui.label(&action.file_name);
                            });
                        }

                        for (path, err) in &report.errors {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("❌ Error:").color(Color32::RED).small());
                                ui.label(format!("{}: {err}", path.display()));
                            });
                        }
                    });
                }
            });

        self.is_open = open;
    }
}

fn chrono_now_str() -> String {
    use std::time::SystemTime;
    let now = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let hours = (now / 3600) % 24;
    let minutes = (now / 60) % 60;
    let seconds = now % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02} UTC")
}
