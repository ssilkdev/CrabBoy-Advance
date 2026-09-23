//! HD Mode 7 High-Resolution Affine Rendering Engine (ROADMAP M6).
//!
//! Renders GBA affine backgrounds (Mode 1 BG2, Mode 2 BG2/BG3, Modes 3-5 bitmap)
//! and affine sprites at 2x, 4x, or 8x internal resolution.
//!
//! Key capabilities:
//! - Subpixel affine texture sampling: replaces blocky, aliased Mode 7 rotation
//!   and scaling with vector-smooth edges.
//! - Perspective scanline interpolation: interpolates affine transformation
//!   parameters (PA, PB, PC, PD, X, Y) across sub-scanlines to eliminate
//!   scanline step artifacts in 3D perspective games (e.g. Mario Kart, F-Zero).
//! - Subpixel affine sprite (rotscale OBJ) evaluation.
//! - Seamless native layer compositing: composites HD affine layers with native
//!   text/sprite layers (Backdrop, BG0/1, UI HUDs) with exact priority resolution
//!   and GBA color special effects (alpha blending, brightness up/down).
//! - Dual output modes: full HD framebuffer (`HdFrame`) for high-DPI displays
//!   and downsampled SSAA (`downsample_ssaa`) for native resolution with
//!   supersampled anti-aliasing.

use std::sync::Arc;
use crate::gba::hd_pack::{extract_sprite, recolor_rgba, HdReplacement};
use super::{
    blend::{apply_color_effects, bgr555_to_rgb888, rgb888_to_bgr555, Pixel},
    layers::{LayerKind, PpuLayer},
    obj::get_sprite_size,
    Ppu, SCREEN_HEIGHT, SCREEN_WIDTH,
};
use serde::{Deserialize, Serialize};

/// Supported internal rendering scale multipliers for HD Mode 7.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HdScale {
    /// Native 240x160 resolution.
    Off,
    /// 2x internal resolution (480x320).
    X2,
    /// 4x internal resolution (960x640) - recommended sweet spot.
    X4,
    /// 8x internal resolution (1920x1280) - ultra high definition.
    X8,
}

impl Default for HdScale {
    fn default() -> Self {
        Self::Off
    }
}

impl HdScale {
    pub const ALL: [HdScale; 4] = [
        HdScale::Off,
        HdScale::X2,
        HdScale::X4,
        HdScale::X8,
    ];

    #[inline]
    pub fn factor(self) -> usize {
        match self {
            Self::Off => 1,
            Self::X2 => 2,
            Self::X4 => 4,
            Self::X8 => 8,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Off => "Off (Native 240x160)",
            Self::X2 => "2x HD (480x320)",
            Self::X4 => "4x HD (960x640) [Recommended]",
            Self::X8 => "8x Ultra HD (1920x1280)",
        }
    }
}

/// Configuration parameters for HD Mode 7 rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HdMode7Config {
    /// Active internal scaling multiplier.
    pub scale: HdScale,
    /// Smooth affine parameter transitions across scanlines (eliminates stair-stepping on tracks).
    pub perspective_interpolation: bool,
    /// Downsample HD output to native 240x160 using supersampled anti-aliasing (SSAA).
    pub ssaa: bool,
}

impl Default for HdMode7Config {
    fn default() -> Self {
        Self {
            scale: HdScale::Off,
            perspective_interpolation: true,
            ssaa: false,
        }
    }
}

/// High-resolution rendered frame buffer with dimensions `(240 * scale) x (160 * scale)`.
#[derive(Clone, Debug, PartialEq)]
pub struct HdFrame {
    pub width: usize,
    pub height: usize,
    pub scale: usize,
    pub pixels: Vec<u32>,
}

impl HdFrame {
    pub fn new(scale: usize) -> Self {
        let width = SCREEN_WIDTH * scale;
        let height = SCREEN_HEIGHT * scale;
        Self {
            width,
            height,
            scale,
            pixels: vec![0xFF00_0000; width * height],
        }
    }

    /// Downsamples the high-resolution frame to native resolution using box-filtered
    /// supersampled anti-aliasing (SSAA). Supports widescreen frames of arbitrary width.
    pub fn downsample_ssaa_wide(&self) -> Vec<u32> {
        let target_w = self.width / self.scale;
        let target_h = self.height / self.scale;
        let mut out = vec![0u32; target_w * target_h];
        let scale = self.scale;
        if scale == 1 {
            out.copy_from_slice(&self.pixels[..target_w * target_h]);
            return out;
        }

        let inv_count = 1.0 / ((scale * scale) as f32);

        for ny in 0..target_h {
            for nx in 0..target_w {
                let mut sum_r = 0.0f32;
                let mut sum_g = 0.0f32;
                let mut sum_b = 0.0f32;

                for sy in 0..scale {
                    let hy = ny * scale + sy;
                    let row_offset = hy * self.width;
                    for sx in 0..scale {
                        let hx = nx * scale + sx;
                        let p = self.pixels[row_offset + hx];
                        sum_r += (p & 0xFF) as f32;
                        sum_g += ((p >> 8) & 0xFF) as f32;
                        sum_b += ((p >> 16) & 0xFF) as f32;
                    }
                }

                let r = (sum_r * inv_count).round() as u32;
                let g = (sum_g * inv_count).round() as u32;
                let b = (sum_b * inv_count).round() as u32;
                out[ny * target_w + nx] = 0xFF00_0000 | (b << 16) | (g << 8) | r;
            }
        }
        out
    }

    /// Downsamples the high-resolution frame to native 240x160 using box-filtered
    /// supersampled anti-aliasing (SSAA).
    pub fn downsample_ssaa(&self) -> Box<[u32; SCREEN_WIDTH * SCREEN_HEIGHT]> {
        let mut out = Box::new([0u32; SCREEN_WIDTH * SCREEN_HEIGHT]);
        let wide = self.downsample_ssaa_wide();
        let copy_len = wide.len().min(SCREEN_WIDTH * SCREEN_HEIGHT);
        out[..copy_len].copy_from_slice(&wide[..copy_len]);
        out
    }
}

/// Extracted affine sprite descriptor for high-resolution evaluation.
struct HdAffineSprite {
    sprite_x: i32,
    raw_y: i32,
    orig_w: usize,
    orig_h: usize,
    bound_w: usize,
    bound_h: usize,
    pa: i16,
    pb: i16,
    pc: i16,
    pd: i16,
    raw_tile: usize,
    is_8bpp: bool,
    priority: u8,
    pal_num: usize,
    is_semi_trans: bool,
}

#[derive(Clone)]
struct HdReplacedSprite {
    sprite_x: i32,
    raw_y: i32,
    orig_w: usize,
    orig_h: usize,
    priority: u8,
    is_semi_trans: bool,
    hflip: bool,
    vflip: bool,
    replacement: Arc<HdReplacement>,
    palette: Vec<u16>,
}

/// Renders a complete frame using HD Mode 7 or HD Pack replacements.
///
/// Returns `None` if neither HD Mode 7 nor an HD Pack is active.
pub fn render_hd_mode7(ppu: &Ppu, config: &HdMode7Config) -> Option<HdFrame> {
    let scale = if config.scale != HdScale::Off {
        config.scale.factor()
    } else if ppu.is_hd_pack_enabled() {
        ppu.hd_pack().map_or(1, |p| p.scale).clamp(2, 8)
    } else {
        return None;
    };

    if scale <= 1 {
        return None;
    }

    let mut frame = HdFrame::new(scale);

    // Extract draw commands indexed by [scanline][layer_index]
    let mut cmd_map = [None; 160 * 6];
    for cmd in &ppu.draw_commands {
        let sc = cmd.scanline as usize;
        let l_idx = cmd.layer.index();
        if sc < 160 && l_idx < 6 {
            cmd_map[sc * 6 + l_idx] = Some(*cmd);
        }
    }

    // Collect active affine sprites
    let mut affine_sprites = Vec::with_capacity(32);
    let obj_char_base = 0x10000;
    let mapping_1d = (ppu.dispcnt & (1 << 6)) != 0;

    // Collect HD replaced sprites if pack is loaded
    let mut hd_replaced_sprites = Vec::new();
    if ppu.is_hd_pack_enabled() {
        if let Some(pack) = ppu.hd_pack() {
            for i in 0..128 {
                if let Some(raw) = extract_sprite(&ppu.oam[..], i, &ppu.vram[..], &ppu.palette_ram[..], ppu.dispcnt) {
                    if let Some(rep) = pack.find_sprite_replacement(raw.sprite_hash, raw.palette_hash) {
                        hd_replaced_sprites.push(HdReplacedSprite {
                            sprite_x: raw.sprite_x,
                            raw_y: raw.raw_y,
                            orig_w: raw.width,
                            orig_h: raw.height,
                            priority: raw.priority,
                            is_semi_trans: raw.is_semi_trans,
                            hflip: raw.hflip,
                            vflip: raw.vflip,
                            replacement: Arc::clone(rep),
                            palette: raw.palette,
                        });
                    }
                }
            }
        }
    }

    for i in 0..128 {
        let oam_addr = i * 8;
        let attr0 = (ppu.oam[oam_addr] as u16) | ((ppu.oam[oam_addr + 1] as u16) << 8);
        let attr1 = (ppu.oam[oam_addr + 2] as u16) | ((ppu.oam[oam_addr + 3] as u16) << 8);
        let attr2 = (ppu.oam[oam_addr + 4] as u16) | ((ppu.oam[oam_addr + 5] as u16) << 8);

        let is_affine = (attr0 & (1 << 8)) != 0;
        let is_disabled = !is_affine && ((attr0 & (1 << 9)) != 0);
        if is_disabled || !is_affine {
            continue;
        }

        let is_double_size = (attr0 & (1 << 9)) != 0;
        let obj_mode = (attr0 >> 10) & 3;
        let is_semi_trans = obj_mode == 1;
        let is_8bpp = (attr0 & (1 << 13)) != 0;
        let shape = (attr0 >> 14) & 3;
        let size = (attr1 >> 14) & 3;
        let (orig_w, orig_h) = get_sprite_size(shape, size);
        let (bound_w, bound_h) = if is_double_size {
            (orig_w * 2, orig_h * 2)
        } else {
            (orig_w, orig_h)
        };

        let raw_y = (attr0 & 0xFF) as i32;
        let raw_x = (attr1 & 0x1FF) as i32;
        let sprite_x = if raw_x >= 256 { raw_x - 512 } else { raw_x };

        if sprite_x >= 240 || sprite_x + (bound_w as i32) <= 0 {
            continue;
        }

        let affine_group = ((attr1 >> 9) & 0x1F) as usize;
        let group_base = affine_group * 32;
        let read_param = |idx: usize| -> i16 {
            let off = group_base + idx * 8 + 6;
            if off + 1 < ppu.oam.len() {
                (ppu.oam[off] as i16) | ((ppu.oam[off + 1] as i16) << 8)
            } else {
                0
            }
        };

        let pa = read_param(0);
        let pb = read_param(1);
        let pc = read_param(2);
        let pd = read_param(3);

        let raw_tile = (attr2 & 0x3FF) as usize;
        let priority = ((attr2 >> 10) & 3) as u8;
        let pal_num = ((attr2 >> 12) & 0x0F) as usize;

        affine_sprites.push(HdAffineSprite {
            sprite_x,
            raw_y,
            orig_w,
            orig_h,
            bound_w,
            bound_h,
            pa,
            pb,
            pc,
            pd,
            raw_tile,
            is_8bpp,
            priority,
            pal_num,
            is_semi_trans,
        });
    }

    let mode = (ppu.dispcnt & 7) as u8;
    let frame_flag = (ppu.dispcnt & (1 << 4)) != 0;
    let backdrop_color = (ppu.palette_ram[0] as u16) | ((ppu.palette_ram[1] as u16) << 8);

    let eva = ppu.bldalpha & 0x1F;
    let evb = (ppu.bldalpha >> 8) & 0x1F;
    let evy = ppu.bldy & 0x1F;

    // Windowing state
    let win0_enable = (ppu.dispcnt & (1 << 13)) != 0;
    let win1_enable = (ppu.dispcnt & (1 << 14)) != 0;
    let any_win = win0_enable || win1_enable;
    let win0_y1 = (ppu.win0v >> 8) as usize;
    let win0_y2 = (ppu.win0v & 0xFF) as usize;
    let win1_y1 = (ppu.win1v >> 8) as usize;
    let win1_y2 = (ppu.win1v & 0xFF) as usize;
    let win0_x1 = (ppu.win0h >> 8) as usize;
    let win0_x2 = (ppu.win0h & 0xFF) as usize;
    let win1_x1 = (ppu.win1h >> 8) as usize;
    let win1_x2 = (ppu.win1h & 0xFF) as usize;

    let f_scale = scale as f64;

    // Render every subpixel in the high-resolution viewport
    for hy in 0..frame.height {
        let ny = hy / scale;
        let sy = hy % scale;
        let t = sy as f64 / f_scale;

        // Determine active window mask for this native scanline
        let in_win0_y = win0_enable
            && (if win0_y1 <= win0_y2 {
                ny >= win0_y1 && ny < win0_y2
            } else {
                ny >= win0_y1 || ny < win0_y2
            });
        let in_win1_y = win1_enable
            && (if win1_y1 <= win1_y2 {
                ny >= win1_y1 && ny < win1_y2
            } else {
                ny >= win1_y1 || ny < win1_y2
            });

        let row_offset = hy * frame.width;

        for hx in 0..frame.width {
            let nx = hx / scale;

            // Window mask check
            let win_mask = if any_win {
                let in_win0 = in_win0_y
                    && (if win0_x1 <= win0_x2 {
                        nx >= win0_x1 && nx < win0_x2
                    } else {
                        nx >= win0_x1 || nx < win0_x2
                    });
                let in_win1 = in_win1_y
                    && (if win1_x1 <= win1_x2 {
                        nx >= win1_x1 && nx < win1_x2
                    } else {
                        nx >= win1_x1 || nx < win1_x2
                    });

                if in_win0 {
                    (ppu.winin & 0x3F) as u8
                } else if in_win1 {
                    ((ppu.winin >> 8) & 0x3F) as u8
                } else {
                    (ppu.winout & 0x3F) as u8
                }
            } else {
                0x3Fu8
            };

            // Pixel candidate array for priority sorting
            let mut candidates: [Pixel; 6] = [Pixel::default(); 6];
            let mut cand_count = 0;

            // 1. Backdrop (layer 5, priority 4)
            if (win_mask & (1 << 5)) != 0 {
                candidates[cand_count] = Pixel {
                    color: backdrop_color,
                    layer: 5,
                    priority: 4,
                    is_transparent: false,
                    is_obj_alpha: false,
                };
                cand_count += 1;
            }

            // 2. BG Layers (0..4)
            for bg_idx in 0..4 {
                if (ppu.dispcnt & (1 << (8 + bg_idx))) == 0 || (ppu.layer_mask & (1 << bg_idx)) == 0 {
                    continue;
                }
                if (win_mask & (1 << bg_idx)) == 0 {
                    continue;
                }

                let layer_enum = match bg_idx {
                    0 => PpuLayer::Bg0,
                    1 => PpuLayer::Bg1,
                    2 => PpuLayer::Bg2,
                    _ => PpuLayer::Bg3,
                };

                let cmd_opt = cmd_map[ny * 6 + layer_enum.index()];
                let is_affine = match mode {
                    1 if bg_idx == 2 => true,
                    2 if bg_idx >= 2 => true,
                    3..=5 if bg_idx == 2 => true,
                    _ => false,
                };

                if is_affine {
                    if let Some(cmd) = cmd_opt {
                        // Subpixel affine evaluation
                        let (pa, _pb, pc, _pd, ref_x, ref_y) = if config.perspective_interpolation
                            && ny + 1 < 160
                            && cmd_map[(ny + 1) * 6 + layer_enum.index()].is_some()
                        {
                            let cmd1 = cmd_map[(ny + 1) * 6 + layer_enum.index()].unwrap();
                            (
                                cmd.affine_matrix[0] as f64 * (1.0 - t) + cmd1.affine_matrix[0] as f64 * t,
                                cmd.affine_matrix[1] as f64 * (1.0 - t) + cmd1.affine_matrix[1] as f64 * t,
                                cmd.affine_matrix[2] as f64 * (1.0 - t) + cmd1.affine_matrix[2] as f64 * t,
                                cmd.affine_matrix[3] as f64 * (1.0 - t) + cmd1.affine_matrix[3] as f64 * t,
                                cmd.affine_origin[0] as f64 * (1.0 - t) + cmd1.affine_origin[0] as f64 * t,
                                cmd.affine_origin[1] as f64 * (1.0 - t) + cmd1.affine_origin[1] as f64 * t,
                            )
                        } else {
                            (
                                cmd.affine_matrix[0] as f64,
                                cmd.affine_matrix[1] as f64,
                                cmd.affine_matrix[2] as f64,
                                cmd.affine_matrix[3] as f64,
                                cmd.affine_origin[0] as f64 + (sy as f64 * cmd.affine_matrix[1] as f64 / f_scale),
                                cmd.affine_origin[1] as f64 + (sy as f64 * cmd.affine_matrix[3] as f64 / f_scale),
                            )
                        };

                        let x_sub = hx as f64 / f_scale;
                        let cur_x = ref_x + x_sub * pa;
                        let cur_y = ref_y + x_sub * pc;

                        let px = (cur_x.floor() as i64) >> 8;
                        let py = (cur_y.floor() as i64) >> 8;

                        if cmd.kind == LayerKind::Affine {
                            let bgcnt = cmd.bgcnt;
                            let wrap = (bgcnt & (1 << 13)) != 0;
                            let size_shift = ((bgcnt >> 14) & 3) as usize;
                            let size_px = (128 << size_shift) as i64;
                            let tiles_per_row = 16 << size_shift;
                            let char_base = (((bgcnt >> 2) & 3) as usize) * 16384;
                            let screen_base = (((bgcnt >> 8) & 0x1F) as usize) * 2048;

                            let in_bounds = px >= 0 && px < size_px && py >= 0 && py < size_px;
                            if in_bounds || wrap {
                                let map_x = px.rem_euclid(size_px) as usize;
                                let map_y = py.rem_euclid(size_px) as usize;
                                let tile_x = map_x / 8;
                                let tile_y = map_y / 8;
                                let map_entry = screen_base + tile_y * tiles_per_row + tile_x;

                                if map_entry < ppu.vram.len() {
                                    let tile_num = ppu.vram[map_entry] as usize;
                                    let in_tile_x = map_x % 8;
                                    let in_tile_y = map_y % 8;
                                    let pix_addr = char_base + tile_num * 64 + in_tile_y * 8 + in_tile_x;

                                    if pix_addr < ppu.vram.len() {
                                        let color_idx = ppu.vram[pix_addr] as usize;
                                        if color_idx != 0 {
                                            let pal_addr = color_idx * 2;
                                            if pal_addr + 1 < ppu.palette_ram.len() {
                                                let c = (ppu.palette_ram[pal_addr] as u16)
                                                    | ((ppu.palette_ram[pal_addr + 1] as u16) << 8);
                                                candidates[cand_count] = Pixel {
                                                    color: c,
                                                    layer: bg_idx as u8,
                                                    priority: cmd.priority,
                                                    is_transparent: false,
                                                    is_obj_alpha: false,
                                                };
                                                cand_count += 1;
                                            }
                                        }
                                    }
                                }
                            }
                        } else if cmd.kind == LayerKind::Bitmap {
                            match mode {
                                3 => {
                                    if px >= 0 && px < 240 && py >= 0 && py < 160 {
                                        let addr = (py as usize * 240 + px as usize) * 2;
                                        if addr + 1 < ppu.vram.len() {
                                            let c = (ppu.vram[addr] as u16) | ((ppu.vram[addr + 1] as u16) << 8);
                                            candidates[cand_count] = Pixel {
                                                color: c,
                                                layer: 2,
                                                priority: cmd.priority,
                                                is_transparent: false,
                                                is_obj_alpha: false,
                                            };
                                            cand_count += 1;
                                        }
                                    }
                                }
                                4 => {
                                    if px >= 0 && px < 240 && py >= 0 && py < 160 {
                                        let base = if frame_flag { 0xA000 } else { 0 };
                                        let addr = base + py as usize * 240 + px as usize;
                                        if addr < ppu.vram.len() {
                                            let color_idx = ppu.vram[addr] as usize;
                                            if color_idx != 0 {
                                                let pal_addr = color_idx * 2;
                                                if pal_addr + 1 < ppu.palette_ram.len() {
                                                    let c = (ppu.palette_ram[pal_addr] as u16)
                                                        | ((ppu.palette_ram[pal_addr + 1] as u16) << 8);
                                                    candidates[cand_count] = Pixel {
                                                        color: c,
                                                        layer: 2,
                                                        priority: cmd.priority,
                                                        is_transparent: false,
                                                        is_obj_alpha: false,
                                                    };
                                                    cand_count += 1;
                                                }
                                            }
                                        }
                                    }
                                }
                                5 => {
                                    if px >= 0 && px < 160 && py >= 0 && py < 128 {
                                        let base = if frame_flag { 0xA000 } else { 0 };
                                        let addr = base + (py as usize * 160 + px as usize) * 2;
                                        if addr + 1 < ppu.vram.len() {
                                            let c = (ppu.vram[addr] as u16) | ((ppu.vram[addr + 1] as u16) << 8);
                                            candidates[cand_count] = Pixel {
                                                color: c,
                                                layer: 2,
                                                priority: cmd.priority,
                                                is_transparent: false,
                                                is_obj_alpha: false,
                                            };
                                            cand_count += 1;
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                } else {
                    // Non-affine BG: Check for HD tile replacement first if pack is loaded
                    let mut sampled_tile: Option<u16> = None;
                    if ppu.is_hd_pack_enabled() {
                        if let Some(pack) = ppu.hd_pack() {
                            if pack.tile_count() > 0 {
                                let bgcnt = ppu.bgcnt[bg_idx];
                                let char_base = (((bgcnt >> 2) & 3) as usize) * 16384;
                                let is_8bpp = (bgcnt & (1 << 7)) != 0;
                                let screen_base = (((bgcnt >> 8) & 0x1F) as usize) * 2048;
                                let screen_size = (bgcnt >> 14) & 3;

                                let (map_w, map_h) = match screen_size {
                                    0 => (256, 256),
                                    1 => (512, 256),
                                    2 => (256, 512),
                                    3 => (512, 512),
                                    _ => (256, 256),
                                };

                                let sx_sub = ((nx as f64 + (hx % scale) as f64 / f_scale) + ppu.bghofs[bg_idx] as f64).rem_euclid(map_w as f64);
                                let sy_sub = ((ny as f64 + (hy % scale) as f64 / f_scale) + ppu.bgvofs[bg_idx] as f64).rem_euclid(map_h as f64);

                                let tile_col = (sx_sub as usize % 256) / 8;
                                let tile_row = (sy_sub as usize % 256) / 8;
                                let block_x = sx_sub as usize / 256;
                                let block_y = sy_sub as usize / 256;
                                let block_offset = match screen_size {
                                    0 => 0,
                                    1 => block_x * 2048,
                                    2 => block_y * 2048,
                                    3 => (block_y * 2 + block_x) * 2048,
                                    _ => 0,
                                };

                                let map_addr = screen_base + block_offset + (tile_row * 32 + tile_col) * 2;
                                if map_addr + 1 < ppu.vram.len() {
                                    let map_entry = (ppu.vram[map_addr] as u16) | ((ppu.vram[map_addr + 1] as u16) << 8);
                                    let tile_num = (map_entry & 0x3FF) as usize;
                                    let hflip = (map_entry & (1 << 10)) != 0;
                                    let vflip = (map_entry & (1 << 11)) != 0;
                                    let pal_num = ((map_entry >> 12) & 0x0F) as usize;

                                    let tile_addr = if is_8bpp { char_base + tile_num * 64 } else { char_base + tile_num * 32 };
                                    if tile_addr < ppu.vram.len() {
                                        let tile_hash = crate::gba::hd_pack::hash_tile(&ppu.vram[..], is_8bpp, tile_addr);
                                        let palette = crate::gba::hd_pack::extract_palette(&ppu.palette_ram[..], pal_num, is_8bpp, false);
                                        let pal_hash = crate::gba::hd_pack::hash_palette(&palette);

                                        if let Some(rep) = pack.find_tile_replacement(tile_hash, pal_hash) {
                                            let mut u = (sx_sub % 8.0) / 8.0;
                                            let mut v = (sy_sub % 8.0) / 8.0;
                                            if hflip { u = 1.0 - u; }
                                            if vflip { v = 1.0 - v; }
                                            let tx = (u.clamp(0.0, 0.99999) * rep.image.width as f64) as usize;
                                            let ty = (v.clamp(0.0, 0.99999) * rep.image.height as f64) as usize;
                                            let idx = ty * rep.image.width + tx;
                                            if idx < rep.image.pixels.len() {
                                                let mut rgba = rep.image.pixels[idx];
                                                if ((rgba >> 24) & 0xFF) > 0 {
                                                    if rep.recolor {
                                                        let base_p = rep.base_palette.as_deref().unwrap_or(&palette);
                                                        rgba = recolor_rgba(rgba, base_p, &palette);
                                                    }
                                                    let r = (rgba & 0xFF) as u8;
                                                    let g = ((rgba >> 8) & 0xFF) as u8;
                                                    let b = ((rgba >> 16) & 0xFF) as u8;
                                                    sampled_tile = Some(rgb888_to_bgr555(r, g, b));
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if let Some(c) = sampled_tile {
                        let priority = (ppu.bgcnt[bg_idx] & 3) as u8;
                        candidates[cand_count] = Pixel {
                            color: c,
                            layer: bg_idx as u8,
                            priority,
                            is_transparent: false,
                            is_obj_alpha: false,
                        };
                        cand_count += 1;
                    } else if let Some(ref lb) = ppu.layer_buffers {
                        let rgba = lb.get_layer(layer_enum)[ny * SCREEN_WIDTH + nx];
                        if (rgba & 0xFF00_0000) != 0 {
                            let r = (rgba & 0xFF) as u8;
                            let g = ((rgba >> 8) & 0xFF) as u8;
                            let b = ((rgba >> 16) & 0xFF) as u8;
                            let priority = (ppu.bgcnt[bg_idx] & 3) as u8;
                            candidates[cand_count] = Pixel {
                                color: rgb888_to_bgr555(r, g, b),
                                layer: bg_idx as u8,
                                priority,
                                is_transparent: false,
                                is_obj_alpha: false,
                            };
                            cand_count += 1;
                        }
                    }
                }
            }

            // 3. OBJ Layer
            if (ppu.dispcnt & (1 << 12)) != 0 && (ppu.layer_mask & (1 << 4)) != 0 && (win_mask & (1 << 4)) != 0 {
                let mut obj_pixel: Option<Pixel> = None;

                let x_sub = hx as f64 / f_scale;
                let _y_sub = hy as f64 / f_scale;

                // Evaluate high-resolution HD replaced sprites
                for spr in &hd_replaced_sprites {
                    let py_i32 = if spr.raw_y + (spr.orig_h as i32) > 256
                        && (ny as i32) < (spr.raw_y + (spr.orig_h as i32) - 256)
                    {
                        (ny as i32) + 256 - spr.raw_y
                    } else if (ny as i32) >= spr.raw_y && (ny as i32) < spr.raw_y + (spr.orig_h as i32) {
                        (ny as i32) - spr.raw_y
                    } else {
                        continue;
                    };

                    let rel_x = x_sub - spr.sprite_x as f64;
                    if rel_x < 0.0 || rel_x >= spr.orig_w as f64 {
                        continue;
                    }

                    let mut u = rel_x / spr.orig_w as f64;
                    let mut v = (py_i32 as f64 + (sy as f64 / f_scale)) / spr.orig_h as f64;

                    let do_hflip = spr.replacement.hflip.unwrap_or(spr.hflip);
                    let do_vflip = spr.replacement.vflip.unwrap_or(spr.vflip);

                    if do_hflip {
                        u = 1.0 - u;
                    }
                    if do_vflip {
                        v = 1.0 - v;
                    }

                    let u_clamped = u.clamp(0.0, 0.99999);
                    let v_clamped = v.clamp(0.0, 0.99999);

                    let tx = (u_clamped * spr.replacement.image.width as f64) as usize;
                    let ty = (v_clamped * spr.replacement.image.height as f64) as usize;

                    let idx = ty * spr.replacement.image.width + tx;
                    if idx < spr.replacement.image.pixels.len() {
                        let mut rgba = spr.replacement.image.pixels[idx];
                        let alpha = (rgba >> 24) & 0xFF;
                        if alpha > 0 {
                            if spr.replacement.recolor {
                                let base_p = spr.replacement.base_palette.as_deref().unwrap_or(&spr.palette);
                                rgba = recolor_rgba(rgba, base_p, &spr.palette);
                            }
                            let r = (rgba & 0xFF) as u8;
                            let g = ((rgba >> 8) & 0xFF) as u8;
                            let b = ((rgba >> 16) & 0xFF) as u8;
                            let c = rgb888_to_bgr555(r, g, b);

                            if obj_pixel.as_ref().map_or(true, |prev| spr.priority <= prev.priority) {
                                obj_pixel = Some(Pixel {
                                    color: c,
                                    layer: 4,
                                    priority: spr.priority,
                                    is_transparent: false,
                                    is_obj_alpha: spr.is_semi_trans,
                                });
                            }
                        }
                    }
                }

                // If no replaced sprite was hit, evaluate high-resolution affine sprites
                if obj_pixel.is_none() {
                    for spr in &affine_sprites {
                        let half_bw = spr.bound_w as f64 / 2.0;
                        let half_bh = spr.bound_h as f64 / 2.0;
                        let half_ow = spr.orig_w as f64 / 2.0;
                        let half_oh = spr.orig_h as f64 / 2.0;

                        let py_i32 = if spr.raw_y + (spr.bound_h as i32) > 256
                            && (ny as i32) < (spr.raw_y + (spr.bound_h as i32) - 256)
                        {
                            (ny as i32) + 256 - spr.raw_y
                        } else if (ny as i32) >= spr.raw_y && (ny as i32) < spr.raw_y + (spr.bound_h as i32) {
                            (ny as i32) - spr.raw_y
                        } else {
                            continue;
                        };

                        let rel_x = x_sub - spr.sprite_x as f64;
                        if rel_x < 0.0 || rel_x >= spr.bound_w as f64 {
                            continue;
                        }

                        let hbx = rel_x - half_bw;
                        let hby = (py_i32 as f64 + (sy as f64 / f_scale)) - half_bh;

                        let tex_x = (spr.pa as f64 * hbx + spr.pb as f64 * hby) / 256.0 + half_ow;
                        let tex_y = (spr.pc as f64 * hbx + spr.pd as f64 * hby) / 256.0 + half_oh;

                        if tex_x >= 0.0 && tex_x < spr.orig_w as f64 && tex_y >= 0.0 && tex_y < spr.orig_h as f64 {
                            let px = tex_x as usize;
                            let py = tex_y as usize;

                            let in_tile_x = px % 8;
                            let in_tile_y = py % 8;
                            let tile_x = px / 8;
                            let tile_y = py / 8;

                            let tile_offset = if mapping_1d {
                                let tiles_per_row = spr.orig_w / 8;
                                if spr.is_8bpp {
                                    (spr.raw_tile + (tile_y * tiles_per_row + tile_x) * 2) & 0x3FF
                                } else {
                                    (spr.raw_tile + tile_y * tiles_per_row + tile_x) & 0x3FF
                                }
                            } else {
                                if spr.is_8bpp {
                                    let tile_col = ((spr.raw_tile & 0x1F) + tile_x * 2) & 0x1F;
                                    let tile_row = (((spr.raw_tile >> 5) & 0x1F) + tile_y) & 0x1F;
                                    (tile_row * 32 + tile_col) & 0x3FF
                                } else {
                                    let tile_col = ((spr.raw_tile & 0x1F) + tile_x) & 0x1F;
                                    let tile_row = (((spr.raw_tile >> 5) & 0x1F) + tile_y) & 0x1F;
                                    (tile_row * 32 + tile_col) & 0x3FF
                                }
                            };

                            let (color_idx, is_trans) = if spr.is_8bpp {
                                let tile_addr = obj_char_base + tile_offset * 32 + in_tile_y * 8 + in_tile_x;
                                if tile_addr < ppu.vram.len() {
                                    let idx = ppu.vram[tile_addr];
                                    (idx as usize, idx == 0)
                                } else {
                                    (0, true)
                                }
                            } else {
                                let tile_addr = obj_char_base + tile_offset * 32 + in_tile_y * 4 + (in_tile_x / 2);
                                if tile_addr < ppu.vram.len() {
                                    let byte = ppu.vram[tile_addr];
                                    let idx = if in_tile_x % 2 == 0 { byte & 0x0F } else { (byte >> 4) & 0x0F };
                                    (spr.pal_num * 16 + (idx as usize), idx == 0)
                                } else {
                                    (0, true)
                                }
                            };

                            if !is_trans {
                                let pal_addr = 0x200 + color_idx * 2;
                                if pal_addr + 1 < ppu.palette_ram.len() {
                                    let c = (ppu.palette_ram[pal_addr] as u16)
                                        | ((ppu.palette_ram[pal_addr + 1] as u16) << 8);
                                    if obj_pixel.as_ref().map_or(true, |prev| spr.priority <= prev.priority) {
                                        obj_pixel = Some(Pixel {
                                            color: c,
                                            layer: 4,
                                            priority: spr.priority,
                                            is_transparent: false,
                                            is_obj_alpha: spr.is_semi_trans,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }

                // If no affine sprite was hit, check native non-affine sprite buffer
                if obj_pixel.is_none() {
                    if let Some(ref lb) = ppu.layer_buffers {
                        let rgba = lb.get_layer(PpuLayer::Obj)[ny * SCREEN_WIDTH + nx];
                        if (rgba & 0xFF00_0000) != 0 {
                            let r = (rgba & 0xFF) as u8;
                            let g = ((rgba >> 8) & 0xFF) as u8;
                            let b = ((rgba >> 16) & 0xFF) as u8;
                            obj_pixel = Some(Pixel {
                                color: rgb888_to_bgr555(r, g, b),
                                layer: 4,
                                priority: 0,
                                is_transparent: false,
                                is_obj_alpha: false,
                            });
                        }
                    }
                }

                if let Some(p) = obj_pixel {
                    candidates[cand_count] = p;
                    cand_count += 1;
                }
            }

            // 4. Priority Resolution and Compositing
            // Sort active candidates by (priority ASC, is_bg ASC, layer_idx ASC)
            // On GBA: priority 0 is top; when priority is equal, OBJ appears above BG;
            // among BGs, BG0 > BG1 > BG2 > BG3.
            let active_slice = &mut candidates[..cand_count];
            active_slice.sort_by_key(|p| {
                let is_bg = if p.layer == 4 { 0 } else { 1 };
                (p.priority, is_bg, p.layer)
            });

            let top = active_slice[0];
            let bot = if active_slice.len() > 1 {
                active_slice[1]
            } else {
                Pixel::default()
            };

            let final_bgr = if (win_mask & (1 << 5)) != 0 {
                apply_color_effects(top, bot, ppu.bldcnt, eva, evb, evy)
            } else {
                top.color
            };

            let (r, g, b) = bgr555_to_rgb888(final_bgr);
            frame.pixels[row_offset + hx] = 0xFF00_0000 | ((b as u32) << 16) | ((g as u32) << 8) | (r as u32);
        }
    }

    Some(frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hd_scale_factors_and_names() {
        assert_eq!(HdScale::Off.factor(), 1);
        assert_eq!(HdScale::X2.factor(), 2);
        assert_eq!(HdScale::X4.factor(), 4);
        assert_eq!(HdScale::X8.factor(), 8);

        for scale in HdScale::ALL {
            assert!(!scale.display_name().is_empty());
        }
    }

    #[test]
    fn test_hd_frame_downsampling_ssaa() {
        let mut hd = HdFrame::new(4);
        assert_eq!(hd.width, 960);
        assert_eq!(hd.height, 640);
        assert_eq!(hd.pixels.len(), 960 * 640);

        // Fill with white
        hd.pixels.fill(0xFFFF_FFFF);
        let downsampled = hd.downsample_ssaa();
        assert_eq!(downsampled[0], 0xFFFF_FFFF);
        assert_eq!(downsampled[SCREEN_WIDTH * SCREEN_HEIGHT - 1], 0xFFFF_FFFF);
    }
}
