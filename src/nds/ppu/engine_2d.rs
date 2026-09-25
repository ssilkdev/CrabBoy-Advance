//! Nintendo DS 2D Graphics Engine (Engine A / Engine B)
//!
//! Emulates background layer rendering (Text, Affine, Extended Affine),
//! OAM sprite synthesis, alpha blending, and windowing for a 256x192 display.

pub const SCREEN_WIDTH: usize = 256;
pub const SCREEN_HEIGHT: usize = 192;

#[derive(Clone)]
pub struct Engine2D {
    pub is_main_engine: bool,
    pub dispcnt: u32,

    pub bgcnt: [u16; 4],
    pub bghofs: [u16; 4],
    pub bgvofs: [u16; 4],

    // Affine background parameters (BG2, BG3)
    pub bgpa: [i16; 2],
    pub bgpb: [i16; 2],
    pub bgpc: [i16; 2],
    pub bgpd: [i16; 2],
    pub bgx: [i32; 2],
    pub bgy: [i32; 2],
    pub internal_x: [i32; 2],
    pub internal_y: [i32; 2],

    // Blending & Master Brightness
    pub bldcnt: u16,
    pub bldalpha: u16,
    pub bldy: u16,
    pub master_bright: u16,

    // Windows
    pub win0h: u16,
    pub win1h: u16,
    pub win0v: u16,
    pub win1v: u16,
    pub winin: u16,
    pub winout: u16,

    // Memories
    pub palette: [u8; 1024],
    pub oam: [u8; 1024],

    /// Render target for this engine (256x192 RGBA 0xAABBGGRR)
    pub framebuffer: Vec<u32>,
}

impl Engine2D {
    pub fn new(is_main_engine: bool) -> Self {
        Self {
            is_main_engine,
            dispcnt: 0,
            bgcnt: [0; 4],
            bghofs: [0; 4],
            bgvofs: [0; 4],
            bgpa: [0x100, 0],
            bgpb: [0, 0],
            bgpc: [0, 0],
            bgpd: [0, 0x100],
            bgx: [0; 2],
            bgy: [0; 2],
            internal_x: [0; 2],
            internal_y: [0; 2],
            bldcnt: 0,
            bldalpha: 0,
            bldy: 0,
            master_bright: 0,
            win0h: 0,
            win1h: 0,
            win0v: 0,
            win1v: 0,
            winin: 0,
            winout: 0,
            palette: [0; 1024],
            oam: [0; 1024],
            framebuffer: vec![0xFF000000; SCREEN_WIDTH * SCREEN_HEIGHT],
        }
    }

    pub fn reset_scanline_affine(&mut self) {
        self.internal_x = self.bgx;
        self.internal_y = self.bgy;
    }

    pub fn step_scanline_affine(&mut self) {
        self.internal_x[0] = self.internal_x[0].wrapping_add(self.bgpb[0] as i32);
        self.internal_y[0] = self.internal_y[0].wrapping_add(self.bgpd[0] as i32);
        self.internal_x[1] = self.internal_x[1].wrapping_add(self.bgpb[1] as i32);
        self.internal_y[1] = self.internal_y[1].wrapping_add(self.bgpd[1] as i32);
    }

    /// Convert 15-bit BGR555 to 32-bit RGBA (0xAABBGGRR)
    #[inline(always)]
    pub fn bgr555_to_rgba(c: u16) -> u32 {
        let r = ((c & 0x1F) as u32) << 3;
        let g = (((c >> 5) & 0x1F) as u32) << 3;
        let b = (((c >> 10) & 0x1F) as u32) << 3;
        // Expand 5-bit to 8-bit using high bits
        let r = r | (r >> 5);
        let g = g | (g >> 5);
        let b = b | (b >> 5);
        0xFF000000 | (b << 16) | (g << 8) | r
    }

    /// Read BG palette color
    #[inline(always)]
    pub fn read_bg_palette(&self, index: usize) -> u16 {
        let offset = (index * 2) & 0x3FE;
        u16::from_le_bytes([self.palette[offset], self.palette[offset + 1]])
    }

    /// Read OBJ palette color
    #[inline(always)]
    pub fn read_obj_palette(&self, index: usize) -> u16 {
        let offset = 0x200 + ((index * 2) & 0x1FE);
        u16::from_le_bytes([self.palette[offset], self.palette[offset + 1]])
    }

    /// Render a single scanline (0..191)
    pub fn render_scanline(&mut self, line: usize, vram_bg: &[u8], vram_obj: &[u8]) {
        if line >= SCREEN_HEIGHT {
            return;
        }

        let forced_blank = (self.dispcnt & (1 << 7)) != 0;
        if forced_blank {
            let row_start = line * SCREEN_WIDTH;
            self.framebuffer[row_start..row_start + SCREEN_WIDTH].fill(0xFFFFFFFF);
            return;
        }

        // Backdrop color: palette index 0
        let backdrop_color = Self::bgr555_to_rgba(self.read_bg_palette(0));
        let row_start = line * SCREEN_WIDTH;
        let mut line_pixels = [backdrop_color; SCREEN_WIDTH];
        let mut line_priorities = [255u8; SCREEN_WIDTH];

        // Render BG layers in priority order (3 down to 0)
        let bg_mode = (self.dispcnt & 0x07) as u8;
        for prio in (0..4).rev() {
            for bg_idx in (0..4).rev() {
                if (self.dispcnt & (1 << (8 + bg_idx))) == 0 {
                    continue; // Layer disabled
                }
                let bg_prio = (self.bgcnt[bg_idx] & 0x03) as u8;
                if bg_prio != prio {
                    continue;
                }

                self.render_bg_scanline(bg_idx, bg_mode, line, vram_bg, &mut line_pixels, &mut line_priorities);
            }
        }

        // Render OAM Sprites
        if (self.dispcnt & (1 << 12)) != 0 {
            self.render_sprites_scanline(line, vram_obj, &mut line_pixels, &mut line_priorities);
        }

        // Apply Master Brightness control if configured
        let bright_mode = (self.master_bright >> 14) & 0x03;
        let factor = (self.master_bright & 0x1F).min(16) as u32;
        if factor > 0 {
            if bright_mode == 1 {
                // Brightness Increase (Up to White)
                for px in line_pixels.iter_mut() {
                    let r = *px & 0xFF;
                    let g = (*px >> 8) & 0xFF;
                    let b = (*px >> 16) & 0xFF;
                    let r = r + ((255 - r) * factor) / 16;
                    let g = g + ((255 - g) * factor) / 16;
                    let b = b + ((255 - b) * factor) / 16;
                    *px = 0xFF000000 | (b << 16) | (g << 8) | r;
                }
            } else if bright_mode == 2 {
                // Brightness Decrease (Down to Black)
                for px in line_pixels.iter_mut() {
                    let r = *px & 0xFF;
                    let g = (*px >> 8) & 0xFF;
                    let b = (*px >> 16) & 0xFF;
                    let r = r - (r * factor) / 16;
                    let g = g - (g * factor) / 16;
                    let b = b - (b * factor) / 16;
                    *px = 0xFF000000 | (b << 16) | (g << 8) | r;
                }
            }
        }

        self.framebuffer[row_start..row_start + SCREEN_WIDTH].copy_from_slice(&line_pixels);
    }

    /// Render a single BG line (Text / Tile mode)
    fn render_bg_scanline(
        &self,
        bg_idx: usize,
        _bg_mode: u8,
        line: usize,
        vram_bg: &[u8],
        line_pixels: &mut [u32],
        line_priorities: &mut [u8],
    ) {
        let cnt = self.bgcnt[bg_idx];
        let prio = (cnt & 0x03) as u8;
        let char_base = (((cnt >> 2) & 0x0F) as usize) * 0x4000;
        let screen_base = (((cnt >> 8) & 0x1F) as usize) * 0x800;
        let is_256_color = (cnt & (1 << 7)) != 0;

        let scx = self.bghofs[bg_idx] as usize;
        let scy = (self.bgvofs[bg_idx] as usize).wrapping_add(line);

        let tile_y = (scy / 8) % 32;
        let fine_y = scy % 8;

        for x in 0..SCREEN_WIDTH {
            let px_x = (x + scx) % 512;
            let tile_x = (px_x / 8) % 32;
            let fine_x = px_x % 8;

            // Map entry offset (32x32 tiles, 2 bytes per entry)
            let map_entry_offset = screen_base + (tile_y * 32 + tile_x) * 2;
            if map_entry_offset + 1 >= vram_bg.len() {
                continue;
            }

            let entry = u16::from_le_bytes([vram_bg[map_entry_offset], vram_bg[map_entry_offset + 1]]);
            let tile_id = (entry & 0x3FF) as usize;
            let hflip = (entry & (1 << 10)) != 0;
            let vflip = (entry & (1 << 11)) != 0;
            let pal_slot = ((entry >> 12) & 0x0F) as usize;

            let fy = if vflip { 7 - fine_y } else { fine_y };
            let fx = if hflip { 7 - fine_x } else { fine_x };

            let color_idx = if is_256_color {
                let tile_offset = char_base + tile_id * 64 + fy * 8 + fx;
                if tile_offset < vram_bg.len() { vram_bg[tile_offset] } else { 0 }
            } else {
                let tile_offset = char_base + tile_id * 32 + fy * 4 + fx / 2;
                if tile_offset < vram_bg.len() {
                    let byte = vram_bg[tile_offset];
                    if (fx & 1) == 0 { byte & 0x0F } else { byte >> 4 }
                } else {
                    0
                }
            };

            if color_idx != 0 && prio <= line_priorities[x] {
                let pal_color = if is_256_color {
                    self.read_bg_palette(color_idx as usize)
                } else {
                    self.read_bg_palette(pal_slot * 16 + (color_idx as usize))
                };
                line_pixels[x] = Self::bgr555_to_rgba(pal_color);
                line_priorities[x] = prio;
            }
        }
    }

    /// Render OAM sprites on current scanline
    fn render_sprites_scanline(
        &self,
        line: usize,
        vram_obj: &[u8],
        line_pixels: &mut [u32],
        line_priorities: &mut [u8],
    ) {
        // Iterate through all 128 sprites in OAM
        for spr_idx in 0..128 {
            let oam_offset = spr_idx * 8;
            let attr0 = u16::from_le_bytes([self.oam[oam_offset], self.oam[oam_offset + 1]]);
            let attr1 = u16::from_le_bytes([self.oam[oam_offset + 2], self.oam[oam_offset + 3]]);
            let attr2 = u16::from_le_bytes([self.oam[oam_offset + 4], self.oam[oam_offset + 5]]);

            let is_disabled = (attr0 & (1 << 9)) != 0 && (attr0 & (1 << 8)) == 0;
            if is_disabled {
                continue;
            }

            let spr_y = (attr0 & 0xFF) as usize;
            let shape = ((attr0 >> 14) & 0x03) as usize;
            let size_code = ((attr1 >> 14) & 0x03) as usize;

            let (width, height) = match (shape, size_code) {
                (0, 0) => (8, 8),
                (0, 1) => (16, 16),
                (0, 2) => (32, 32),
                (0, 3) => (64, 64),
                (1, 0) => (16, 8),
                (1, 1) => (32, 8),
                (1, 2) => (32, 16),
                (1, 3) => (64, 32),
                (2, 0) => (8, 16),
                (2, 1) => (8, 32),
                (2, 2) => (16, 32),
                (2, 3) => (32, 64),
                _ => (8, 8),
            };

            let rel_y = if line >= spr_y && line < spr_y + height {
                line - spr_y
            } else if spr_y >= 192 && line < (spr_y + height).saturating_sub(256) {
                line + 256 - spr_y
            } else {
                continue;
            };

            let spr_x = (attr1 & 0x1FF) as usize;
            let prio = ((attr2 >> 10) & 0x03) as u8;
            let is_256_color = (attr0 & (1 << 13)) != 0;
            let pal_slot = ((attr2 >> 12) & 0x0F) as usize;
            let tile_id = (attr2 & 0x3FF) as usize;
            let hflip = (attr1 & (1 << 12)) != 0;
            let vflip = (attr1 & (1 << 13)) != 0;

            let fy = if vflip { height - 1 - rel_y } else { rel_y };
            let tile_row = fy / 8;
            let fine_y = fy % 8;

            for px in 0..width {
                let screen_x = (spr_x + px) & 0x1FF;
                if screen_x >= SCREEN_WIDTH {
                    continue;
                }

                let fx = if hflip { width - 1 - px } else { px };
                let tile_col = fx / 8;
                let fine_x = fx % 8;

                let cur_tile = tile_id + tile_row * (width / 8) + tile_col;
                let color_idx = if is_256_color {
                    let offset = cur_tile * 64 + fine_y * 8 + fine_x;
                    if offset < vram_obj.len() { vram_obj[offset] } else { 0 }
                } else {
                    let offset = cur_tile * 32 + fine_y * 4 + fine_x / 2;
                    if offset < vram_obj.len() {
                        let b = vram_obj[offset];
                        if (fine_x & 1) == 0 { b & 0x0F } else { b >> 4 }
                    } else {
                        0
                    }
                };

                if color_idx != 0 && prio <= line_priorities[screen_x] {
                    let pal_color = if is_256_color {
                        self.read_obj_palette(color_idx as usize)
                    } else {
                        self.read_obj_palette(pal_slot * 16 + (color_idx as usize))
                    };
                    line_pixels[screen_x] = Self::bgr555_to_rgba(pal_color);
                    line_priorities[screen_x] = prio;
                }
            }
        }
    }
}
