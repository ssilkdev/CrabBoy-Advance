//! LCD Ghosting and Frame Blending (ROADMAP M5).
//!
//! Provides authentic Game Boy Advance LCD panel response emulation and
//! transparency de-flickering.
//!
//! Original non-backlit AGB-001 screens had response times of ~50ms+, causing
//! natural pixel persistence (ghosting). GBA developers exploited this by
//! flickering sprites/layers at 30 Hz (alternating frames) to achieve 50%
//! transparency (e.g. shadows in F-Zero, shields, water reflections).
//!
//! On modern fast OLED and high-refresh LCD panels, that 30 Hz flicker looks like
//! harsh strobing. Frame blending restores natural transparency and authentic
//! motion characteristics.

use super::{SCREEN_HEIGHT, SCREEN_WIDTH};
use serde::{Deserialize, Serialize};

pub const FRAME_PIXELS: usize = SCREEN_WIDTH * SCREEN_HEIGHT;

/// Available frame blending algorithms.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum FrameBlendMode {
    /// No blending (instant 60fps native frame output).
    Off,
    /// 50/50 blend between current and previous frame.
    Simple50,
    /// Intelligent 30 Hz flicker eliminator: blends alternating pixels while
    /// keeping moving backgrounds and sprites sharp.
    SmartDeFlicker,
    /// Authentic LCD phosphor/liquid crystal response decay (0.0 = full ghosting, 1.0 = instant).
    LcdGhosting { decay: f32 },
}

impl Default for FrameBlendMode {
    fn default() -> Self {
        Self::Off
    }
}

impl FrameBlendMode {
    pub const ALL: [FrameBlendMode; 4] = [
        FrameBlendMode::Off,
        FrameBlendMode::Simple50,
        FrameBlendMode::SmartDeFlicker,
        FrameBlendMode::LcdGhosting { decay: 0.65 },
    ];

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Off => "Off (Instant 60 Hz)",
            Self::Simple50 => "50/50 Frame Blend (Smooth Transparency)",
            Self::SmartDeFlicker => "Smart De-Flicker (Motion-Preserving)",
            Self::LcdGhosting { .. } => "Authentic LCD Ghosting (AGB-001 Persistence)",
        }
    }
}

/// Frame blending processor maintaining historical frames.
pub struct FrameBlender {
    prev_frame: Box<[u32; FRAME_PIXELS]>,
    prev_prev_frame: Box<[u32; FRAME_PIXELS]>,
    output_buffer: Box<[u32; FRAME_PIXELS]>,
    has_history: bool,
}

impl Default for FrameBlender {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameBlender {
    pub fn new() -> Self {
        Self {
            prev_frame: Box::new([0xFF00_0000; FRAME_PIXELS]),
            prev_prev_frame: Box::new([0xFF00_0000; FRAME_PIXELS]),
            output_buffer: Box::new([0xFF00_0000; FRAME_PIXELS]),
            has_history: false,
        }
    }

    /// Reset historical frame buffers (e.g. on game change or save state load).
    pub fn reset(&mut self) {
        self.prev_frame.fill(0xFF00_0000);
        self.prev_prev_frame.fill(0xFF00_0000);
        self.output_buffer.fill(0xFF00_0000);
        self.has_history = false;
    }

    /// Process the current framebuffer according to the selected blend mode.
    pub fn blend<'a>(
        &'a mut self,
        current: &[u32; FRAME_PIXELS],
        mode: FrameBlendMode,
    ) -> &'a [u32; FRAME_PIXELS] {
        if !self.has_history || matches!(mode, FrameBlendMode::Off) {
            self.output_buffer.copy_from_slice(&current[..]);
            self.prev_prev_frame.copy_from_slice(&self.prev_frame[..]);
            self.prev_frame.copy_from_slice(&current[..]);
            self.has_history = true;
            return &self.output_buffer;
        }

        match mode {
            FrameBlendMode::Off => &self.output_buffer,

            FrameBlendMode::Simple50 => {
                // High-speed bitwise parallel 50/50 blend with zero channel crosstalk
                for i in 0..FRAME_PIXELS {
                    self.output_buffer[i] = blend_50_50(current[i], self.prev_frame[i]);
                }
                self.prev_prev_frame.copy_from_slice(&self.prev_frame[..]);
                self.prev_frame.copy_from_slice(&current[..]);
                &self.output_buffer
            }

            FrameBlendMode::SmartDeFlicker => {
                // If a pixel oscillates at 30 Hz (frame t == frame t-2, but != frame t-1),
                // blend frame t and t-1 to remove strobing while leaving moving content sharp.
                for i in 0..FRAME_PIXELS {
                    let cur = current[i];
                    let p1 = self.prev_frame[i];
                    let p2 = self.prev_prev_frame[i];

                    let is_flicker = cur == p2 && cur != p1;
                    self.output_buffer[i] = if is_flicker {
                        blend_50_50(cur, p1)
                    } else {
                        cur
                    };
                }
                self.prev_prev_frame.copy_from_slice(&self.prev_frame[..]);
                self.prev_frame.copy_from_slice(&current[..]);
                &self.output_buffer
            }

            FrameBlendMode::LcdGhosting { decay } => {
                let alpha = decay.clamp(0.05, 0.95);
                let inv_alpha = 1.0 - alpha;

                for i in 0..FRAME_PIXELS {
                    let cur = current[i];
                    let prev = self.prev_frame[i];

                    let cr = (cur & 0xFF) as f32;
                    let cg = ((cur >> 8) & 0xFF) as f32;
                    let cb = ((cur >> 16) & 0xFF) as f32;

                    let pr = (prev & 0xFF) as f32;
                    let pg = ((prev >> 8) & 0xFF) as f32;
                    let pb = ((prev >> 16) & 0xFF) as f32;

                    let r = (alpha * cr + inv_alpha * pr).round() as u32;
                    let g = (alpha * cg + inv_alpha * pg).round() as u32;
                    let b = (alpha * cb + inv_alpha * pb).round() as u32;

                    self.output_buffer[i] = 0xFF00_0000 | (b << 16) | (g << 8) | r;
                }

                self.prev_prev_frame.copy_from_slice(&self.prev_frame[..]);
                self.prev_frame.copy_from_slice(&self.output_buffer[..]);
                &self.output_buffer
            }
        }
    }
}

/// Bitwise 50/50 blend of two 32-bit RGBA (0xAABBGGRR) words.
#[inline]
pub fn blend_50_50(c1: u32, c2: u32) -> u32 {
    let rb = (((c1 & 0x00FF_00FF) + (c2 & 0x00FF_00FF)) >> 1) & 0x00FF_00FF;
    let ga = ((((c1 & 0xFF00_FF00) >> 1) + ((c2 & 0xFF00_FF00) >> 1))) & 0xFF00_FF00;
    rb | ga
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blend_50_50_arithmetic() {
        let white = 0xFFFF_FFFF;
        let black = 0xFF00_0000;
        let mid = blend_50_50(white, black);
        assert_eq!(mid, 0xFF7F_7F7F);
    }

    #[test]
    fn test_smart_deflicker_detects_30hz() {
        let mut blender = FrameBlender::new();
        let frame_a = [0xFF00_00FF; FRAME_PIXELS]; // Red
        let frame_b = [0xFFFF_0000; FRAME_PIXELS]; // Blue

        // Frame 1: A
        let _ = blender.blend(&frame_a, FrameBlendMode::SmartDeFlicker);
        // Frame 2: B
        let _ = blender.blend(&frame_b, FrameBlendMode::SmartDeFlicker);
        // Frame 3: A (oscillating 30Hz flicker!)
        let out = blender.blend(&frame_a, FrameBlendMode::SmartDeFlicker);

        let expected_mid = blend_50_50(frame_a[0], frame_b[0]);
        assert_eq!(out[0], expected_mid, "Smart de-flicker blended 30Hz alternating frames");
    }
}
