//! TAS Engine & Input Macro Dialog

use super::tas::{TasEngine, TasMode};
use crate::gba::Gba;
use egui::{Color32, RichText, Window};
use std::path::Path;

#[derive(Default)]
pub struct TasDialog {
    pub is_open: bool,
}

impl TasDialog {
    pub fn new() -> Self {
        Self { is_open: false }
    }

    pub fn show(&mut self, ctx: &egui::Context, tas: &mut TasEngine, gba: &mut Gba, toast: &mut Option<String>) {
        if !self.is_open {
            return;
        }

        let mut open = self.is_open;
        Window::new("⏱ TAS Engine & Frame Stepping")
            .open(&mut open)
            .default_width(450.0)
            .resizable(false)
            .show(ctx, |ui| {
                ui.heading("Tool-Assisted Speedrun & Macro Controller");
                ui.label(RichText::new("Record, edit, and replay deterministic frame-by-frame inputs.").weak().small());
                ui.separator();

                // Status readout
                let (status_text, color) = match tas.mode {
                    TasMode::Recording => (format!("🔴 RECORDING ({} frames)", tas.recorded_inputs.len()), Color32::from_rgb(255, 80, 80)),
                    TasMode::Playback => (format!("🟢 PLAYING ({}/{} frames)", tas.playback_frame, tas.recorded_inputs.len()), Color32::GREEN),
                    TasMode::Idle => ("⚪ Engine Idle".to_string(), Color32::GRAY),
                };
                ui.label(RichText::new(status_text).color(color).strong().size(15.0));

                ui.separator();
                ui.horizontal(|ui| {
                    if tas.mode != TasMode::Recording {
                        if ui.button("🔴 Start Recording").clicked() {
                            tas.start_recording();
                            *toast = Some("Started TAS Input Recording".to_string());
                        }
                    } else if ui.button("⏹ Stop Recording").clicked() {
                        tas.stop();
                        *toast = Some(format!("Recorded {} frames", tas.recorded_inputs.len()));
                    }

                    if tas.mode != TasMode::Playback {
                        if ui.button("▶ Replay Inputs").clicked() {
                            tas.start_playback();
                            *toast = Some("Started TAS Playback".to_string());
                        }
                    } else if ui.button("⏹ Stop Playback").clicked() {
                        tas.stop();
                    }

                    if ui.button("🗑 Clear").clicked() {
                        tas.recorded_inputs.clear();
                        tas.stop();
                        *toast = Some("Cleared TAS buffer".to_string());
                    }
                });

                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("💾 Export to saves/macro.tas").clicked() {
                        let path = Path::new("saves/macro.tas");
                        if tas.save_to_file(path).is_ok() {
                            *toast = Some("Saved TAS macro to saves/macro.tas".to_string());
                        }
                    }
                    if ui.button("📂 Load from saves/macro.tas").clicked() {
                        let path = Path::new("saves/macro.tas");
                        if tas.load_from_file(path).is_ok() {
                            *toast = Some(format!("Loaded {} TAS frames", tas.recorded_inputs.len()));
                        }
                    }
                });

                ui.separator();
                ui.heading("Precision Frame Stepping");
                ui.horizontal(|ui| {
                    if ui.button("⏭ Next Frame (N / .)").clicked() {
                        gba.run_frame();
                    }
                    ui.label(format!("Emulated Frame Counter: {}", gba.frame_counter));
                });

                ui.label(RichText::new(format!("CPU Cycles: {} | PC: 0x{:08X}", gba.cpu.cycles, gba.cpu.regs[15])).weak().small().monospace());
            });
        self.is_open = open;
    }
}
