//! Audio Processing Unit (APU) Live Inspector & Oscilloscope
//!
//! Provides comprehensive real-time audio diagnostics:
//! - Live master stereo waveform oscilloscope
//! - Channel 1 (Square + Sweep) frequency, duty, envelope, and sweep metrics
//! - Channel 2 (Square) frequency, duty, and envelope diagnostics
//! - Channel 3 (Wave) volume and 32-sample Wave RAM preview
//! - Channel 4 (White Noise) LFSR mode and polynomial dividers
//! - DirectSound Channels A & B FIFO queue depth and timer routing
//! - Interactive Mute and Solo controls per channel

use crate::gba::Gba;
use egui::{Color32, Pos2, RichText, Stroke, Ui, Vec2};

pub fn show_audio_inspector(ui: &mut Ui, gba: &mut Gba) {
    ui.heading("🎛 APU Live Audio Inspector & Oscilloscope");
    ui.label(RichText::new("Real-time telemetry for DMG sound channels, DirectSound FIFOs, and master mixing.").weak().small());
    ui.separator();

    // 1. Live Oscilloscope
    ui.label(RichText::new("Master Output Oscilloscope (Live Waveform):").strong());
    let desired_size = Vec2::new(ui.available_width(), 90.0);
    let (rect, _) = ui.allocate_exact_size(desired_size, egui::Sense::hover());

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, Color32::from_rgb(15, 18, 24));
    painter.rect_stroke(rect, 4.0, Stroke::new(1.0_f32, Color32::from_rgb(45, 55, 75)), egui::StrokeKind::Outside);

    // Grid center line (0.0 V)
    let mid_y = rect.center().y;
    painter.line_segment(
        [Pos2::new(rect.left(), mid_y), Pos2::new(rect.right(), mid_y)],
        Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 30)),
    );

    // Waveform plotting
    let scope_buf = &gba.mmu.apu.scope_buffer;
    let start_idx = gba.mmu.apu.scope_idx;
    let n_samples = scope_buf.len();
    let dx = rect.width() / (n_samples as f32);
    let half_h = (rect.height() - 8.0) * 0.5;

    let mut points = Vec::with_capacity(n_samples);
    for i in 0..n_samples {
        let sample_idx = (start_idx + i) % n_samples;
        let s = scope_buf[sample_idx].clamp(-1.0, 1.0);
        let x = rect.left() + (i as f32) * dx;
        let y = mid_y - s * half_h;
        points.push(Pos2::new(x, y));
    }

    if points.len() > 1 {
        for w in points.windows(2) {
            painter.line_segment([w[0], w[1]], Stroke::new(1.5_f32, Color32::from_rgb(0, 240, 180)));
        }
    }

    ui.add_space(6.0);

    // 2. Master Sound Registers (SOUNDCNT_X, SOUNDCNT_L, SOUNDCNT_H)
    let soundcnt_x = gba.mmu.apu.soundcnt_x;
    let master_enabled = (soundcnt_x & (1 << 7)) != 0;
    let soundcnt_l = gba.mmu.apu.soundcnt_l;
    let soundcnt_h = gba.mmu.apu.soundcnt_h;

    let vol_r = (soundcnt_l & 7) as u8;
    let vol_l = ((soundcnt_l >> 4) & 7) as u8;
    let dmg_ratio_str = match soundcnt_h & 3 {
        0 => "25%",
        1 => "50%",
        2 => "100%",
        _ => "Prohibited (100%)",
    };

    ui.horizontal(|ui| {
        let status_color = if master_enabled { Color32::from_rgb(0, 220, 120) } else { Color32::RED };
        ui.label(RichText::new(if master_enabled { "● MASTER AUDIO: ON" } else { "○ MASTER AUDIO: OFF" }).color(status_color).strong());
        ui.separator();
        ui.label(format!("Master Vol: L {}/7 | R {}/7", vol_l, vol_r));
        ui.separator();
        ui.label(format!("DMG Ratio: {}", dmg_ratio_str));
    });

    ui.separator();

    // 3. PSG Channel 1: Square + Frequency Sweep
    let ch1 = &gba.mmu.apu.dmg.ch1;
    let ch1_hz = if ch1.frequency < 2048 {
        131072.0 / (2048.0 - ch1.frequency as f32)
    } else {
        0.0
    };
    let duty_names = ["12.5%", "25.0%", "50.0%", "75.0%"];

    egui::CollapsingHeader::new(RichText::new("PSG Channel 1: Square Wave with Sweep (SOUND1CNT)").strong())
        .default_open(true)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let badge = if ch1.active {
                    RichText::new("● ACTIVE").color(Color32::from_rgb(0, 230, 100)).strong()
                } else {
                    RichText::new("○ INACTIVE").color(Color32::GRAY)
                };
                ui.label(badge);
                ui.label(format!("Pitch: {} (raw: {} | {:.1} Hz)", note_name(ch1_hz), ch1.frequency, ch1_hz));
                ui.label(format!("Duty: {} (step: {}/8)", duty_names[ch1.duty & 3], ch1.duty_step));
            });

            ui.horizontal(|ui| {
                let vol = ch1.envelope.volume;
                let vol_bar = format!("Vol: {:02}/15 [{}{}]", vol, "■".repeat(vol as usize), "░".repeat(15 - vol as usize));
                ui.label(vol_bar);
                ui.label(format!(
                    "Envelope: Init {} | Dir: {} | Period: {} | Timer: {}",
                    ch1.envelope.initial_volume,
                    if ch1.envelope.direction_inc { "+" } else { "-" },
                    ch1.envelope.period,
                    ch1.envelope.timer
                ));
            });

            ui.horizontal(|ui| {
                ui.label(format!(
                    "Sweep: Shift: {} | Dir: {} | Period: {} | Timer: {} | Shadow: {} | Enabled: {}",
                    ch1.sweep.shift,
                    if ch1.sweep.decrease { "Dec" } else { "Inc" },
                    ch1.sweep.period,
                    ch1.sweep.timer,
                    ch1.sweep.shadow_freq,
                    ch1.sweep.enabled
                ));
                ui.label(format!("Length: {}/64 (en: {})", ch1.length_counter, ch1.length_enabled));
            });

            ui.horizontal(|ui| {
                let is_muted = gba.mmu.apu.audio_output.is_channel_muted(2);
                if ui.button(if is_muted { "🔊 Unmute Ch1" } else { "🔇 Mute Ch1" }).clicked() {
                    gba.mmu.apu.audio_output.set_channel_muted(2, !is_muted);
                }
                if ui.button("Solo Ch1").clicked() {
                    gba.mmu.apu.audio_output.set_channel_mute_mask(!(1 << 2) & 0x3F);
                }
            });
        });

    // 4. PSG Channel 2: Square Wave (Envelope only)
    let ch2 = &gba.mmu.apu.dmg.ch2;
    let ch2_hz = if ch2.frequency < 2048 {
        131072.0 / (2048.0 - ch2.frequency as f32)
    } else {
        0.0
    };

    egui::CollapsingHeader::new(RichText::new("PSG Channel 2: Square Wave (SOUND2CNT)").strong())
        .default_open(false)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let badge = if ch2.active {
                    RichText::new("● ACTIVE").color(Color32::from_rgb(0, 230, 100)).strong()
                } else {
                    RichText::new("○ INACTIVE").color(Color32::GRAY)
                };
                ui.label(badge);
                ui.label(format!("Pitch: {} (raw: {} | {:.1} Hz)", note_name(ch2_hz), ch2.frequency, ch2_hz));
                ui.label(format!("Duty: {} | Vol: {}/15 | Len: {}/64", duty_names[ch2.duty & 3], ch2.envelope.volume, ch2.length_counter));
            });
            ui.horizontal(|ui| {
                let is_muted = gba.mmu.apu.audio_output.is_channel_muted(3);
                if ui.button(if is_muted { "🔊 Unmute Ch2" } else { "🔇 Mute Ch2" }).clicked() {
                    gba.mmu.apu.audio_output.set_channel_muted(3, !is_muted);
                }
                if ui.button("Solo Ch2").clicked() {
                    gba.mmu.apu.audio_output.set_channel_mute_mask(!(1 << 3) & 0x3F);
                }
            });
        });

    // 5. PSG Channel 3: Programmable Wave RAM
    let ch3 = &gba.mmu.apu.dmg.ch3;
    let ch3_hz = if ch3.frequency < 2048 {
        2097152.0 / (2048.0 - ch3.frequency as f32) / 32.0
    } else {
        0.0
    };
    let ch3_vol_str = if ch3.force_75 {
        "75%"
    } else {
        match ch3.volume_code {
            1 => "100%",
            2 => "50%",
            3 => "25%",
            _ => "0%",
        }
    };

    egui::CollapsingHeader::new(RichText::new("PSG Channel 3: Programmable Wave RAM (SOUND3CNT)").strong())
        .default_open(false)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let badge = if ch3.active {
                    RichText::new("● ACTIVE").color(Color32::from_rgb(0, 230, 100)).strong()
                } else {
                    RichText::new("○ INACTIVE").color(Color32::GRAY)
                };
                ui.label(badge);
                ui.label(format!("Pitch: {:.1} Hz | Vol: {} | Master En: {}", ch3_hz, ch3_vol_str, ch3.master_enable));
                ui.label(format!("Len: {}/256", ch3.length_counter));
            });

            // Mini Wave RAM preview: 32 nibbles
            ui.label("Wave RAM (32 samples):");
            let mut wave_str = String::with_capacity(64);
            for i in 0..32 {
                let byte = ch3.wave_ram[i / 2];
                let nibble = if (i & 1) == 0 { byte >> 4 } else { byte & 0xF };
                let hex_char = match nibble {
                    0..=9 => (b'0' + nibble) as char,
                    10..=15 => (b'A' + nibble - 10) as char,
                    _ => '?',
                };
                wave_str.push(hex_char);
            }
            ui.monospace(wave_str);

            ui.horizontal(|ui| {
                let is_muted = gba.mmu.apu.audio_output.is_channel_muted(4);
                if ui.button(if is_muted { "🔊 Unmute Ch3" } else { "🔇 Mute Ch3" }).clicked() {
                    gba.mmu.apu.audio_output.set_channel_muted(4, !is_muted);
                }
                if ui.button("Solo Ch3").clicked() {
                    gba.mmu.apu.audio_output.set_channel_mute_mask(!(1 << 4) & 0x3F);
                }
            });
        });

    // 6. PSG Channel 4: White Noise
    let ch4 = &gba.mmu.apu.dmg.ch4;
    egui::CollapsingHeader::new(RichText::new("PSG Channel 4: White Noise (SOUND4CNT)").strong())
        .default_open(false)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let badge = if ch4.active {
                    RichText::new("● ACTIVE").color(Color32::from_rgb(0, 230, 100)).strong()
                } else {
                    RichText::new("○ INACTIVE").color(Color32::GRAY)
                };
                ui.label(badge);
                ui.label(format!("Mode: {} | Ratio: {} | Shift: {} | Vol: {}/15",
                    if ch4.width_7bit { "7-bit (Metallic)" } else { "15-bit (Smooth)" },
                    ch4.ratio,
                    ch4.shift_clock,
                    ch4.envelope.volume
                ));
            });
            ui.horizontal(|ui| {
                let is_muted = gba.mmu.apu.audio_output.is_channel_muted(5);
                if ui.button(if is_muted { "🔊 Unmute Ch4" } else { "🔇 Mute Ch4" }).clicked() {
                    gba.mmu.apu.audio_output.set_channel_muted(5, !is_muted);
                }
                if ui.button("Solo Ch4").clicked() {
                    gba.mmu.apu.audio_output.set_channel_mute_mask(!(1 << 5) & 0x3F);
                }
            });
        });

    // 7. DirectSound Channels A & B (DMA FIFOs)
    egui::CollapsingHeader::new(RichText::new("DirectSound Channels A & B (DMA FIFO PCM)").strong())
        .default_open(false)
        .show(ui, |ui| {
            let sa = &gba.mmu.apu.sound_a;
            let sb = &gba.mmu.apu.sound_b;
            ui.horizontal(|ui| {
                ui.label(format!(
                    "FIFO A: Queue: {}/32 | Vol: {:.0}% | Timer: {} | L: {} R: {} | Sample: {}",
                    sa.fifo.len(),
                    sa.volume * 100.0,
                    sa.timer_select,
                    sa.left_enable,
                    sa.right_enable,
                    sa.current_sample
                ));
            });
            ui.horizontal(|ui| {
                ui.label(format!(
                    "FIFO B: Queue: {}/32 | Vol: {:.0}% | Timer: {} | L: {} R: {} | Sample: {}",
                    sb.fifo.len(),
                    sb.volume * 100.0,
                    sb.timer_select,
                    sb.left_enable,
                    sb.right_enable,
                    sb.current_sample
                ));
            });
        });
}

fn note_name(freq_hz: f32) -> &'static str {
    if freq_hz <= 20.0 { return "---"; }
    let notes = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    let midi = (12.0 * (freq_hz / 440.0).log2() + 69.0).round() as i32;
    if midi < 0 || midi > 127 { return "---"; }
    notes[(midi % 12) as usize]
}
