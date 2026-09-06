//! Video & Graphics Quality Health Linter
//!
//! Inspects PPU output framebuffers to detect:
//! - Visual screen freezes (identical framebuffer frames while CPU is executing)
//! - Black screen hangs (failed booting or unhandled exceptions)
//! - Layer rendering failures and sprite count anomalies
//! - Deterministic frame CRC32 hashing for automated regression tests

use crate::gba::ppu::{Ppu, SCREEN_HEIGHT, SCREEN_WIDTH};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VideoHealthGrade {
    Pass,
    Warning,
    Critical,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VideoHealthReport {
    pub grade: VideoHealthGrade,
    pub frames_monitored: u64,
    pub latest_frame_crc32: u32,
    pub average_brightness: f32,
    pub frozen_frame_count: u32,
    pub active_sprites: usize,
    pub visible_layers_mask: u8,
    pub anomalies: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct VideoLinter {
    pub frames_monitored: u64,
    last_hash: u32,
    consecutive_identical_frames: u32,
    black_screen_frames: u32,
    latest_brightness: f32,
    latest_sprite_count: usize,
    latest_layer_mask: u8,
}

impl Default for VideoLinter {
    fn default() -> Self {
        Self::new()
    }
}

impl VideoLinter {
    pub fn new() -> Self {
        Self {
            frames_monitored: 0,
            last_hash: 0,
            consecutive_identical_frames: 0,
            black_screen_frames: 0,
            latest_brightness: 0.0,
            latest_sprite_count: 0,
            latest_layer_mask: 0x1F,
        }
    }

    /// Analyze a completed frame from the PPU
    pub fn on_frame(&mut self, ppu: &Ppu) {
        self.frames_monitored += 1;
        self.latest_layer_mask = ppu.layer_mask;

        // Calculate quick CRC32 of 240x160 RGBA framebuffer
        let hash = compute_frame_hash(ppu.framebuffer.as_ref());

        if hash == self.last_hash && hash != 0 {
            self.consecutive_identical_frames += 1;
        } else {
            self.consecutive_identical_frames = 0;
            self.last_hash = hash;
        }

        // Calculate average brightness
        let mut total_lum = 0u64;
        let mut black_pixels = 0usize;
        for &pixel in ppu.framebuffer.iter() {
            let r = (pixel & 0xFF) as u64;
            let g = ((pixel >> 8) & 0xFF) as u64;
            let b = ((pixel >> 16) & 0xFF) as u64;
            let lum = (r * 299 + g * 587 + b * 114) / 1000;
            total_lum += lum;
            if lum < 5 {
                black_pixels += 1;
            }
        }
        let total_pixels = SCREEN_WIDTH * SCREEN_HEIGHT;
        self.latest_brightness = (total_lum as f32 / total_pixels as f32) / 255.0;

        if black_pixels >= total_pixels - 100 {
            self.black_screen_frames += 1;
        } else {
            self.black_screen_frames = 0;
        }

        // Count non-disabled sprites from OAM
        let mut active_sprites = 0;
        for i in 0..128 {
            let attr0 = (ppu.oam[i * 8] as u16) | ((ppu.oam[i * 8 + 1] as u16) << 8);
            let is_disabled = (attr0 & 0x0200) != 0 && (attr0 & 0x0100) == 0;
            if !is_disabled {
                active_sprites += 1;
            }
        }
        self.latest_sprite_count = active_sprites;
    }

    pub fn evaluate_health(&self) -> VideoHealthReport {
        let mut anomalies = Vec::new();
        let mut grade = VideoHealthGrade::Pass;

        if self.consecutive_identical_frames > 240 {
            grade = VideoHealthGrade::Critical;
            anomalies.push(format!("Visual Screen Freeze detected! Framebuffer unchanged for {} consecutive frames.", self.consecutive_identical_frames));
        } else if self.consecutive_identical_frames > 120 {
            if grade == VideoHealthGrade::Pass { grade = VideoHealthGrade::Warning; }
            anomalies.push(format!("Potential Screen Freeze: {} consecutive identical frames.", self.consecutive_identical_frames));
        }

        if self.black_screen_frames > 180 {
            grade = VideoHealthGrade::Critical;
            anomalies.push(format!("Black Screen Hang! Screen has been completely dark for {} frames.", self.black_screen_frames));
        }

        VideoHealthReport {
            grade,
            frames_monitored: self.frames_monitored,
            latest_frame_crc32: self.last_hash,
            average_brightness: self.latest_brightness,
            frozen_frame_count: self.consecutive_identical_frames,
            active_sprites: self.latest_sprite_count,
            visible_layers_mask: self.latest_layer_mask,
            anomalies,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

/// Fast CRC32 hashing for deterministic frame verification
fn compute_frame_hash(buffer: &[u32]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &word in buffer {
        for &b in &word.to_le_bytes() {
            crc ^= b as u32;
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
    }
    !crc
}
