//! GBA Picture Processing Unit (PPU)
#![allow(clippy::needless_range_loop)]

pub mod bg;
pub mod blend;
pub mod obj;

use bg::{render_affine_bg, render_bitmap_bg, render_text_bg};
use blend::{apply_color_effects, bgr555_to_rgb888, Pixel};
use obj::render_sprites;

pub const SCREEN_WIDTH: usize = 240;
pub const SCREEN_HEIGHT: usize = 160;
pub const TOTAL_SCANLINES: u32 = 228;
pub const SCANLINE_CYCLES: u32 = 1232;
pub const HDRAW_CYCLES: u32 = 960;

pub struct Ppu {
    pub vram: Box<[u8; 96 * 1024]>,
    pub palette_ram: Box<[u8; 1024]>,
    pub oam: Box<[u8; 1024]>,

    // Registers
    pub dispcnt: u16,
    pub dispstat: u16,
    pub vcount: u16,

    pub bgcnt: [u16; 4],
    pub bghofs: [u16; 4],
    pub bgvofs: [u16; 4],

    // Affine parameters
    pub bg_pa: [i16; 2],
    pub bg_pb: [i16; 2],
    pub bg_pc: [i16; 2],
    pub bg_pd: [i16; 2],
    pub bg_x: [i32; 2],
    pub bg_y: [i32; 2],
    pub bg_x_internal: [i32; 2],
    pub bg_y_internal: [i32; 2],

    // Windows
    pub win0h: u16,
    pub win1h: u16,
    pub win0v: u16,
    pub win1v: u16,
    pub winin: u16,
    pub winout: u16,

    // Color effects
    pub bldcnt: u16,
    pub bldalpha: u16,
    pub bldy: u16,

    // Scanline timing state
    pub cycle_in_scanline: u32,
    pub frame_ready: bool,
    // Layer toggle mask: bit 0..3 = BG0..3, bit 4 = OBJ (default 0x1F = all enabled)
    pub layer_mask: u8,

    // Front/back RGBA8888 framebuffers (240x160)
    pub framebuffer: Box<[u32; SCREEN_WIDTH * SCREEN_HEIGHT]>,
}

impl Default for Ppu {
    fn default() -> Self {
        Self::new()
    }
}

impl Ppu {
    pub fn new() -> Self {
        Self {
            vram: vec![0u8; 96 * 1024].into_boxed_slice().try_into().unwrap(),
            palette_ram: Box::new([0; 1024]),
            oam: Box::new([0; 1024]),
            dispcnt: 0x0080, // Forced blank initially
            dispstat: 0,
            vcount: 0,
            bgcnt: [0; 4],
            bghofs: [0; 4],
            bgvofs: [0; 4],
            bg_pa: [0x0100, 0x0100],
            bg_pb: [0, 0],
            bg_pc: [0, 0],
            bg_pd: [0x0100, 0x0100],
            bg_x: [0, 0],
            bg_y: [0, 0],
            bg_x_internal: [0, 0],
            bg_y_internal: [0, 0],
            win0h: 0,
            win1h: 0,
            win0v: 0,
            win1v: 0,
            winin: 0,
            winout: 0,
            bldcnt: 0,
            bldalpha: 0,
            bldy: 0,
            cycle_in_scanline: 0,
            frame_ready: false,
            layer_mask: 0x1F,
            framebuffer: Box::new([0xFF00_0000; SCREEN_WIDTH * SCREEN_HEIGHT]),
        }
    }

    /// Advances PPU by elapsed cycles. Returns (irq_vblank, irq_hblank, irq_vcounter, dma_vblank, dma_hblank)
    pub fn step(&mut self, cycles: u32) -> (bool, bool, bool, bool, bool) {
        let mut irq_vblank = false;
        let mut irq_hblank = false;
        let mut irq_vcounter = false;
        let mut dma_vblank = false;
        let mut dma_hblank = false;

        self.cycle_in_scanline += cycles;

        // Check HBlank transition (at cycle 960)
        let old_hblank = (self.dispstat & 2) != 0;
        let in_hblank = self.cycle_in_scanline >= HDRAW_CYCLES;

        if self.vcount < 160 {
            if !old_hblank && in_hblank {
                self.dispstat |= 2;
                if (self.dispstat & (1 << 4)) != 0 {
                    irq_hblank = true;
                }
                dma_hblank = true;
            }
        } else {
            // During VBlank (lines 160..227), HBlank flag is not set on GBA
            self.dispstat &= !2;
        }

        if self.cycle_in_scanline >= SCANLINE_CYCLES {
            self.cycle_in_scanline -= SCANLINE_CYCLES;
            self.dispstat &= !2; // Exit HBlank

            // Render previous scanline if visible
            if self.vcount < 160 {
                self.render_scanline(self.vcount as u32);
            }

            self.vcount += 1;
            if self.vcount == 160 {
                // Entering VBlank
                self.dispstat |= 1;
                self.frame_ready = true;

                if (self.dispstat & (1 << 3)) != 0 {
                    irq_vblank = true;
                }
                dma_vblank = true;
            } else if self.vcount == 227 {
                // On scanline 227, VBlank flag in DISPSTAT is cleared according to GBA specs
                self.dispstat &= !1;
            } else if self.vcount >= TOTAL_SCANLINES as u16 {
                // New frame
                self.vcount = 0;
                self.dispstat &= !1; // Ensure VBlank cleared
                // Reload internal affine coordinates at start of frame
                self.bg_x_internal = self.bg_x;
                self.bg_y_internal = self.bg_y;
            }

            // V-Counter match check
            let target_vcount = (self.dispstat >> 8) & 0xFF;
            if self.vcount == target_vcount {
                self.dispstat |= 4;
                if (self.dispstat & (1 << 5)) != 0 {
                    irq_vcounter = true;
                }
            } else {
                self.dispstat &= !4;
            }
        }

        (irq_vblank, irq_hblank, irq_vcounter, dma_vblank, dma_hblank)
    }

    fn render_scanline(&mut self, y: u32) {
        if (self.dispcnt & (1 << 7)) != 0 {
            // Forced blank: render white
            let row = (y as usize) * SCREEN_WIDTH;
            for x in 0..SCREEN_WIDTH {
                self.framebuffer[row + x] = 0xFFFF_FFFF;
            }
            return;
        }

        // Get backdrop color (Palette entry 0)
        let backdrop_color = (self.palette_ram[0] as u16) | ((self.palette_ram[1] as u16) << 8);

        let mut bg_layer_bufs: [[Pixel; SCREEN_WIDTH]; 4] = [[Pixel::default(); SCREEN_WIDTH]; 4];
        let mut obj_buf: [Pixel; SCREEN_WIDTH] = [Pixel::default(); SCREEN_WIDTH];
        let mut objwin_buf: [bool; SCREEN_WIDTH] = [false; SCREEN_WIDTH];

        let mode = (self.dispcnt & 7) as u8;
        let frame = (self.dispcnt & (1 << 4)) != 0;

        // Render BG layers based on mode
        match mode {
            0 => {
                // Mode 0: Text BG 0, 1, 2, 3
                for i in 0..4 {
                    if (self.dispcnt & (1 << (8 + i))) != 0 && (self.layer_mask & (1 << i)) != 0 {
                        render_text_bg(
                            i as u8,
                            y,
                            self.dispcnt,
                            self.bgcnt[i],
                            self.bghofs[i],
                            self.bgvofs[i],
                            &self.vram[..],
                            &self.palette_ram[..],
                            &mut bg_layer_bufs[i],
                        );
                    }
                }
            }
            1 => {
                // Mode 1: BG 0, 1 Text, BG 2 Affine
                for i in 0..2 {
                    if (self.dispcnt & (1 << (8 + i))) != 0 && (self.layer_mask & (1 << i)) != 0 {
                        render_text_bg(
                            i as u8,
                            y,
                            self.dispcnt,
                            self.bgcnt[i],
                            self.bghofs[i],
                            self.bgvofs[i],
                            &self.vram[..],
                            &self.palette_ram[..],
                            &mut bg_layer_bufs[i],
                        );
                    }
                }
                if (self.dispcnt & (1 << 10)) != 0 && (self.layer_mask & (1 << 2)) != 0 {
                    render_affine_bg(
                        2,
                        y,
                        self.bgcnt[2],
                        self.bg_x_internal[0],
                        self.bg_y_internal[0],
                        self.bg_pa[0],
                        self.bg_pb[0],
                        self.bg_pc[0],
                        self.bg_pd[0],
                        &self.vram[..],
                        &self.palette_ram[..],
                        &mut bg_layer_bufs[2],
                    );
                }
                // Affine internal registers always increment every scanline, even if BG is disabled
                self.bg_x_internal[0] += self.bg_pb[0] as i32;
                self.bg_y_internal[0] += self.bg_pd[0] as i32;
            }
            2 => {
                // Mode 2: BG 2, 3 Affine
                for i in 0..2 {
                    let bg_idx = 2 + i;
                    if (self.dispcnt & (1 << (8 + bg_idx))) != 0 && (self.layer_mask & (1 << bg_idx)) != 0 {
                        render_affine_bg(
                            bg_idx as u8,
                            y,
                            self.bgcnt[bg_idx],
                            self.bg_x_internal[i],
                            self.bg_y_internal[i],
                            self.bg_pa[i],
                            self.bg_pb[i],
                            self.bg_pc[i],
                            self.bg_pd[i],
                            &self.vram[..],
                            &self.palette_ram[..],
                            &mut bg_layer_bufs[bg_idx],
                        );
                    }
                    // Always increment affine internal registers per scanline
                    self.bg_x_internal[i] += self.bg_pb[i] as i32;
                    self.bg_y_internal[i] += self.bg_pd[i] as i32;
                }
            }
            3..=5
                if (self.dispcnt & (1 << 10)) != 0 && (self.layer_mask & (1 << 2)) != 0 => {
                    render_bitmap_bg(
                        mode,
                        frame,
                        y,
                        &self.vram[..],
                        &self.palette_ram[..],
                        &mut bg_layer_bufs[2],
                    );
                }
            _ => {}
        }

        // Render sprites (OBJ)
        if (self.dispcnt & (1 << 12)) != 0 && (self.layer_mask & (1 << 4)) != 0 {
            render_sprites(
                y,
                self.dispcnt,
                &self.oam[..],
                &self.vram[..],
                &self.palette_ram[..],
                &mut obj_buf,
                &mut objwin_buf,
            );
        }

        // Windowing configuration
        let win0_enable = (self.dispcnt & (1 << 13)) != 0;
        let win1_enable = (self.dispcnt & (1 << 14)) != 0;
        let objwin_enable = (self.dispcnt & (1 << 15)) != 0;
        let any_win = win0_enable || win1_enable || objwin_enable;

        let win0_y1 = (self.win0v >> 8) as u32;
        let win0_y2 = (self.win0v & 0xFF) as u32;
        let in_win0_y = win0_enable && (if win0_y1 <= win0_y2 { y >= win0_y1 && y < win0_y2 } else { y >= win0_y1 || y < win0_y2 });

        let win1_y1 = (self.win1v >> 8) as u32;
        let win1_y2 = (self.win1v & 0xFF) as u32;
        let in_win1_y = win1_enable && (if win1_y1 <= win1_y2 { y >= win1_y1 && y < win1_y2 } else { y >= win1_y1 || y < win1_y2 });

        let win0_x1 = (self.win0h >> 8) as usize;
        let win0_x2 = (self.win0h & 0xFF) as usize;
        let win1_x1 = (self.win1h >> 8) as usize;
        let win1_x2 = (self.win1h & 0xFF) as usize;

        // Precompute window masks across the scanline
        let mut win_masks = [0x3Fu8; SCREEN_WIDTH];
        if any_win {
            for x in 0..SCREEN_WIDTH {
                let in_win0 = in_win0_y && (if win0_x1 <= win0_x2 { x >= win0_x1 && x < win0_x2 } else { x >= win0_x1 || x < win0_x2 });
                let in_win1 = in_win1_y && (if win1_x1 <= win1_x2 { x >= win1_x1 && x < win1_x2 } else { x >= win1_x1 || x < win1_x2 });

                win_masks[x] = if in_win0 {
                    (self.winin & 0x3F) as u8
                } else if in_win1 {
                    ((self.winin >> 8) & 0x3F) as u8
                } else if objwin_enable && objwin_buf[x] {
                    ((self.winout >> 8) & 0x3F) as u8 // OBJWIN uses WINOUT bits 8-13
                } else {
                    (self.winout & 0x3F) as u8
                };
            }
        }

        let eva = self.bldalpha & 0x1F;
        let evb = (self.bldalpha >> 8) & 0x1F;
        let evy = self.bldy & 0x1F;

        let row_offset = (y as usize) * SCREEN_WIDTH;

        // Merge layers per pixel
        for x in 0..SCREEN_WIDTH {
            // Determine window mask
            let win_mask = win_masks[x];

            // Collect top 2 visible pixels
            let mut top = Pixel {
                color: backdrop_color,
                layer: 5,
                priority: 4,
                is_transparent: false,
                is_obj_alpha: false,
            };
            let mut bot = top;

            // Check OBJ
            if (win_mask & (1 << 4)) != 0 && !obj_buf[x].is_transparent {
                top = obj_buf[x];
            }

            // Check BG layers (in ascending priority, 3 down to 0)
            for prio in (0..=3).rev() {
                for bg_idx in 0..4 {
                    if (win_mask & (1 << bg_idx)) != 0 && !bg_layer_bufs[bg_idx][x].is_transparent && bg_layer_bufs[bg_idx][x].priority == prio {
                        if bg_layer_bufs[bg_idx][x].priority < top.priority {
                            bot = top;
                            top = bg_layer_bufs[bg_idx][x];
                        } else if bg_layer_bufs[bg_idx][x].priority < bot.priority {
                            bot = bg_layer_bufs[bg_idx][x];
                        }
                    }
                }
            }

            // Apply color special effects
            let final_bgr555 = if (win_mask & (1 << 5)) != 0 {
                apply_color_effects(top, bot, self.bldcnt, eva, evb, evy)
            } else {
                top.color
            };

            let (r, g, b) = bgr555_to_rgb888(final_bgr555);
            // Format: 0xFF_AA_BB_GG_RR in little-endian (or RGBA 0xFF_BB_GG_RR)
            self.framebuffer[row_offset + x] = 0xFF00_0000 | ((b as u32) << 16) | ((g as u32) << 8) | (r as u32);
        }
    }
}
