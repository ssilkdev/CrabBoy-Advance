//! GBA PPU Diorama Subsystem: Layer & Metadata Extraction
//! Separates Background tiles, Window regions, and OAM Sprites into structured 3D entities.

use super::blend::{bgr555_to_rgb888, Pixel};
use super::obj::get_sprite_size;
use super::{SCREEN_HEIGHT, SCREEN_WIDTH};

/// An extracted individual 3D Sprite with screen position, dimensions, priority, and RGBA pixel buffer.
#[derive(Clone, Debug)]
pub struct Sprite3D {
    pub id: usize,
    pub x: i32,
    pub y: i32,
    pub width: usize,
    pub height: usize,
    pub priority: u8,
    pub is_affine: bool,
    pub is_semi_transparent: bool,
    pub pixels: Vec<u32>, // width * height, format: 0xAABBGGRR (RGBA in little-endian)
}

/// An extracted 2D plane / layer (BG0..3 or Window) with priority, scroll metrics, and RGBA pixel buffer.
#[derive(Clone, Debug)]
pub struct Layer3D {
    pub id: u8,
    pub enabled: bool,
    pub priority: u8,
    pub scroll_x: u16,
    pub scroll_y: u16,
    pub pixels: Box<[u32; SCREEN_WIDTH * SCREEN_HEIGHT]>,
}

impl Layer3D {
    pub fn new(id: u8) -> Self {
        Self {
            id,
            enabled: false,
            priority: id.min(3),
            scroll_x: 0,
            scroll_y: 0,
            pixels: Box::new([0; SCREEN_WIDTH * SCREEN_HEIGHT]),
        }
    }

    pub fn clear(&mut self) {
        self.pixels.fill(0);
        self.enabled = false;
        self.scroll_x = 0;
        self.scroll_y = 0;
    }
}

/// Aggregates all structured render layers, individual OAM sprites, and backdrop for 3D diorama display.
#[derive(Clone, Debug)]
pub struct DioramaFrameData {
    pub bg_layers: [Layer3D; 4],
    pub window_enabled: bool,
    pub win0_bounds: Option<[u16; 4]>, // [x1, y1, x2, y2]
    pub win1_bounds: Option<[u16; 4]>,
    pub backdrop_color: [u8; 4], // RGBA
    pub sprites: Vec<Sprite3D>,
    pub obj_composite: Box<[u32; SCREEN_WIDTH * SCREEN_HEIGHT]>,
}

impl Default for DioramaFrameData {
    fn default() -> Self {
        Self::new()
    }
}

impl DioramaFrameData {
    pub fn new() -> Self {
        Self {
            bg_layers: [
                Layer3D::new(0),
                Layer3D::new(1),
                Layer3D::new(2),
                Layer3D::new(3),
            ],
            window_enabled: false,
            win0_bounds: None,
            win1_bounds: None,
            backdrop_color: [0, 0, 0, 255],
            sprites: Vec::with_capacity(128),
            obj_composite: Box::new([0; SCREEN_WIDTH * SCREEN_HEIGHT]),
        }
    }

    pub fn clear(&mut self) {
        for bg in &mut self.bg_layers {
            bg.clear();
        }
        self.window_enabled = false;
        self.win0_bounds = None;
        self.win1_bounds = None;
        self.sprites.clear();
        self.obj_composite.fill(0);
    }

    /// Records isolated scanline pixel buffers during PPU scanline rendering.
    #[inline]
    pub fn record_scanline(
        &mut self,
        y: u32,
        bg_layer_bufs: &[[Pixel; SCREEN_WIDTH]; 4],
        obj_buf: &[Pixel; SCREEN_WIDTH],
    ) {
        let row = (y as usize) * SCREEN_WIDTH;

        // Record BG layers
        for i in 0..4 {
            let dst_row = &mut self.bg_layers[i].pixels[row..row + SCREEN_WIDTH];
            for (x, &px) in bg_layer_bufs[i].iter().enumerate() {
                if !px.is_transparent {
                    let (r, g, b) = bgr555_to_rgb888(px.color);
                    dst_row[x] = 0xFF00_0000 | ((b as u32) << 16) | ((g as u32) << 8) | (r as u32);
                } else {
                    dst_row[x] = 0;
                }
            }
        }

        // Record composite OBJ scanline
        let dst_obj = &mut self.obj_composite[row..row + SCREEN_WIDTH];
        for (x, &px) in obj_buf.iter().enumerate() {
            if !px.is_transparent {
                let (r, g, b) = bgr555_to_rgb888(px.color);
                dst_obj[x] = 0xFF00_0000 | ((b as u32) << 16) | ((g as u32) << 8) | (r as u32);
            } else {
                dst_obj[x] = 0;
            }
        }
    }

    /// Finalizes layer metadata and extracts individual 3D OAM sprites at VBlank.
    pub fn finalize_frame(
        &mut self,
        dispcnt: u16,
        bgcnt: &[u16; 4],
        bghofs: &[u16; 4],
        bgvofs: &[u16; 4],
        win0h: u16,
        win0v: u16,
        win1h: u16,
        win1v: u16,
        palette_ram: &[u8],
        oam: &[u8],
        vram: &[u8],
    ) {
        // Extract backdrop color (palette entry 0)
        let bg_color = (palette_ram[0] as u16) | ((palette_ram[1] as u16) << 8);
        let (r, g, b) = bgr555_to_rgb888(bg_color);
        self.backdrop_color = [r, g, b, 255];

        // Update BG layer metadata
        for i in 0..4 {
            let is_enabled = (dispcnt & (1 << (8 + i))) != 0;
            self.bg_layers[i].enabled = is_enabled;
            self.bg_layers[i].priority = (bgcnt[i] & 3) as u8;
            self.bg_layers[i].scroll_x = bghofs[i];
            self.bg_layers[i].scroll_y = bgvofs[i];
        }

        // Update Window configuration
        let win0_enable = (dispcnt & (1 << 13)) != 0;
        let win1_enable = (dispcnt & (1 << 14)) != 0;
        let objwin_enable = (dispcnt & (1 << 15)) != 0;
        self.window_enabled = win0_enable || win1_enable || objwin_enable;

        self.win0_bounds = if win0_enable {
            Some([
                (win0h >> 8) & 0xFF,
                (win0v >> 8) & 0xFF,
                win0h & 0xFF,
                win0v & 0xFF,
            ])
        } else {
            None
        };

        self.win1_bounds = if win1_enable {
            Some([
                (win1h >> 8) & 0xFF,
                (win1v >> 8) & 0xFF,
                win1h & 0xFF,
                win1v & 0xFF,
            ])
        } else {
            None
        };

        // Extract individual OAM sprites
        self.sprites = extract_sprites_from_oam(dispcnt, oam, vram, palette_ram);
    }
}

/// Decodes all active, on-screen OAM sprites into independent 3D entities.
pub fn extract_sprites_from_oam(
    dispcnt: u16,
    oam: &[u8],
    vram: &[u8],
    palette_ram: &[u8],
) -> Vec<Sprite3D> {
    if (dispcnt & (1 << 12)) == 0 {
        return Vec::new();
    }

    let mapping_1d = (dispcnt & (1 << 6)) != 0;
    let obj_char_base = 0x10000; // 64 KB VRAM offset
    let mode = (dispcnt & 7) as u8;

    let mut sprites = Vec::with_capacity(64);

    for i in 0..128 {
        let oam_addr = i * 8;
        if oam_addr + 6 > oam.len() {
            continue;
        }

        let attr0 = (oam[oam_addr] as u16) | ((oam[oam_addr + 1] as u16) << 8);
        let attr1 = (oam[oam_addr + 2] as u16) | ((oam[oam_addr + 3] as u16) << 8);
        let attr2 = (oam[oam_addr + 4] as u16) | ((oam[oam_addr + 5] as u16) << 8);

        let is_affine = (attr0 & (1 << 8)) != 0;
        let is_disabled = !is_affine && ((attr0 & (1 << 9)) != 0);
        if is_disabled {
            continue;
        }

        let is_double_size = is_affine && ((attr0 & (1 << 9)) != 0);
        let obj_mode = (attr0 >> 10) & 3;
        if obj_mode == 2 {
            // OBJ window, skip from regular sprite rendering
            continue;
        }
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
        let sprite_y = if raw_y >= 192 { raw_y - 256 } else { raw_y };

        let raw_x = (attr1 & 0x1FF) as i32;
        let sprite_x = if raw_x >= 256 { raw_x - 512 } else { raw_x };

        // Offscreen culling
        if sprite_x >= SCREEN_WIDTH as i32
            || sprite_x + (bound_w as i32) <= 0
            || sprite_y >= SCREEN_HEIGHT as i32
            || sprite_y + (bound_h as i32) <= 0
        {
            continue;
        }

        let raw_tile = (attr2 & 0x3FF) as usize;
        if mode >= 3 && raw_tile < 512 {
            continue;
        }
        let tile_base = if is_8bpp && !mapping_1d {
            raw_tile & !1
        } else {
            raw_tile
        };

        let priority = ((attr2 >> 10) & 3) as u8;
        let pal_num = ((attr2 >> 12) & 0x0F) as usize;

        let mut sprite_pixels = vec![0u32; bound_w * bound_h];
        let mut has_visible_pixels = false;

        if !is_affine {
            let hflip = (attr1 & (1 << 12)) != 0;
            let vflip = (attr1 & (1 << 13)) != 0;

            for py_idx in 0..orig_h {
                let py = if vflip { orig_h - 1 - py_idx } else { py_idx };
                for px_idx in 0..orig_w {
                    let px = if hflip { orig_w - 1 - px_idx } else { px_idx };

                    let tile_x = px / 8;
                    let tile_y = py / 8;
                    let in_tile_x = px % 8;
                    let in_tile_y = py % 8;

                    let tile_offset = if mapping_1d {
                        let tiles_per_row = orig_w / 8;
                        if is_8bpp {
                            (tile_base + (tile_y * tiles_per_row + tile_x) * 2) & 0x3FF
                        } else {
                            (tile_base + tile_y * tiles_per_row + tile_x) & 0x3FF
                        }
                    } else if is_8bpp {
                        let tile_col = ((tile_base & 0x1F) + tile_x * 2) & 0x1F;
                        let tile_row = (((tile_base >> 5) & 0x1F) + tile_y) & 0x1F;
                        (tile_row * 32 + tile_col) & 0x3FF
                    } else {
                        let tile_col = ((tile_base & 0x1F) + tile_x) & 0x1F;
                        let tile_row = (((tile_base >> 5) & 0x1F) + tile_y) & 0x1F;
                        (tile_row * 32 + tile_col) & 0x3FF
                    };

                    let (color_idx, is_trans) = if is_8bpp {
                        let tile_addr = obj_char_base + tile_offset * 32 + in_tile_y * 8 + in_tile_x;
                        if tile_addr < vram.len() {
                            let idx = vram[tile_addr];
                            (idx as usize, idx == 0)
                        } else {
                            (0, true)
                        }
                    } else {
                        let tile_addr = obj_char_base + tile_offset * 32 + in_tile_y * 4 + (in_tile_x / 2);
                        if tile_addr < vram.len() {
                            let byte = vram[tile_addr];
                            let idx = if in_tile_x % 2 == 0 { byte & 0x0F } else { (byte >> 4) & 0x0F };
                            (pal_num * 16 + (idx as usize), idx == 0)
                        } else {
                            (0, true)
                        }
                    };

                    if !is_trans {
                        let pal_addr = 0x200 + color_idx * 2;
                        if pal_addr + 1 < palette_ram.len() {
                            let color = (palette_ram[pal_addr] as u16) | ((palette_ram[pal_addr + 1] as u16) << 8);
                            let (r, g, b) = bgr555_to_rgb888(color);
                            sprite_pixels[py_idx * orig_w + px_idx] =
                                0xFF00_0000 | ((b as u32) << 16) | ((g as u32) << 8) | (r as u32);
                            has_visible_pixels = true;
                        }
                    }
                }
            }
        } else {
            // Affine sprite extraction
            let affine_group = ((attr1 >> 9) & 0x1F) as usize;
            let group_base = affine_group * 32;
            let read_param = |idx: usize| -> i16 {
                let off = group_base + idx * 8 + 6;
                if off + 1 < oam.len() {
                    (oam[off] as i16) | ((oam[off + 1] as i16) << 8)
                } else {
                    0
                }
            };

            let pa = read_param(0);
            let pb = read_param(1);
            let pc = read_param(2);
            let pd = read_param(3);

            let half_bound_w = bound_w as i32 / 2;
            let half_bound_h = bound_h as i32 / 2;
            let half_orig_w = orig_w as i32 / 2;
            let half_orig_h = orig_h as i32 / 2;

            for by in 0..bound_h {
                let iy = (by as i32) - half_bound_h;
                for bx in 0..bound_w {
                    let ix = (bx as i32) - half_bound_w;
                    let tex_x = ((pa as i32 * ix + pb as i32 * iy) >> 8) + half_orig_w;
                    let tex_y = ((pc as i32 * ix + pd as i32 * iy) >> 8) + half_orig_h;

                    if tex_x < 0 || tex_x >= orig_w as i32 || tex_y < 0 || tex_y >= orig_h as i32 {
                        continue;
                    }

                    let px = tex_x as usize;
                    let py = tex_y as usize;

                    let tile_x = px / 8;
                    let tile_y = py / 8;
                    let in_tile_x = px % 8;
                    let in_tile_y = py % 8;

                    let tile_offset = if mapping_1d {
                        let tiles_per_row = orig_w / 8;
                        if is_8bpp {
                            (tile_base + (tile_y * tiles_per_row + tile_x) * 2) & 0x3FF
                        } else {
                            (tile_base + tile_y * tiles_per_row + tile_x) & 0x3FF
                        }
                    } else if is_8bpp {
                        let tile_col = ((tile_base & 0x1F) + tile_x * 2) & 0x1F;
                        let tile_row = (((tile_base >> 5) & 0x1F) + tile_y) & 0x1F;
                        (tile_row * 32 + tile_col) & 0x3FF
                    } else {
                        let tile_col = ((tile_base & 0x1F) + tile_x) & 0x1F;
                        let tile_row = (((tile_base >> 5) & 0x1F) + tile_y) & 0x1F;
                        (tile_row * 32 + tile_col) & 0x3FF
                    };

                    let (color_idx, is_trans) = if is_8bpp {
                        let tile_addr = obj_char_base + tile_offset * 32 + in_tile_y * 8 + in_tile_x;
                        if tile_addr < vram.len() {
                            let idx = vram[tile_addr];
                            (idx as usize, idx == 0)
                        } else {
                            (0, true)
                        }
                    } else {
                        let tile_addr = obj_char_base + tile_offset * 32 + in_tile_y * 4 + (in_tile_x / 2);
                        if tile_addr < vram.len() {
                            let byte = vram[tile_addr];
                            let idx = if in_tile_x.is_multiple_of(2) { byte & 0x0F } else { (byte >> 4) & 0x0F };
                            (pal_num * 16 + (idx as usize), idx == 0)
                        } else {
                            (0, true)
                        }
                    };

                    if !is_trans {
                        let pal_addr = 0x200 + color_idx * 2;
                        if pal_addr + 1 < palette_ram.len() {
                            let color = (palette_ram[pal_addr] as u16) | ((palette_ram[pal_addr + 1] as u16) << 8);
                            let (r, g, b) = bgr555_to_rgb888(color);
                            sprite_pixels[by * bound_w + bx] =
                                0xFF00_0000 | ((b as u32) << 16) | ((g as u32) << 8) | (r as u32);
                            has_visible_pixels = true;
                        }
                    }
                }
            }
        }

        if has_visible_pixels {
            sprites.push(Sprite3D {
                id: i,
                x: sprite_x,
                y: sprite_y,
                width: bound_w,
                height: bound_h,
                priority,
                is_affine,
                is_semi_transparent: is_semi_trans,
                pixels: sprite_pixels,
            });
        }
    }

    sprites
}
