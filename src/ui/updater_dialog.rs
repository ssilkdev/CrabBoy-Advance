//! Modern Windows 11 Software Update Dialog for CrabBoy Advance
//!
//! Displays current version vs latest available GitHub release,
//! changelog and release notes, download progress, and one-click
//! update & restart actions.

use eframe::egui::{self, Color32, RichText, Vec2};

use super::updater::{UpdateManager, UpdateStatus, CURRENT_VERSION, REPO_NAME, REPO_OWNER};

pub struct UpdaterDialog {
    pub is_open: bool,
}

impl Default for UpdaterDialog {
    fn default() -> Self {
        Self::new()
    }
}

impl UpdaterDialog {
    pub fn new() -> Self {
        Self { is_open: false }
    }

    pub fn open(&mut self) {
        self.is_open = true;
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        updater: &UpdateManager,
        toast_message: &mut Option<String>,
    ) {
        if !self.is_open {
            return;
        }

        let mut is_open = self.is_open;
        let mut close_dialog = false;
        let status = {
            let lock = updater.status.lock().unwrap();
            lock.clone()
        };

        egui::Window::new("🔄 Software Update - CrabBoy Advance")
            .open(&mut is_open)
            .resizable(true)
            .default_size(Vec2::new(560.0, 480.0))
            .min_size(Vec2::new(450.0, 360.0))
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);

                // Top Header Banner
                ui.horizontal(|ui| {
                    ui.heading(RichText::new("🦀 CrabBoy Advance").strong().color(Color32::from_rgb(255, 120, 80)));
                    ui.label(RichText::new(format!("v{}", CURRENT_VERSION)).monospace().color(Color32::LIGHT_GRAY));
                });
                ui.separator();

                // Status Card
                egui::Frame::group(ui.style())
                    .fill(Color32::from_rgb(25, 27, 34))
                    .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(50, 54, 66)))
                    .inner_margin(12.0)
                    .show(ui, |ui| {
                        match &status {
                            UpdateStatus::Idle => {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("ℹ Update status not checked yet.").color(Color32::LIGHT_GRAY));
                                });
                            }
                            UpdateStatus::Checking => {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label(RichText::new("Checking GitHub for the latest release...").color(Color32::from_rgb(100, 180, 255)));
                                });
                            }
                            UpdateStatus::UpToDate { version, .. } => {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("✅ You're up to date!").strong().color(Color32::from_rgb(80, 220, 120)));
                                });
                                ui.label(RichText::new(format!(
                                    "CrabBoy Advance v{} is currently the newest published version on GitHub.",
                                    version
                                )).weak());
                            }
                            UpdateStatus::UpdateAvailable { current, latest } => {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("🎉 New Update Available!").strong().color(Color32::from_rgb(255, 200, 60)));
                                    ui.label(RichText::new(format!("v{} ➔ v{}", current, latest.version)).strong().color(Color32::WHITE));
                                });
                                ui.add_space(2.0);
                                ui.label(RichText::new(&latest.title).strong().color(Color32::from_rgb(220, 220, 240)));

                                if latest.exe_size > 0 {
                                    let mb = latest.exe_size as f64 / (1024.0 * 1024.0);
                                    ui.label(RichText::new(format!("Download size: {:.1} MB", mb)).weak().small());
                                }
                            }
                            UpdateStatus::Downloading { progress, downloaded_bytes, total_bytes } => {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("📥 Downloading Update...").strong().color(Color32::from_rgb(100, 180, 255)));
                                });
                                ui.add_space(4.0);
                                ui.add(egui::ProgressBar::new(*progress).show_percentage().animate(true));

                                let dl_mb = *downloaded_bytes as f64 / (1024.0 * 1024.0);
                                let tot_mb = *total_bytes as f64 / (1024.0 * 1024.0);
                                if *total_bytes > 0 {
                                    ui.label(RichText::new(format!("{:.1} MB of {:.1} MB", dl_mb, tot_mb)).weak().small());
                                } else {
                                    ui.label(RichText::new(format!("{:.1} MB downloaded", dl_mb)).weak().small());
                                }
                            }
                            UpdateStatus::DownloadedReadyToRestart { latest, .. } => {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("✨ Update Ready to Install!").strong().color(Color32::from_rgb(80, 220, 120)));
                                });
                                ui.label(RichText::new(format!(
                                    "CrabBoy Advance v{} has been downloaded. Restart to complete the update.",
                                    latest.version
                                )).color(Color32::LIGHT_GRAY));
                            }
                            UpdateStatus::Failed(err) => {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("⚠️ Unable to check/download updates").strong().color(Color32::from_rgb(255, 100, 100)));
                                });
                                ui.label(RichText::new(err).color(Color32::from_rgb(220, 150, 150)).small());
                            }
                        }
                    });

                ui.add_space(4.0);

                // Release Notes / Changelog section
                let release_body = match &status {
                    UpdateStatus::UpdateAvailable { latest, .. } => Some((&latest.tag_name, &latest.body)),
                    UpdateStatus::DownloadedReadyToRestart { latest, .. } => Some((&latest.tag_name, &latest.body)),
                    _ => None,
                };

                if let Some((tag, body)) = release_body {
                    ui.label(RichText::new(format!("Release Notes ({}):", tag)).strong());
                    egui::ScrollArea::vertical()
                        .max_height(220.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            egui::Frame::group(ui.style())
                                .fill(Color32::from_rgb(18, 20, 24))
                                .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(40, 44, 52)))
                                .inner_margin(8.0)
                                .show(ui, |ui| {
                                    ui.add(
                                        egui::TextEdit::multiline(&mut body.as_str())
                                            .font(egui::TextStyle::Monospace)
                                            .desired_width(f32::INFINITY)
                                            .interactive(false),
                                    );
                                });
                        });
                } else {
                    // Quick info about GitHub repo
                    ui.add_space(4.0);
                    ui.label(RichText::new("GitHub Repository:").strong());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("https://github.com/{}/{}", REPO_OWNER, REPO_NAME)).weak());
                    });
                }

                ui.add_space(8.0);
                ui.separator();

                // Actions Ribbon
                ui.horizontal(|ui| {
                    match &status {
                        UpdateStatus::UpdateAvailable { latest, .. } => {
                            let btn = egui::Button::new(
                                RichText::new(format!("📥 Download & Update to v{}", latest.version))
                                    .strong()
                                    .color(Color32::WHITE),
                            )
                            .fill(Color32::from_rgb(35, 120, 70));

                            if ui.add(btn).clicked() {
                                updater.download_and_install_async();
                                *toast_message = Some(format!("📥 Downloading CrabBoy Advance v{}...", latest.version));
                            }

                            if ui.button("🌐 View on GitHub").clicked() {
                                open_in_browser(&latest.html_url);
                            }
                        }
                        UpdateStatus::DownloadedReadyToRestart { latest, .. } => {
                            let btn = egui::Button::new(
                                RichText::new("🔄 Restart & Apply Update Now")
                                    .strong()
                                    .color(Color32::WHITE),
                            )
                            .fill(Color32::from_rgb(20, 140, 60));

                            if ui.add(btn).clicked() {
                                if let Err(e) = updater.restart_and_apply() {
                                    *toast_message = Some(format!("Update failed: {}", e));
                                }
                            }

                            if ui.button("🌐 View on GitHub").clicked() {
                                open_in_browser(&latest.html_url);
                            }
                        }
                        UpdateStatus::Downloading { .. } => {
                            ui.add_enabled(false, egui::Button::new("⏳ Downloading..."));
                        }
                        _ => {
                            let check_btn = egui::Button::new("🔍 Check for Updates");
                            if ui.add(check_btn).clicked() {
                                updater.check_for_updates_async();
                                *toast_message = Some("🔍 Checking GitHub for updates...".to_string());
                            }

                            if ui.button("🌐 Visit GitHub Releases").clicked() {
                                let url = format!("https://github.com/{}/{}/releases", REPO_OWNER, REPO_NAME);
                                open_in_browser(&url);
                            }
                        }
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            close_dialog = true;
                        }
                    });
                });
            });

        if close_dialog {
            is_open = false;
        }
        self.is_open = is_open;
    }
}

fn open_in_browser(url: &str) {
    #[cfg(target_os = "windows")]
    {
        let mut cmd = std::process::Command::new("cmd");
        cmd.args(["/C", "start", "", url]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }
        let _ = cmd.spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}
