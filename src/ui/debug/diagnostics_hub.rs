//! System Diagnostics Hub & Flight Recorder UI
//!
//! Real-time telemetry, automated quality linters, and IO event trace viewer
//! accessible via F8 and the Debug Tools menu.

use crate::gba::diagnostics::{HealthGrade, OverallHealthGrade, VideoHealthGrade};
use crate::gba::Gba;
use egui::{Color32, RichText, ScrollArea, Ui};

#[derive(Default)]
pub struct DiagnosticsHubState {
    pub filter_query: String,
    pub status_message: Option<String>,
}

pub fn show_diagnostics_hub(ui: &mut Ui, gba: &mut Gba, state: &mut DiagnosticsHubState) {
    let audio_report = gba.diagnostics.audio_linter.evaluate_health();
    let video_report = gba.diagnostics.video_linter.evaluate_health();

    let overall_grade = if audio_report.grade == HealthGrade::Critical
        || video_report.grade == VideoHealthGrade::Critical
    {
        OverallHealthGrade::Critical
    } else if audio_report.grade == HealthGrade::Warning
        || video_report.grade == VideoHealthGrade::Warning
    {
        OverallHealthGrade::Warning
    } else {
        OverallHealthGrade::Pass
    };

    // Header & Overall Health Badge
    ui.horizontal(|ui| {
        ui.heading("🔬 CrabBoy Advance Diagnostics Hub");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (text, color) = match overall_grade {
                OverallHealthGrade::Pass => ("PASS", Color32::from_rgb(0, 230, 130)),
                OverallHealthGrade::Warning => ("WARNING", Color32::from_rgb(255, 200, 40)),
                OverallHealthGrade::Critical => ("CRITICAL", Color32::from_rgb(255, 70, 70)),
            };
            ui.label(RichText::new(format!(" [{}] ", text)).strong().size(16.0).color(color));
            ui.label(RichText::new("Overall System Health:").strong());
        });
    });

    ui.separator();

    // Controls and Action Buttons
    ui.horizontal(|ui| {
        if ui.button("💾 Export JSON Report").clicked() {
            let report = gba.diagnostics.generate_report(&gba.mmu, gba.frame_counter, gba.cpu.cycles);
            if let Some(path) = rfd::FileDialog::new()
                .set_file_name("crabboy_diagnostic_report.json")
                .add_filter("JSON Document", &["json"])
                .save_file()
            {
                match report.save_json(&path) {
                    Ok(_) => state.status_message = Some(format!("Report saved to {}", path.display())),
                    Err(e) => state.status_message = Some(format!("Export error: {}", e)),
                }
            }
        }

        if ui.button("🧹 Clear Flight Recorder").clicked() {
            gba.mmu.flight_recorder.clear();
            state.status_message = Some("Flight Recorder cleared.".to_string());
        }

        if ui.button("🔄 Reset Health Counters").clicked() {
            gba.diagnostics.reset();
            state.status_message = Some("Health metrics reset.".to_string());
        }

        if let Some(ref msg) = state.status_message {
            ui.label(RichText::new(msg).small().color(Color32::LIGHT_BLUE));
        }
    });

    ui.add_space(4.0);

    // Active Anomalies Section
    let mut anomalies = Vec::new();
    anomalies.extend(audio_report.anomalies.iter().cloned());
    anomalies.extend(video_report.anomalies.iter().cloned());

    egui::CollapsingHeader::new(format!("⚠️ Active Diagnostics Anomalies ({})", anomalies.len()))
        .default_open(true)
        .show(ui, |ui| {
            if anomalies.is_empty() {
                ui.label(RichText::new("✓ All subsystems operating within nominal thresholds.").color(Color32::from_rgb(0, 200, 120)));
            } else {
                for (i, a) in anomalies.iter().enumerate() {
                    let color = if a.contains("Critical") || a.contains("Screen Freeze") || a.contains("Black Screen") {
                        Color32::from_rgb(255, 90, 90)
                    } else {
                        Color32::from_rgb(255, 210, 50)
                    };
                    ui.label(RichText::new(format!("{}. {}", i + 1, a)).color(color));
                }
            }
        });

    ui.add_space(4.0);

    // Subsystem Metrics: Audio & Video side by side
    ui.columns(2, |cols| {
        // Column 1: Audio Health
        cols[0].group(|ui| {
            let audio_color = match audio_report.grade {
                HealthGrade::Pass => Color32::from_rgb(0, 220, 130),
                HealthGrade::Warning => Color32::from_rgb(255, 200, 40),
                HealthGrade::Critical => Color32::from_rgb(255, 70, 70),
            };
            ui.horizontal(|ui| {
                ui.label(RichText::new("🔊 Audio Subsystem Health").strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(format!("{:?}", audio_report.grade)).color(audio_color).strong());
                });
            });
            ui.separator();

            ui.label(format!("Peak Amplitude:   {:.3}", audio_report.peak_amplitude));
            ui.label(format!("DC Offset Bias:   {:.4}", audio_report.dc_bias));
            ui.label(format!("Clipping Rate:    {:.2}% ({} samples)", audio_report.clipping_rate * 100.0, audio_report.clipping_samples));
            ui.label(format!("Silence Ratio:    {:.1}%", audio_report.silence_ratio * 100.0));

            ui.add_space(2.0);
            ui.label(RichText::new("Channel Triggers:").small().weak());
            ui.label(format!(
                "DS-A: {} | DS-B: {} | SQ1: {} | SQ2: {} | WAV: {} | NOI: {}",
                audio_report.channel_triggers[0],
                audio_report.channel_triggers[1],
                audio_report.channel_triggers[2],
                audio_report.channel_triggers[3],
                audio_report.channel_triggers[4],
                audio_report.channel_triggers[5],
            ));
        });

        // Column 2: Video Health
        cols[1].group(|ui| {
            let video_color = match video_report.grade {
                VideoHealthGrade::Pass => Color32::from_rgb(0, 220, 130),
                VideoHealthGrade::Warning => Color32::from_rgb(255, 200, 40),
                VideoHealthGrade::Critical => Color32::from_rgb(255, 70, 70),
            };
            ui.horizontal(|ui| {
                ui.label(RichText::new("📺 Video Subsystem Health").strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(format!("{:?}", video_report.grade)).color(video_color).strong());
                });
            });
            ui.separator();

            ui.label(format!("Frame CRC32:      0x{:08X}", video_report.latest_frame_crc32));
            ui.label(format!("Brightness:       {:.2}%", video_report.average_brightness * 100.0));
            ui.label(format!("Frozen Frames:    {}", video_report.frozen_frame_count));
            ui.label(format!("Active Sprites:   {}", video_report.active_sprites));
            ui.label(format!("Visible Layers:   0x{:02X}", video_report.visible_layers_mask));
        });
    });

    ui.add_space(6.0);

    // IO Bus Flight Data Recorder
    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("🛫 IO Bus Flight Data Recorder (Recent Hardware Bus Writes)").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(format!("Total Events: {}", gba.mmu.flight_recorder.total_events)).weak().small());
            });
        });
        ui.separator();

        ui.horizontal(|ui| {
            ui.label("Filter by Name/Addr:");
            ui.text_edit_singleline(&mut state.filter_query);
            if !state.filter_query.is_empty() && ui.button("✖").clicked() {
                state.filter_query.clear();
            }
        });

        let events = gba.mmu.flight_recorder.recent_events(128);
        let query = state.filter_query.trim().to_uppercase();

        ScrollArea::vertical()
            .max_height(200.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("flight_recorder_grid")
                    .striped(true)
                    .spacing([12.0, 3.0])
                    .show(ui, |ui| {
                        ui.label(RichText::new("Cycle").strong().small());
                        ui.label(RichText::new("PC").strong().small());
                        ui.label(RichText::new("Op").strong().small());
                        ui.label(RichText::new("Register").strong().small());
                        ui.label(RichText::new("Addr").strong().small());
                        ui.label(RichText::new("Val").strong().small());
                        ui.label(RichText::new("Size").strong().small());
                        ui.end_row();

                        for ev in events.iter().rev() {
                            if !query.is_empty() {
                                let match_name = ev.reg_name.to_uppercase().contains(&query);
                                let match_addr = format!("0x{:03X}", ev.addr).contains(&query);
                                if !match_name && !match_addr {
                                    continue;
                                }
                            }

                            ui.label(RichText::new(format!("{}", ev.cycle)).monospace().small());
                            ui.label(RichText::new(format!("0x{:08X}", ev.pc)).monospace().small());
                            ui.label(
                                RichText::new(if ev.is_write { "WR" } else { "RD" })
                                    .color(if ev.is_write {
                                        Color32::from_rgb(255, 170, 70)
                                    } else {
                                        Color32::from_rgb(70, 170, 255)
                                    })
                                    .monospace()
                                    .small(),
                            );
                            ui.label(RichText::new(&ev.reg_name).strong().small());
                            ui.label(RichText::new(format!("0x{:03X}", ev.addr)).monospace().small());
                            ui.label(RichText::new(format!("0x{:08X}", ev.val)).monospace().small());
                            ui.label(RichText::new(format!("{}b", ev.size)).small());
                            ui.end_row();
                        }
                    });
            });
    });
}
