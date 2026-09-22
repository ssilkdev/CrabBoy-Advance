//! Game Boy / Game Boy Color PPU.
//!
//! Scanline-based renderer: the mode state machine is cycle-driven (so STAT
//! interrupts and LY timing are right), but each visible line is composited
//! in one pass at the end of mode 3. That is accurate enough for every
//! mid-frame register change games actually use (scroll splits, window
//! toggles, palette swaps) because those happen during HBlank/VBlank.
//!
//! CGB differences handled here:
//! - two 8 KiB VRAM banks (bank 1 holds BG attribute tiles),
//! - per-tile BG attributes (palette 0-7, bank, X/Y flip, priority),
//! - 8 BG + 8 OBJ palettes of 4 colors each in CRAM (RGB555),
//! - OBJ priority by OAM index rather than by X coordinate,
//! - the BG-over-OBJ master priority bit in LCDC.0 changes meaning.

pub const GB_WIDTH: usize = 160;
pub const GB_HEIGHT: usize = 144;

/// T-cycles per scanline (456) and lines per frame (154) at DMG speed.
pub const CYCLES_PER_LINE: u32 = 456;
pub const LINES_PER_FRAME: u8 = 154;

/// Classic DMG green palette, RGBA8888 (0xAABBGGRR to match the GBA PPU's
/// framebuffer byte order used elsewhere in this codebase).
const DMG_SHADES: [u32; 4] = [0xFF0F_BC9B, 0xFF0F_AC8B, 0xFF30_6230, 0xFF0F_380F];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PpuMode {
    HBlank = 0,
    VBlank = 1,
    OamScan = 2,
    Drawing = 3,
}

pub struct GbPpu {
    pub cgb: bool,

    /// Two VRAM banks; bank 1 is CGB-only.
    pub vram: [[u8; 0x2000]; 2],
    pub vram_bank: usize,
    pub oam: [u8; 0xA0],

    // --- LCD registers ---
    pub lcdc: u8,
    pub stat: u8,
    pub scy: u8,
    pub scx: u8,
    pub ly: u8,
    pub lyc: u8,
    pub bgp: u8,
    pub obp0: u8,
    pub obp1: u8,
    pub wy: u8,
    pub wx: u8,

    // --- CGB palettes ---
    pub bcps: u8,
    pub ocps: u8,
    pub bg_cram: [u8; 64],
    pub obj_cram: [u8; 64],

    pub mode: PpuMode,
    pub dot: u32,
    /// Window internal line counter: advances only on lines where the window
    /// is actually drawn, which is why it cannot be derived from LY.
    window_line: u8,

    pub framebuffer: Box<[u32; GB_WIDTH * GB_HEIGHT]>,
    pub frame_ready: bool,
    pub frame_counter: u64,

    /// Per-pixel BG color index + priority for the current line, consumed by
    /// sprite compositing (a BG color index of 0 is "transparent").
    bg_indices: [u8; GB_WIDTH],
    bg_priority: [bool; GB_WIDTH],
}

impl GbPpu {
    pub fn new(cgb: bool) -> Self {
        Self {
            cgb,
            vram: [[0; 0x2000]; 2],
            vram_bank: 0,
            oam: [0; 0xA0],
            lcdc: 0x91,
            stat: 0x85,
            scy: 0,
            scx: 0,
            ly: 0,
            lyc: 0,
            bgp: 0xFC,
            obp0: 0xFF,
            obp1: 0xFF,
            wy: 0,
            wx: 0,
            bcps: 0,
            ocps: 0,
            // Uninitialised CRAM is effectively white on real hardware after
            // the CGB boot ROM runs; starting at 0xFF avoids a black frame
            // for games that only write the palettes they use.
            bg_cram: [0xFF; 64],
            obj_cram: [0xFF; 64],
            mode: PpuMode::OamScan,
            dot: 0,
            window_line: 0,
            framebuffer: Box::new([0xFF00_0000; GB_WIDTH * GB_HEIGHT]),
            frame_ready: false,
            frame_counter: 0,
            bg_indices: [0; GB_WIDTH],
            bg_priority: [false; GB_WIDTH],
        }
    }

    #[inline]
    pub fn lcd_enabled(&self) -> bool {
        (self.lcdc & 0x80) != 0
    }

    pub fn read_vram(&self, addr: u16) -> u8 {
        self.vram[self.vram_bank][(addr as usize) & 0x1FFF]
    }

    pub fn write_vram(&mut self, addr: u16, val: u8) {
        self.vram[self.vram_bank][(addr as usize) & 0x1FFF] = val;
    }

    pub fn read_oam(&self, addr: u16) -> u8 {
        self.oam[(addr as usize) & 0xFF.min(0x9F)]
    }

    // --- CGB palette access ---------------------------------------------

    pub fn write_bcpd(&mut self, val: u8) {
        let idx = (self.bcps & 0x3F) as usize;
        self.bg_cram[idx] = val;
        if (self.bcps & 0x80) != 0 {
            self.bcps = 0x80 | ((idx as u8 + 1) & 0x3F);
        }
    }

    pub fn read_bcpd(&self) -> u8 {
        self.bg_cram[(self.bcps & 0x3F) as usize]
    }

    pub fn write_ocpd(&mut self, val: u8) {
        let idx = (self.ocps & 0x3F) as usize;
        self.obj_cram[idx] = val;
        if (self.ocps & 0x80) != 0 {
            self.ocps = 0x80 | ((idx as u8 + 1) & 0x3F);
        }
    }

    pub fn read_ocpd(&self) -> u8 {
        self.obj_cram[(self.ocps & 0x3F) as usize]
    }

    /// RGB555 -> RGBA8888 with the same channel order the GBA PPU writes.
    /// The 5->8 bit expansion replicates the high bits (v<<3 | v>>2) so that
    /// 31 maps to 255 rather than 248 -- otherwise whites look grey.
    #[inline]
    fn cram_color(cram: &[u8; 64], pal: usize, color: usize) -> u32 {
        let idx = (pal * 8 + color * 2) & 0x3F;
        let lo = cram[idx] as u16;
        let hi = cram[idx + 1] as u16;
        let rgb555 = (hi << 8) | lo;
        let r = (rgb555 & 0x1F) as u32;
        let g = ((rgb555 >> 5) & 0x1F) as u32;
        let b = ((rgb555 >> 10) & 0x1F) as u32;
        let r8 = (r << 3) | (r >> 2);
        let g8 = (g << 3) | (g >> 2);
        let b8 = (b << 3) | (b >> 2);
        0xFF00_0000 | (b8 << 16) | (g8 << 8) | r8
    }

    #[inline]
    fn dmg_color(palette: u8, index: u8) -> u32 {
        DMG_SHADES[((palette >> (index * 2)) & 0x03) as usize]
    }

    // --- Timing ---------------------------------------------------------

    /// Advance the PPU by `cycles` T-cycles.
    /// Returns (vblank_irq, stat_irq).
    pub fn step(&mut self, cycles: u32) -> (bool, bool) {
        if !self.lcd_enabled() {
            // LCD off: LY is reset and the PPU reports mode 0. Games poll
            // this before writing VRAM in bulk, so it must not keep ticking.
            self.ly = 0;
            self.dot = 0;
            self.window_line = 0;
            self.mode = PpuMode::HBlank;
            self.stat = (self.stat & 0xFC) | 0x00;
            return (false, false);
        }

        let mut vblank_irq = false;
        let mut stat_irq = false;
        let mut remaining = cycles;

        while remaining > 0 {
            let chunk = remaining.min(4);
            remaining -= chunk;
            self.dot += chunk;

            let prev_mode = self.mode;

            if self.ly < GB_HEIGHT as u8 {
                self.mode = if self.dot < 80 {
                    PpuMode::OamScan
                } else if self.dot < 80 + 172 {
                    PpuMode::Drawing
                } else {
                    PpuMode::HBlank
                };
            } else {
                self.mode = PpuMode::VBlank;
            }

            // Render once, at the moment drawing ends for this line.
            if prev_mode == PpuMode::Drawing && self.mode == PpuMode::HBlank {
                self.render_scanline();
            }

            if self.dot >= CYCLES_PER_LINE {
                self.dot -= CYCLES_PER_LINE;
                self.ly += 1;

                if self.ly == GB_HEIGHT as u8 {
                    vblank_irq = true;
                    self.frame_ready = true;
                    self.frame_counter += 1;
                    if (self.stat & 0x10) != 0 {
                        stat_irq = true;
                    }
                } else if self.ly >= LINES_PER_FRAME {
                    self.ly = 0;
                    self.window_line = 0;
                }

                // LYC=LY coincidence is evaluated on every LY change.
                let coincide = self.ly == self.lyc;
                self.stat = (self.stat & !0x04) | ((coincide as u8) << 2);
                if coincide && (self.stat & 0x40) != 0 {
                    stat_irq = true;
                }
            }

            if prev_mode != self.mode {
                let enable_bit = match self.mode {
                    PpuMode::HBlank => 0x08,
                    PpuMode::VBlank => 0x10,
                    PpuMode::OamScan => 0x20,
                    PpuMode::Drawing => 0x00,
                };
                if enable_bit != 0 && (self.stat & enable_bit) != 0 {
                    stat_irq = true;
                }
            }

            self.stat = (self.stat & 0xFC) | (self.mode as u8);
        }

        (vblank_irq, stat_irq)
    }

    // --- Rendering ------------------------------------------------------

    fn render_scanline(&mut self) {
        let y = self.ly as usize;
        if y >= GB_HEIGHT {
            return;
        }
        self.bg_indices = [0; GB_WIDTH];
        self.bg_priority = [false; GB_WIDTH];

        self.render_background(y);
        self.render_window(y);
        if (self.lcdc & 0x02) != 0 {
            self.render_sprites(y);
        }
    }

    fn render_background(&mut self, y: usize) {
        // On DMG, LCDC.0 = 0 blanks BG+window entirely. On CGB the same bit
        // instead only drops BG priority, and the BG still draws.
        let bg_enabled = (self.lcdc & 0x01) != 0 || self.cgb;
        let row = y * GB_WIDTH;
        if !bg_enabled {
            let white = if self.cgb {
                0xFFFF_FFFF
            } else {
                Self::dmg_color(self.bgp, 0)
            };
            for x in 0..GB_WIDTH {
                self.framebuffer[row + x] = white;
            }
            return;
        }

        let map_base: usize = if (self.lcdc & 0x08) != 0 { 0x1C00 } else { 0x1800 };
        let signed_tiles = (self.lcdc & 0x10) == 0;
        let bg_y = (y as u16 + self.scy as u16) & 0xFF;
        let tile_row = (bg_y / 8) as usize;
        let py = (bg_y % 8) as usize;

        for x in 0..GB_WIDTH {
            let bg_x = (x as u16 + self.scx as u16) & 0xFF;
            let tile_col = (bg_x / 8) as usize;
            let px = (bg_x % 8) as usize;

            let map_idx = map_base + tile_row * 32 + tile_col;
            let tile_num = self.vram[0][map_idx];
            let attr = if self.cgb { self.vram[1][map_idx] } else { 0 };

            let (color_idx, rgba, prio) = self.fetch_bg_pixel(tile_num, attr, px, py, signed_tiles);
            self.bg_indices[x] = color_idx;
            self.bg_priority[x] = prio;
            self.framebuffer[row + x] = rgba;
        }
    }

    fn render_window(&mut self, y: usize) {
        let win_enabled = (self.lcdc & 0x20) != 0 && ((self.lcdc & 0x01) != 0 || self.cgb);
        if !win_enabled || y < self.wy as usize || self.wx > 166 {
            return;
        }
        let map_base: usize = if (self.lcdc & 0x40) != 0 { 0x1C00 } else { 0x1800 };
        let signed_tiles = (self.lcdc & 0x10) == 0;
        let wline = self.window_line as usize;
        let tile_row = wline / 8;
        let py = wline % 8;
        let row = y * GB_WIDTH;
        let start_x = (self.wx as i32 - 7).max(0) as usize;
        let mut drew = false;

        for x in start_x..GB_WIDTH {
            let win_x = x as i32 - (self.wx as i32 - 7);
            if win_x < 0 {
                continue;
            }
            drew = true;
            let tile_col = (win_x / 8) as usize;
            let px = (win_x % 8) as usize;
            let map_idx = map_base + tile_row * 32 + (tile_col & 31);
            let tile_num = self.vram[0][map_idx];
            let attr = if self.cgb { self.vram[1][map_idx] } else { 0 };

            let (color_idx, rgba, prio) = self.fetch_bg_pixel(tile_num, attr, px, py, signed_tiles);
            self.bg_indices[x] = color_idx;
            self.bg_priority[x] = prio;
            self.framebuffer[row + x] = rgba;
        }

        if drew {
            self.window_line = self.window_line.wrapping_add(1);
        }
    }

    /// Shared BG/window tile fetch. Returns (palette index, RGBA, BG-priority).
    fn fetch_bg_pixel(
        &self,
        tile_num: u8,
        attr: u8,
        px: usize,
        py: usize,
        signed_tiles: bool,
    ) -> (u8, u32, bool) {
        let bank = if self.cgb { ((attr >> 3) & 1) as usize } else { 0 };
        let xflip = self.cgb && (attr & 0x20) != 0;
        let yflip = self.cgb && (attr & 0x40) != 0;
        let priority = self.cgb && (attr & 0x80) != 0;

        let fy = if yflip { 7 - py } else { py };
        let fx = if xflip { 7 - px } else { px };

        let tile_addr = if signed_tiles {
            // 0x8800 addressing: tile number is signed relative to 0x9000.
            (0x1000i32 + (tile_num as i8 as i32) * 16) as usize
        } else {
            tile_num as usize * 16
        };
        let lo = self.vram[bank][(tile_addr + fy * 2) & 0x1FFF];
        let hi = self.vram[bank][(tile_addr + fy * 2 + 1) & 0x1FFF];
        let bit = 7 - fx;
        let color_idx = (((hi >> bit) & 1) << 1) | ((lo >> bit) & 1);

        let rgba = if self.cgb {
            Self::cram_color(&self.bg_cram, (attr & 0x07) as usize, color_idx as usize)
        } else {
            Self::dmg_color(self.bgp, color_idx)
        };
        (color_idx, rgba, priority)
    }

    fn render_sprites(&mut self, y: usize) {
        let tall = (self.lcdc & 0x04) != 0;
        let height = if tall { 16 } else { 8 };
        let row = y * GB_WIDTH;

        // Hardware scans OAM in index order and keeps the first 10 sprites
        // that intersect the line.
        let mut visible: Vec<usize> = Vec::with_capacity(10);
        for i in 0..40 {
            let sy = self.oam[i * 4] as i32 - 16;
            if (y as i32) >= sy && (y as i32) < sy + height {
                visible.push(i);
                if visible.len() == 10 {
                    break;
                }
            }
        }

        // Draw order: lowest priority first so higher-priority sprites
        // overwrite. DMG priority is by X (ties broken by OAM index);
        // CGB priority is purely OAM index.
        if self.cgb {
            visible.reverse();
        } else {
            visible.sort_by_key(|&i| (std::cmp::Reverse(self.oam[i * 4 + 1]), std::cmp::Reverse(i)));
        }

        for &i in &visible {
            let sy = self.oam[i * 4] as i32 - 16;
            let sx = self.oam[i * 4 + 1] as i32 - 8;
            let mut tile = self.oam[i * 4 + 2];
            let attr = self.oam[i * 4 + 3];

            if tall {
                tile &= 0xFE;
            }
            let yflip = (attr & 0x40) != 0;
            let xflip = (attr & 0x20) != 0;
            let obj_behind_bg = (attr & 0x80) != 0;

            let mut line = (y as i32 - sy) as usize;
            if yflip {
                line = (height as usize - 1) - line;
            }
            let tile_index = tile as usize + if line >= 8 { 1 } else { 0 };
            let fy = line % 8;

            let bank = if self.cgb { ((attr >> 3) & 1) as usize } else { 0 };
            let tile_addr = tile_index * 16;
            let lo = self.vram[bank][(tile_addr + fy * 2) & 0x1FFF];
            let hi = self.vram[bank][(tile_addr + fy * 2 + 1) & 0x1FFF];

            for px in 0..8usize {
                let screen_x = sx + px as i32;
                if screen_x < 0 || screen_x >= GB_WIDTH as i32 {
                    continue;
                }
                let x = screen_x as usize;
                let fx = if xflip { px } else { 7 - px };
                let color_idx = (((hi >> fx) & 1) << 1) | ((lo >> fx) & 1);
                if color_idx == 0 {
                    continue; // color 0 is transparent for sprites
                }

                // Priority resolution. On CGB, LCDC.0 clear makes sprites
                // unconditionally win; otherwise a BG tile with its priority
                // attribute set beats the sprite, as does OBJ-behind-BG when
                // the BG pixel is non-zero.
                let master_prio = !self.cgb || (self.lcdc & 0x01) != 0;
                if master_prio
                    && self.bg_indices[x] != 0
                    && (obj_behind_bg || self.bg_priority[x])
                {
                    continue;
                }

                let rgba = if self.cgb {
                    Self::cram_color(&self.obj_cram, (attr & 0x07) as usize, color_idx as usize)
                } else {
                    let pal = if (attr & 0x10) != 0 { self.obp1 } else { self.obp0 };
                    Self::dmg_color(pal, color_idx)
                };
                self.framebuffer[row + x] = rgba;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ppu_with_lcd_on(cgb: bool) -> GbPpu {
        let mut p = GbPpu::new(cgb);
        p.lcdc = 0x91; // LCD on, BG on, 0x8000 addressing
        p
    }

    #[test]
    fn mode_sequence_over_one_line() {
        let mut p = ppu_with_lcd_on(false);
        p.step(4);
        assert_eq!(p.mode, PpuMode::OamScan);
        p.step(80);
        assert_eq!(p.mode, PpuMode::Drawing);
        p.step(172);
        assert_eq!(p.mode, PpuMode::HBlank);
    }

    #[test]
    fn vblank_fires_at_line_144_and_frame_has_154_lines() {
        let mut p = ppu_with_lcd_on(false);
        let mut vblanks = 0;
        for _ in 0..(CYCLES_PER_LINE * LINES_PER_FRAME as u32 / 4) {
            let (v, _) = p.step(4);
            if v {
                vblanks += 1;
            }
        }
        assert_eq!(vblanks, 1);
        assert_eq!(p.ly, 0, "LY wraps back to 0 after 154 lines");
    }

    #[test]
    fn lyc_coincidence_sets_stat_bit_and_can_raise_irq() {
        let mut p = ppu_with_lcd_on(false);
        p.lyc = 1;
        p.stat |= 0x40; // LYC interrupt enable
        let mut got = false;
        for _ in 0..(CYCLES_PER_LINE / 4 + 2) {
            let (_, s) = p.step(4);
            got |= s;
        }
        assert_eq!(p.ly, 1);
        assert!(p.stat & 0x04 != 0, "coincidence flag");
        assert!(got, "STAT IRQ raised on LYC match");
    }

    #[test]
    fn lcd_off_holds_ly_at_zero() {
        let mut p = ppu_with_lcd_on(false);
        p.lcdc = 0x00;
        p.step(CYCLES_PER_LINE * 3);
        assert_eq!(p.ly, 0);
        assert_eq!(p.mode, PpuMode::HBlank);
    }

    #[test]
    fn dmg_background_uses_bgp_shades() {
        let mut p = ppu_with_lcd_on(false);
        p.bgp = 0b11_10_01_00;
        // Tile 0, row 0: color index 3 across all 8 pixels.
        p.vram[0][0] = 0xFF;
        p.vram[0][1] = 0xFF;
        p.render_scanline();
        assert_eq!(p.framebuffer[0], DMG_SHADES[3]);
    }

    #[test]
    fn cgb_palette_expands_rgb555_with_replicated_bits() {
        let mut p = ppu_with_lcd_on(true);
        // BG palette 0, color 3 = pure white (0x7FFF).
        p.bg_cram[6] = 0xFF;
        p.bg_cram[7] = 0x7F;
        p.vram[0][0] = 0xFF;
        p.vram[0][1] = 0xFF;
        p.render_scanline();
        assert_eq!(
            p.framebuffer[0], 0xFFFF_FFFF,
            "31/31/31 must expand to 255/255/255, not 248"
        );
    }

    #[test]
    fn cgb_bg_attribute_selects_vram_bank_one() {
        let mut p = ppu_with_lcd_on(true);
        p.vram[1][0x1800] = 0x08; // BG map attr: bank 1
        p.vram[1][0] = 0xFF; // tile data in bank 1
        p.vram[1][1] = 0xFF;
        p.bg_cram[6] = 0xFF;
        p.bg_cram[7] = 0x7F;
        p.render_scanline();
        assert_eq!(p.framebuffer[0], 0xFFFF_FFFF);
    }

    #[test]
    fn sprite_color_zero_is_transparent() {
        let mut p = ppu_with_lcd_on(false);
        p.lcdc |= 0x02; // OBJ enable
        p.bgp = 0b11_11_11_00;
        p.obp0 = 0b11_10_01_00;
        // Sprite at (0,0) with an all-zero tile -> nothing drawn.
        p.oam[0] = 16;
        p.oam[1] = 8;
        p.oam[2] = 0;
        p.render_scanline();
        assert_eq!(p.framebuffer[0], DMG_SHADES[0], "BG color 0 survives");
    }

    #[test]
    fn only_ten_sprites_per_line_are_drawn() {
        let mut p = ppu_with_lcd_on(false);
        p.lcdc |= 0x02;
        for i in 0..20 {
            p.oam[i * 4] = 16; // y=0
            p.oam[i * 4 + 1] = (8 + i * 8) as u8;
            p.oam[i * 4 + 2] = 1;
        }
        // Tile 1 solid color 3.
        p.vram[0][16] = 0xFF;
        p.vram[0][17] = 0xFF;
        p.obp0 = 0b11_10_01_00;
        p.render_scanline();
        // 11th sprite starts at x=80; it must not have been drawn.
        assert_eq!(p.framebuffer[80], DMG_SHADES[0]);
        assert_eq!(p.framebuffer[72], DMG_SHADES[3], "10th sprite drawn");
    }
}
