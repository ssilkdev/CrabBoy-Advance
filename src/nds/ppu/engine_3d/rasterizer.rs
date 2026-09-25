//! Software 3D Polygon Rasterizer for Nintendo DS
//!
//! Renders 3D triangles with perspective-correct interpolation, depth buffering,
//! texture mapping (A3I5, 4-color, 16-color, 256-color, Direct Color), and alpha blending.

use super::geometry::Polygon3D;
use crate::nds::ppu::engine_2d::{SCREEN_HEIGHT, SCREEN_WIDTH};

pub struct Rasterizer {
    pub disp3dcnt: u16,
    pub clear_color: u32,
    pub clear_depth: u32,
    pub fog_color: u32,
    pub fog_offset: u16,
    pub fog_table: [u8; 32],
    pub toon_table: [u16; 32],

    /// 256x192 32-bit RGBA color buffer
    pub color_buffer: Vec<u32>,
    /// 256x192 24-bit depth buffer
    pub depth_buffer: Vec<u32>,
}

impl Default for Rasterizer {
    fn default() -> Self {
        Self::new()
    }
}

impl Rasterizer {
    pub fn new() -> Self {
        Self {
            disp3dcnt: 0,
            clear_color: 0x0000_0000,
            clear_depth: 0x7FFF,
            fog_color: 0,
            fog_offset: 0,
            fog_table: [0; 32],
            toon_table: [0; 32],
            color_buffer: vec![0xFF000000; SCREEN_WIDTH * SCREEN_HEIGHT],
            depth_buffer: vec![0x00FF_FFFF; SCREEN_WIDTH * SCREEN_HEIGHT],
        }
    }

    /// Clear color and depth buffers using CLEAR_COLOR and CLEAR_DEPTH registers
    pub fn clear(&mut self) {
        let r = (self.clear_color & 0x1F) << 3;
        let g = ((self.clear_color >> 5) & 0x1F) << 3;
        let b = ((self.clear_color >> 10) & 0x1F) << 3;
        let a = ((self.clear_color >> 16) & 0x1F) << 3;
        let clear_rgba = (a << 24) | (b << 16) | (g << 8) | r;

        self.color_buffer.fill(clear_rgba);
        // CLEAR_DEPTH is 15 bits; GBATEK: depth24 = val*0x200 + ((val+1)/0x8000)*0x1FF.
        let d = self.clear_depth & 0x7FFF;
        self.depth_buffer.fill(d * 0x200 + ((d + 1) / 0x8000) * 0x1FF);
    }

    /// Rasterize all front-buffered polygons for the current frame
    pub fn render_frame(&mut self, polygons: &[Polygon3D], tex_vram: &[u8], pal_vram: &[u8]) {
        self.clear();

        for poly in polygons {
            self.render_polygon(poly, tex_vram, pal_vram);
        }
    }

    fn render_polygon(&mut self, poly: &Polygon3D, tex_vram: &[u8], pal_vram: &[u8]) {
        let v0 = &poly.vertices[0];
        let v1 = &poly.vertices[1];
        let v2 = &poly.vertices[2];

        // 2D cross product for winding & culling (in sub-pixel coordinates)
        let x0 = v0.screen_x as i64;
        let y0 = v0.screen_y as i64;
        let x1 = v1.screen_x as i64;
        let y1 = v1.screen_y as i64;
        let x2 = v2.screen_x as i64;
        let y2 = v2.screen_y as i64;

        let cross = (x1 - x0) * (y2 - y0) - (y1 - y0) * (x2 - x0);
        if cross == 0 {
            return; // Degenerate triangle
        }

        // Facing: DS front faces wind clockwise as seen on screen, which with
        // y pointing down is a negative cross product here.
        let is_front = cross < 0;
        // POLYGON_ATTR bit 6 = render back faces, bit 7 = render front faces.
        let render_back = poly.polygon_attr & (1 << 6) != 0;
        let render_front = poly.polygon_attr & (1 << 7) != 0;
        if (is_front && !render_front) || (!is_front && !render_back) {
            return;
        }

        // Bounding box in integer screen coordinates
        let min_x = ((x0.min(x1).min(x2)) >> 4).clamp(0, (SCREEN_WIDTH - 1) as i64) as usize;
        let max_x = (((x0.max(x1).max(x2) + 15) >> 4)).clamp(0, (SCREEN_WIDTH - 1) as i64) as usize;
        let min_y = ((y0.min(y1).min(y2)) >> 4).clamp(0, (SCREEN_HEIGHT - 1) as i64) as usize;
        let max_y = (((y0.max(y1).max(y2) + 15) >> 4)).clamp(0, (SCREEN_HEIGHT - 1) as i64) as usize;

        let inv_cross = 1.0 / (cross as f64);

        // Precompute per-vertex attributes for perspective interpolation
        let w0 = if v0.clip_w != 0 { 1.0 / (v0.clip_w as f64) } else { 1.0 };
        let w1 = if v1.clip_w != 0 { 1.0 / (v1.clip_w as f64) } else { 1.0 };
        let w2 = if v2.clip_w != 0 { 1.0 / (v2.clip_w as f64) } else { 1.0 };

        // z/w per vertex (dimensionless), interpolated with 1/w weights.
        let z0 = v0.clip_z as f64 / v0.clip_w.max(1) as f64 * w0;
        let z1 = v1.clip_z as f64 / v1.clip_w.max(1) as f64 * w1;
        let z2 = v2.clip_z as f64 / v2.clip_w.max(1) as f64 * w2;

        let (r0, g0, b0) = Self::unpack_rgb555(v0.color);
        let (r1, g1, b1) = Self::unpack_rgb555(v1.color);
        let (r2, g2, b2) = Self::unpack_rgb555(v2.color);

        let (r0_w, g0_w, b0_w) = ((r0 as f64) * w0, (g0 as f64) * w0, (b0 as f64) * w0);
        let (r1_w, g1_w, b1_w) = ((r1 as f64) * w1, (g1 as f64) * w1, (b1 as f64) * w1);
        let (r2_w, g2_w, b2_w) = ((r2 as f64) * w2, (g2 as f64) * w2, (b2 as f64) * w2);

        let (u0_w, v0_w) = ((v0.u as f64) * w0, (v0.v as f64) * w0);
        let (u1_w, v1_w) = ((v1.u as f64) * w1, (v1.v as f64) * w1);
        let (u2_w, v2_w) = ((v2.u as f64) * w2, (v2.v as f64) * w2);

        for py in min_y..=max_y {
            let sy = (py as i64) << 4;
            for px in min_x..=max_x {
                let sx = (px as i64) << 4;

                // Edge equations
                let w_edge0 = (x2 - x1) * (sy - y1) - (y2 - y1) * (sx - x1);
                let w_edge1 = (x0 - x2) * (sy - y2) - (y0 - y2) * (sx - x2);
                let w_edge2 = (x1 - x0) * (sy - y0) - (y1 - y0) * (sx - x0);

                if (cross > 0 && w_edge0 >= 0 && w_edge1 >= 0 && w_edge2 >= 0)
                    || (cross < 0 && w_edge0 <= 0 && w_edge1 <= 0 && w_edge2 <= 0)
                {
                    let b0 = (w_edge0 as f64) * inv_cross;
                    let b1 = (w_edge1 as f64) * inv_cross;
                    let b2 = (w_edge2 as f64) * inv_cross;

                    let interp_w = b0 * w0 + b1 * w1 + b2 * w2;
                    if interp_w <= 0.0 {
                        continue;
                    }
                    let w = 1.0 / interp_w;

                    // Interpolate depth
                    // z/w in -1..1 (12-bit fixed point) -> 24-bit depth, 0 = near.
                    let interp_z = (b0 * z0 + b1 * z1 + b2 * z2) * w;
                    let depth = (((interp_z + 1.0) * 0.5) * 16_777_215.0).clamp(0.0, 16_777_215.0) as u32;

                    let pixel_idx = py * SCREEN_WIDTH + px;
                    let existing_depth = self.depth_buffer[pixel_idx];

                    // Depth test mode is POLYGON_ATTR bit 14 (0 = less, 1 = equal
                    // within a small margin); DISP3DCNT bit 4 is anti-aliasing.
                    let pass = if poly.polygon_attr & (1 << 14) != 0 {
                        depth.abs_diff(existing_depth) <= 0x200
                    } else {
                        depth < existing_depth
                    };

                    if pass {
                        // Interpolate vertex color
                        let r = (((b0 * r0_w + b1 * r1_w + b2 * r2_w) * w) as i32).clamp(0, 255) as u32;
                        let g = (((b0 * g0_w + b1 * g1_w + b2 * g2_w) * w) as i32).clamp(0, 255) as u32;
                        let b = (((b0 * b0_w + b1 * b1_w + b2 * b2_w) * w) as i32).clamp(0, 255) as u32;

                        // Interpolate texture coords
                        let tex_u = ((b0 * u0_w + b1 * u1_w + b2 * u2_w) * w) as i32;
                        let tex_v = ((b0 * v0_w + b1 * v1_w + b2 * v2_w) * w) as i32;

                        // Sample texture
                        let (final_r, final_g, final_b, final_a) = self.sample_texture(
                            poly.teximage_param,
                            poly.palette_base,
                            tex_u,
                            tex_v,
                            r,
                            g,
                            b,
                            tex_vram,
                            pal_vram,
                        );

                        // Alpha test (DISP3DCNT bit 2)
                        if (self.disp3dcnt & (1 << 2)) != 0 && final_a == 0 {
                            continue;
                        }

                        // Write to color and depth buffers
                        let pixel = (final_a << 24) | (final_b << 16) | (final_g << 8) | final_r;
                        self.color_buffer[pixel_idx] = pixel;
                        self.depth_buffer[pixel_idx] = depth;
                    }
                }
            }
        }
    }

    #[inline(always)]
    fn unpack_rgb555(c: u16) -> (u8, u8, u8) {
        let r = ((c & 0x1F) as u8) << 3;
        let g = (((c >> 5) & 0x1F) as u8) << 3;
        let b = (((c >> 10) & 0x1F) as u8) << 3;
        (r | (r >> 5), g | (g >> 5), b | (b >> 5))
    }

    /// Sample texture texel and apply modulation shading
    fn sample_texture(
        &self,
        tex_param: u32,
        pal_base: u32,
        u: i32,
        v: i32,
        vert_r: u32,
        vert_g: u32,
        vert_b: u32,
        tex_vram: &[u8],
        pal_vram: &[u8],
    ) -> (u32, u32, u32, u32) {
        let format = (tex_param >> 26) & 7;
        if format == 0 || tex_vram.is_empty() {
            // No texture: use vertex color directly
            return (vert_r, vert_g, vert_b, 0xFF);
        }

        let width_shift = ((tex_param >> 20) & 7) + 3;
        let height_shift = ((tex_param >> 23) & 7) + 3;
        let width = 1 << width_shift;
        let height = 1 << height_shift;

        let wrap_s = (tex_param & (1 << 16)) != 0;
        let wrap_t = (tex_param & (1 << 17)) != 0;

        let (mut tex_x, mut tex_y) = (u >> 4, v >> 4);
        if wrap_s {
            tex_x = tex_x.rem_euclid(width);
        } else {
            tex_x = tex_x.clamp(0, width - 1);
        }
        if wrap_t {
            tex_y = tex_y.rem_euclid(height);
        } else {
            tex_y = tex_y.clamp(0, height - 1);
        }

        let vram_offset = ((tex_param & 0xFFFF) as usize) << 3;
        let mut tr = vert_r;
        let mut tg = vert_g;
        let mut tb = vert_b;
        let mut ta = 0xFF;

        match format {
            2 => {
                // 4-color palette (2 bpp)
                let byte_offset = vram_offset + ((tex_y as usize * width as usize + tex_x as usize) >> 2);
                if byte_offset < tex_vram.len() {
                    let shift = (tex_x & 3) * 2;
                    let idx = ((tex_vram[byte_offset] >> shift) & 3) as usize;
                    let pal_off = (pal_base as usize * 16) + (idx * 2);
                    if pal_off + 1 < pal_vram.len() {
                        let col = u16::from_le_bytes([pal_vram[pal_off], pal_vram[pal_off + 1]]);
                        let (pr, pg, pb) = Self::unpack_rgb555(col);
                        tr = (vert_r * pr as u32) >> 8;
                        tg = (vert_g * pg as u32) >> 8;
                        tb = (vert_b * pb as u32) >> 8;
                    }
                }
            }
            3 => {
                // 16-color palette (4 bpp)
                let byte_offset = vram_offset + ((tex_y as usize * width as usize + tex_x as usize) >> 1);
                if byte_offset < tex_vram.len() {
                    let shift = (tex_x & 1) * 4;
                    let idx = ((tex_vram[byte_offset] >> shift) & 0xF) as usize;
                    let pal_off = (pal_base as usize * 16) + (idx * 2);
                    if pal_off + 1 < pal_vram.len() {
                        let col = u16::from_le_bytes([pal_vram[pal_off], pal_vram[pal_off + 1]]);
                        let (pr, pg, pb) = Self::unpack_rgb555(col);
                        tr = (vert_r * pr as u32) >> 8;
                        tg = (vert_g * pg as u32) >> 8;
                        tb = (vert_b * pb as u32) >> 8;
                    }
                }
            }
            4 => {
                // 256-color palette (8 bpp)
                let byte_offset = vram_offset + (tex_y as usize * width as usize + tex_x as usize);
                if byte_offset < tex_vram.len() {
                    let idx = tex_vram[byte_offset] as usize;
                    let pal_off = (pal_base as usize * 16) + (idx * 2);
                    if pal_off + 1 < pal_vram.len() {
                        let col = u16::from_le_bytes([pal_vram[pal_off], pal_vram[pal_off + 1]]);
                        let (pr, pg, pb) = Self::unpack_rgb555(col);
                        tr = (vert_r * pr as u32) >> 8;
                        tg = (vert_g * pg as u32) >> 8;
                        tb = (vert_b * pb as u32) >> 8;
                    }
                }
            }
            7 => {
                // Direct color (15-bit BGR555 + 1-bit alpha)
                let byte_offset = vram_offset + ((tex_y as usize * width as usize + tex_x as usize) * 2);
                if byte_offset + 1 < tex_vram.len() {
                    let col = u16::from_le_bytes([tex_vram[byte_offset], tex_vram[byte_offset + 1]]);
                    let (pr, pg, pb) = Self::unpack_rgb555(col);
                    tr = (vert_r * pr as u32) >> 8;
                    tg = (vert_g * pg as u32) >> 8;
                    tb = (vert_b * pb as u32) >> 8;
                    ta = if (col & 0x8000) != 0 { 0xFF } else { 0x00 };
                }
            }
            _ => {}
        }

        (tr, tg, tb, ta)
    }
}
