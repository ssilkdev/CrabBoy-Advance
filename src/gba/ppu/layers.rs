//! PPU Per-Layer Buffers and Draw Command Emission (ROADMAP M5).
//!
//! Exposes isolated RGBA per-layer outputs (BG0..3, OBJ, Backdrop) and draw
//! commands alongside the standard 240x160 composited framebuffer.
//!
//! Required by downstream milestones:
//! - M6: HD Mode 7 (affine layer scaling)
//! - M7: HD sprite and tile replacement packs
//! - M8: Per-game widescreen expansion

use super::{SCREEN_HEIGHT, SCREEN_WIDTH};

/// Identified GBA hardware rendering layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PpuLayer {
    Backdrop = 0,
    Bg0 = 1,
    Bg1 = 2,
    Bg2 = 3,
    Bg3 = 4,
    Obj = 5,
}

impl PpuLayer {
    pub const ALL: [PpuLayer; 6] = [
        PpuLayer::Backdrop,
        PpuLayer::Bg0,
        PpuLayer::Bg1,
        PpuLayer::Bg2,
        PpuLayer::Bg3,
        PpuLayer::Obj,
    ];

    #[inline]
    pub fn index(self) -> usize {
        self as usize
    }

    pub fn name(self) -> &'static str {
        match self {
            PpuLayer::Backdrop => "Backdrop",
            PpuLayer::Bg0 => "BG0",
            PpuLayer::Bg1 => "BG1",
            PpuLayer::Bg2 => "BG2",
            PpuLayer::Bg3 => "BG3",
            PpuLayer::Obj => "OBJ",
        }
    }
}

/// Rendering classification of a GBA background layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerKind {
    Backdrop,
    Text,
    Affine,
    Bitmap,
    Obj,
}

/// Recorded rendering command and state snapshot for a layer on a scanline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayerDrawCommand {
    pub scanline: u16,
    pub layer: PpuLayer,
    pub kind: LayerKind,
    pub priority: u8,
    pub h_offset: u16,
    pub v_offset: u16,
    pub affine_matrix: [i16; 4],
    pub affine_origin: [i32; 2],
    pub blend_mode: u8,
    pub window_enabled: bool,
}

/// Isolated framebuffer surfaces for all GBA layers.
///
/// Pixels use standard 32-bit RGBA (0xAABBGGRR in little-endian format).
/// Transparent/unrendered pixels contain 0x00000000 (alpha 0).
pub struct PpuLayerBuffers {
    pub buffers: [Box<[u32; SCREEN_WIDTH * SCREEN_HEIGHT]>; 6],
}

impl Default for PpuLayerBuffers {
    fn default() -> Self {
        Self::new()
    }
}

impl PpuLayerBuffers {
    pub fn new() -> Self {
        Self {
            buffers: [
                Box::new([0u32; SCREEN_WIDTH * SCREEN_HEIGHT]),
                Box::new([0u32; SCREEN_WIDTH * SCREEN_HEIGHT]),
                Box::new([0u32; SCREEN_WIDTH * SCREEN_HEIGHT]),
                Box::new([0u32; SCREEN_WIDTH * SCREEN_HEIGHT]),
                Box::new([0u32; SCREEN_WIDTH * SCREEN_HEIGHT]),
                Box::new([0u32; SCREEN_WIDTH * SCREEN_HEIGHT]),
            ],
        }
    }

    /// Clear all layer buffers to transparent black (0x00000000).
    pub fn clear(&mut self) {
        for buf in &mut self.buffers {
            buf.fill(0);
        }
    }

    #[inline]
    pub fn get_layer(&self, layer: PpuLayer) -> &[u32; SCREEN_WIDTH * SCREEN_HEIGHT] {
        &self.buffers[layer.index()]
    }

    #[inline]
    pub fn get_layer_mut(&mut self, layer: PpuLayer) -> &mut [u32; SCREEN_WIDTH * SCREEN_HEIGHT] {
        &mut self.buffers[layer.index()]
    }

    /// Writes an individual pixel to a specific layer's framebuffer surface.
    #[inline]
    pub fn set_pixel(&mut self, layer: PpuLayer, x: usize, y: usize, rgba: u32) {
        if x < SCREEN_WIDTH && y < SCREEN_HEIGHT {
            self.buffers[layer.index()][y * SCREEN_WIDTH + x] = rgba;
        }
    }
}
