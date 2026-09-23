//! 6-Channel Sound Mixer, HD M4A Re-Synthesis & Turbo DSP Dialog
//!
//! Features:
//! - Real-time switching between HD 48 kHz M4A re-synthesis and native 8-bit hardware audio
//! - Quality interpolation algorithms (Linear, Cubic Hermite spline, Band-limited Sinc)
//! - Jukebox song selector and live player
//! - Standard MIDI File (.mid) exporter
//! - Multi-track 48 kHz WAV stem exporter
//! - Hardware channel isolate, mute, and solo controls

use crate::gba::Gba;
use crate::gba::m4a::{AudioEngineMode, M4aInterpolation};
use egui::{Color32, RichText, Window};
use std::fs;

pub struct AudioMixerDialog {
    pub is_open: bool,
    pub selected_song_id: u16,
    pub stem_export_duration: f32,
}

impl Default for AudioMixerDialog {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioMixerDialog {
    pub fn new() -> Self {
        Self {
            is_open: false,
            selected_song_id: 0,
            stem_export_duration: 60.0,
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, gba: &mut Gba, toast: &mut Option<String>) {
        if !self.is_open {
            return;
        }

        let mut open = self.is_open;
        Window::new("🎛 Audio Channel Mixer & HD Re-Synthesis")
            .open(&mut open)
            .default_width(520.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.heading("HD Music Re-Synthesis (48 kHz Sappy / M4A)");
                ui.label(RichText::new("Plays GBA music at 48 kHz with cubic/sinc interpolation instead of the hardware's 8-bit mixer.").weak().small());
                ui.separator();

                // Status Badge
                if gba.is_m4a_game() {
                    let prof = gba.m4a.profile.as_ref().unwrap();
                    ui.horizontal(|ui| {
                        ui.colored_label(Color32::from_rgb(100, 220, 100), "✓ M4A Audio Engine Active:");
                        ui.label(RichText::new(format!("{} ({} songs detected)", prof.title, prof.song_count)).strong());
                    });
                } else {
                    ui.horizontal(|ui| {
                        ui.colored_label(Color32::from_rgb(180, 180, 180), "ℹ Engine Status:");
                        ui.label(RichText::new("Non-M4A game (falling back to native APU hardware audio)").weak());
                    });
                }

                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let mut mode = gba.hd_audio_mode();
                    if ui.radio_value(&mut mode, AudioEngineMode::HdReSynthesis, "HD Re-Synthesis (48 kHz)").clicked() {
                        gba.set_hd_audio_mode(AudioEngineMode::HdReSynthesis);
                        *toast = Some("Audio Mode: HD Music Re-Synthesis Active".to_string());
                    }
                    if ui.radio_value(&mut mode, AudioEngineMode::HardwareOnly, "Native Hardware Audio (APU 8-bit)").clicked() {
                        gba.set_hd_audio_mode(AudioEngineMode::HardwareOnly);
                        *toast = Some("Audio Mode: Native Hardware Audio Active".to_string());
                    }
                });

                if gba.hd_audio_mode() == AudioEngineMode::HdReSynthesis {
                    ui.group(|ui| {
                        ui.label(RichText::new("Sampler Quality & Processing").strong());
                        ui.horizontal(|ui| {
                            let mut interp = gba.m4a.config.interpolation;
                            if ui.radio_value(&mut interp, M4aInterpolation::Linear, "Linear").clicked() {
                                gba.m4a.config.interpolation = M4aInterpolation::Linear;
                                gba.m4a.sampler.interpolation = M4aInterpolation::Linear;
                            }
                            if ui.radio_value(&mut interp, M4aInterpolation::CubicHermite, "Cubic Hermite (Spline)").clicked() {
                                gba.m4a.config.interpolation = M4aInterpolation::CubicHermite;
                                gba.m4a.sampler.interpolation = M4aInterpolation::CubicHermite;
                            }
                            if ui.radio_value(&mut interp, M4aInterpolation::Sinc, "Band-Limited Sinc (8-pt)").clicked() {
                                gba.m4a.config.interpolation = M4aInterpolation::Sinc;
                                gba.m4a.sampler.interpolation = M4aInterpolation::Sinc;
                            }
                        });

                        ui.horizontal(|ui| {
                            ui.checkbox(&mut gba.m4a.config.reverb_enabled, "Stereo Reverb");
                            if gba.m4a.config.reverb_enabled {
                                ui.label("Wet:");
                                ui.add(egui::Slider::new(&mut gba.m4a.config.reverb_level, 0.0..=0.8).show_value(false));
                                gba.m4a.sampler.reverb_level = gba.m4a.config.reverb_level;
                            }
                            gba.m4a.sampler.reverb_enabled = gba.m4a.config.reverb_enabled;

                            ui.checkbox(&mut gba.m4a.config.mix_hardware_sfx, "Mix Hardware SFX");
                        });
                    });

                    // Jukebox & Exporter (when M4A profile available)
                    let song_count_opt = gba.m4a.profile.as_ref().map(|p| p.song_count);
                    if let Some(song_count) = song_count_opt {
                        ui.add_space(4.0);
                        ui.group(|ui| {
                            ui.label(RichText::new("🎵 Jukebox & Stem/MIDI Exporter").strong());
                            ui.horizontal(|ui| {
                                ui.label("Song ID:");
                                let max_song = (song_count.saturating_sub(1) as u16).max(1);
                                ui.add(egui::Slider::new(&mut self.selected_song_id, 0..=max_song));

                                if ui.button("▶ Play").clicked() {
                                    if gba.play_m4a_song(self.selected_song_id) {
                                        *toast = Some(format!("Playing song #{}", self.selected_song_id));
                                    } else {
                                        *toast = Some(format!("Could not play song #{}", self.selected_song_id));
                                    }
                                }
                                if ui.button("⏹ Stop").clicked() {
                                    gba.stop_m4a_song();
                                    *toast = Some("Playback stopped".to_string());
                                }
                            });

                            ui.horizontal(|ui| {
                                if ui.button("🎼 Export Song to MIDI (.mid)...").clicked() {
                                    if let Some(path) = rfd::FileDialog::new()
                                        .set_file_name(format!("song_{:03}.mid", self.selected_song_id))
                                        .add_filter("MIDI file", &["mid"])
                                        .save_file()
                                    {
                                        match gba.export_m4a_song_midi(self.selected_song_id) {
                                            Ok(bytes) => {
                                                if let Err(e) = fs::write(&path, bytes) {
                                                    *toast = Some(format!("Failed to write MIDI: {}", e));
                                                } else {
                                                    *toast = Some(format!("Exported MIDI to {}", path.display()));
                                                }
                                            }
                                            Err(e) => {
                                                *toast = Some(format!("MIDI export error: {}", e));
                                            }
                                        }
                                    }
                                }

                                ui.label("Stem Len:");
                                ui.add(egui::Slider::new(&mut self.stem_export_duration, 10.0..=180.0).suffix("s"));

                                if ui.button("📦 Export Stems (.wav)...").clicked() {
                                    if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                                        match gba.export_m4a_song_stems(self.selected_song_id, self.stem_export_duration) {
                                            Ok(stems) => {
                                                let mut success_count = 0;
                                                for stem in stems {
                                                    let stem_filename = format!("{}.wav", stem.name.to_lowercase().replace(' ', "_"));
                                                    let stem_path = folder.join(stem_filename);
                                                    if crate::gba::m4a::write_wav_file(&stem_path, &stem.samples, 48_000).is_ok() {
                                                        success_count += 1;
                                                    }
                                                }
                                                *toast = Some(format!("Exported {} stems to {}", success_count, folder.display()));
                                            }
                                            Err(e) => {
                                                *toast = Some(format!("Stem export error: {}", e));
                                            }
                                        }
                                    }
                                }
                            });
                        });
                    }
                }

                ui.separator();
                ui.heading("Hardware Channel Mute & Solo");
                ui.label(RichText::new("Isolate melodies, basslines, or sound effects independently.").weak().small());

                let channel_names = [
                    "DirectSound A (DMA PCM Music)",
                    "DirectSound B (DMA PCM SFX)",
                    "PSG Channel 1 (Square + Sweep)",
                    "PSG Channel 2 (Square)",
                    "PSG Channel 3 (Programmable Wave)",
                    "PSG Channel 4 (White Noise)",
                ];

                let mute_mask = gba.mmu.apu.audio_output.channel_mute_mask();

                for (idx, &name) in channel_names.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let is_muted = (mute_mask & (1 << idx)) != 0;
                        let mut unmuted = !is_muted;

                        if ui.checkbox(&mut unmuted, name).changed() {
                            gba.mmu.apu.audio_output.set_channel_muted(idx, !unmuted);
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Solo").clicked() {
                                // Mute all except this channel
                                let solo_mask = !(1 << idx) & 0x3F;
                                gba.mmu.apu.audio_output.set_channel_mute_mask(solo_mask);
                                *toast = Some(format!("Soloing {}", name));
                            }
                        });
                    });
                }

                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("🔊 Unmute All Channels").clicked() {
                        gba.mmu.apu.audio_output.set_channel_mute_mask(0);
                        *toast = Some("All channels unmuted".to_string());
                    }
                    if ui.button("🔇 Mute All").clicked() {
                        gba.mmu.apu.audio_output.set_channel_mute_mask(0x3F);
                        *toast = Some("All channels muted".to_string());
                    }
                });

                ui.separator();
                ui.heading("Fast-Forward & Turbo Audio Behavior");
                ui.label(RichText::new("Prevents harsh, high-pitched chipmunk squeaking during fast-forward.").weak().small());

                let mut ff_mode = gba.mmu.apu.audio_output.fast_forward_mode();
                if ui.radio_value(&mut ff_mode, 1, "Smart Mute during Fast-Forward (Recommended)").clicked() {
                    gba.mmu.apu.audio_output.set_fast_forward_mode(1);
                    *toast = Some("Fast-Forward: Smart Mute active".to_string());
                }
                if ui.radio_value(&mut ff_mode, 2, "Pitch-Preserved (skip ahead in short grains)").clicked() {
                    gba.mmu.apu.audio_output.set_fast_forward_mode(2);
                    *toast = Some("Fast-Forward: pitch-preserved audio active".to_string());
                }
                if ui.radio_value(&mut ff_mode, 0, "Unmodified Audio (High-Speed Pitch)").clicked() {
                    gba.mmu.apu.audio_output.set_fast_forward_mode(0);
                    *toast = Some("Fast-Forward: Raw audio active".to_string());
                }
            });
        self.is_open = open;
    }
}
