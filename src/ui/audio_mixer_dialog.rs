//! 6-Channel Sound Mixer & Fast-Forward Pitch DSP Dialog

use crate::gba::Gba;
use egui::{RichText, Window};

#[derive(Default)]
pub struct AudioMixerDialog {
    pub is_open: bool,
}

impl AudioMixerDialog {
    pub fn new() -> Self {
        Self { is_open: false }
    }

    pub fn show(&mut self, ctx: &egui::Context, gba: &mut Gba, toast: &mut Option<String>) {
        if !self.is_open {
            return;
        }

        let mut open = self.is_open;
        Window::new("🎛 Audio Channel Mixer & Turbo DSP")
            .open(&mut open)
            .default_width(460.0)
            .resizable(false)
            .show(ctx, |ui| {
                ui.heading("Hardware Channel Mute & Solo");
                ui.label(RichText::new("Isolate melodies, basslines, or sound effects independently.").weak().small());
                ui.separator();

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
                if ui.radio_value(&mut ff_mode, 0, "Unmodified Audio (High-Speed Pitch)").clicked() {
                    gba.mmu.apu.audio_output.set_fast_forward_mode(0);
                    *toast = Some("Fast-Forward: Raw audio active".to_string());
                }
            });
        self.is_open = open;
    }
}
