//! Nintendo DS 3D Graphics Engine Coordinator
//!
//! Integrates the fixed-point Geometry Engine (matrix stacks, GXFIFO, vertex transforms)
//! with the Software Polygon Rasterizer (depth buffering, texture mapping).

pub mod geometry;
pub mod matrix;
pub mod rasterizer;

use geometry::GeometryEngine;
use rasterizer::Rasterizer;

pub struct Engine3D {
    pub geom: GeometryEngine,
    pub rasterizer: Rasterizer,
}

impl Default for Engine3D {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine3D {
    pub fn new() -> Self {
        Self {
            geom: GeometryEngine::new(),
            rasterizer: Rasterizer::new(),
        }
    }

    /// Called at VBlank start (scanline 192): handles buffer swaps and renders 3D scene
    pub fn on_vblank(&mut self, tex_vram: &[u8], pal_vram: &[u8]) {
        self.geom.swap_buffers_if_requested();
        if !self.geom.polygons_front.is_empty() {
            self.rasterizer.render_frame(&self.geom.polygons_front, tex_vram, pal_vram);
        }
    }

    /// Read 32-bit I/O register in 3D range (0x0400_0320..=0x0400_06A0)
    pub fn read_io(&mut self, addr: u32) -> u32 {
        match addr {
            0x0400_0060 => self.rasterizer.disp3dcnt as u32,
            0x0400_0350 => self.rasterizer.clear_color,
            0x0400_0354 => self.rasterizer.clear_depth,
            0x0400_0358 => self.rasterizer.fog_color,
            0x0400_035C => self.rasterizer.fog_offset as u32,
            0x0400_0600 => self.geom.read_gxstat(),
            0x0400_0620 => self.geom.pos_result[0] as u32,
            0x0400_0624 => self.geom.pos_result[1] as u32,
            0x0400_0628 => self.geom.pos_result[2] as u32,
            0x0400_062C => self.geom.pos_result[3] as u32,
            0x0400_0630 => self.geom.vec_result[0] as u32,
            0x0400_0634 => self.geom.vec_result[1] as u32,
            0x0400_0638 => self.geom.vec_result[2] as u32,
            0x0400_0640..=0x0400_067C => {
                let idx = ((addr - 0x0400_0640) / 4) as usize;
                self.geom.read_clip_matrix(idx)
            }
            0x0400_0680..=0x0400_06A0 => {
                let idx = ((addr - 0x0400_0680) / 4) as usize;
                self.geom.read_vector_matrix(idx)
            }
            _ => 0,
        }
    }

    /// Write 32-bit I/O register in 3D range
    pub fn write_io(&mut self, addr: u32, val: u32) {
        match addr {
            0x0400_0060 => self.rasterizer.disp3dcnt = val as u16,
            0x0400_0350 => self.rasterizer.clear_color = val,
            0x0400_0354 => self.rasterizer.clear_depth = val,
            0x0400_0358 => self.rasterizer.fog_color = val,
            0x0400_035C => self.rasterizer.fog_offset = val as u16,
            0x0400_0360..=0x0400_037C => {
                let idx = (addr - 0x0400_0360) as usize;
                if idx + 3 < self.rasterizer.fog_table.len() {
                    self.rasterizer.fog_table[idx..idx + 4].copy_from_slice(&val.to_le_bytes());
                }
            }
            0x0400_0380..=0x0400_03BC => {
                let idx = ((addr - 0x0400_0380) / 2) as usize;
                if idx + 1 < self.rasterizer.toon_table.len() {
                    self.rasterizer.toon_table[idx] = val as u16;
                    self.rasterizer.toon_table[idx + 1] = (val >> 16) as u16;
                }
            }
            0x0400_0400..=0x0400_05CC => self.geom.write_command_port(addr, val),
            0x0400_0600 => self.geom.write_gxstat(val),
            _ => {}
        }
    }

    /// Write 16-bit I/O register in 3D range
    pub fn write_io_u16(&mut self, addr: u32, val: u16) {
        match addr {
            0x0400_0060 => self.rasterizer.disp3dcnt = val,
            0x0400_0350 => self.rasterizer.clear_color = (self.rasterizer.clear_color & 0xFFFF_0000) | (val as u32),
            0x0400_0352 => self.rasterizer.clear_color = (self.rasterizer.clear_color & 0x0000_FFFF) | ((val as u32) << 16),
            0x0400_0354 => self.rasterizer.clear_depth = val as u32,
            0x0400_0358 => self.rasterizer.fog_color = (self.rasterizer.fog_color & 0xFFFF_0000) | (val as u32),
            0x0400_035A => self.rasterizer.fog_color = (self.rasterizer.fog_color & 0x0000_FFFF) | ((val as u32) << 16),
            0x0400_035C => self.rasterizer.fog_offset = val,
            0x0400_0380..=0x0400_03BC => {
                let idx = ((addr - 0x0400_0380) / 2) as usize;
                if idx < self.rasterizer.toon_table.len() {
                    self.rasterizer.toon_table[idx] = val;
                }
            }
            0x0400_0400..=0x0400_05CC => self.geom.write_command_port(addr, val as u32),
            _ => {}
        }
    }
}
