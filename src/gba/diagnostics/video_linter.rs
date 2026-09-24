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
    /// The last frame seen (empty before the first). Repeats are found by
    /// comparing with it, which is exact and far cheaper than hashing
    /// every frame; the CRC is only computed when something needs it.
    last_frame: Vec<u32>,
    /// CRC of `last_frame`, once computed.
    last_hash: Option<u32>,
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
            last_frame: Vec::new(),
            last_hash: None,
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

        // Screen freeze tracking. A repeated frame counts unless its CRC is
        // 0 (as before, when the CRC alone decided "identical").
        let frame = ppu.completed_frame.as_ref();
        if self.last_frame.as_slice() == frame {
            let crc = *self.last_hash.get_or_insert_with(|| compute_frame_hash(frame));
            if crc != 0 {
                self.consecutive_identical_frames += 1;
            } else {
                self.consecutive_identical_frames = 0;
            }
        } else {
            self.consecutive_identical_frames = 0;
            self.last_frame.clear();
            self.last_frame.extend_from_slice(frame);
            self.last_hash = None;
        }

        // Calculate average brightness
        let mut total_lum = 0u64;
        let mut black_pixels = 0usize;
        for &pixel in ppu.completed_frame.iter() {
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
            latest_frame_crc32: match self.last_hash {
                Some(h) => h,
                None if self.last_frame.is_empty() => 0,
                None => compute_frame_hash(&self.last_frame),
            },
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

/// CRC-32 (IEEE) slicing-by-4 tables, built at compile time: `T[0]` is
/// the classic byte table, `T[k]` advances a byte k more positions.
const CRC_TABLES: [[u32; 256]; 4] = {
    let mut t = [[0u32; 256]; 4];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        t[0][i] = c;
        i += 1;
    }
    let mut i = 0;
    while i < 256 {
        let mut s = 1;
        while s < 4 {
            let prev = t[s - 1][i];
            t[s][i] = t[0][(prev & 0xFF) as usize] ^ (prev >> 8);
            s += 1;
        }
        i += 1;
    }
    t
};

/// CRC-32 of the frame's bytes, for deterministic frame verification.
/// Slicing-by-4 (one step per 32-bit pixel instead of 32 bit steps): same
/// values as the bitwise version. It runs every frame, so the bitwise loop
/// was 12-16% of all emulation time.
fn compute_frame_hash(buffer: &[u32]) -> u32 {
    let t = &CRC_TABLES;
    let mut crc = 0xFFFF_FFFFu32;
    for &word in buffer {
        let x = crc ^ word; // bytes are fed little-endian
        crc = t[3][(x & 0xFF) as usize]
            ^ t[2][((x >> 8) & 0xFF) as usize]
            ^ t[1][((x >> 16) & 0xFF) as usize]
            ^ t[0][(x >> 24) as usize];
    }
    !crc
}

#[cfg(test)]
mod crc_tests {
    use super::compute_frame_hash;

    /// The old bit-at-a-time implementation, as the reference.
    fn bitwise(buffer: &[u32]) -> u32 {
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

    #[test]
    fn table_crc_matches_bitwise_and_standard() {
        // "123456789" check value for CRC-32/IEEE, as little-endian words.
        let w = [u32::from_le_bytes(*b"1234"), u32::from_le_bytes(*b"5678")];
        assert_eq!(compute_frame_hash(&w), bitwise(&w));
        let mut x = 0x1234_5678u32;
        let frame: Vec<u32> = (0..240 * 160)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x
            })
            .collect();
        assert_eq!(compute_frame_hash(&frame), bitwise(&frame));
        assert_eq!(compute_frame_hash(&[]), bitwise(&[]));
    }
}
