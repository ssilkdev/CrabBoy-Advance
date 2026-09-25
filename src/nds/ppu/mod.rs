//! Nintendo DS PPU Coordinator
//!
//! Controls dual 2D graphics engines (Engine A & Engine B), VBlank/HBlank timing,
//! POWCNT1 display routing, and dual-framebuffer (256x384) output.

pub mod engine_2d;
pub mod engine_3d;

use crate::nds::bus::VramViews;
use engine_2d::{Engine2D, EngineVram, SCREEN_HEIGHT, SCREEN_WIDTH};
use engine_3d::Engine3D;

pub const TOTAL_SCANLINES: usize = 263;
pub const DUAL_FRAMEBUFFER_HEIGHT: usize = SCREEN_HEIGHT * 2; // 384

pub struct NdsPpu {
    pub engine_a: Engine2D,
    pub engine_b: Engine2D,
    pub engine_3d: Engine3D,

    pub dispstat_a: u16,
    pub dispstat_b: u16,
    pub vcount: u16,
    pub powcnt1: u32,

    /// Stitched dual framebuffer (256x384 RGBA 0xAABBGGRR)
    pub framebuffer: Vec<u32>,
}

impl Default for NdsPpu {
    fn default() -> Self {
        Self::new()
    }
}

impl NdsPpu {
    pub fn new() -> Self {
        Self {
            engine_a: Engine2D::new(true),
            engine_b: Engine2D::new(false),
            engine_3d: Engine3D::new(),
            dispstat_a: 0,
            dispstat_b: 0,
            vcount: 0,
            // Both 2D engines on, Engine A on the upper screen (what the BIOS
        // leaves for a direct boot).
        powcnt1: 0x820F,
            framebuffer: vec![0xFF000000; SCREEN_WIDTH * DUAL_FRAMEBUFFER_HEIGHT],
        }
    }

    /// Update VCOUNT and status flags for current scanline
    pub fn set_scanline(&mut self, line: u16) {
        self.vcount = line;
        let vblank = line >= (SCREEN_HEIGHT as u16) && line < (TOTAL_SCANLINES as u16);

        // Update DISPSTAT for Engine A
        if vblank {
            self.dispstat_a |= 1;
        } else {
            self.dispstat_a &= !1;
        }

        // V-Counter match flag (Bit 2) and setting (Bits 8-15)
        let vmatch_setting_a = (self.dispstat_a >> 8) & 0xFF;
        if (line as u8) == (vmatch_setting_a as u8) {
            self.dispstat_a |= 1 << 2;
        } else {
            self.dispstat_a &= !(1 << 2);
        }

        // Update DISPSTAT for Engine B
        if vblank {
            self.dispstat_b |= 1;
        } else {
            self.dispstat_b &= !1;
        }
        let vmatch_setting_b = (self.dispstat_b >> 8) & 0xFF;
        if (line as u8) == (vmatch_setting_b as u8) {
            self.dispstat_b |= 1 << 2;
        } else {
            self.dispstat_b &= !(1 << 2);
        }
    }

    /// Renders both Engine A and Engine B scanlines and stitches into unified framebuffer.
    ///
    /// `lcdc` holds raw banks A-D for Engine A's VRAM display mode; `views`
    /// holds each engine's mapped BG/OBJ VRAM.
    pub fn render_scanline(&mut self, line: usize, lcdc: [&[u8]; 4], views: &VramViews) {
        if line >= SCREEN_HEIGHT {
            return;
        }

        if line == 0 {
            self.engine_a.reset_scanline_affine();
            self.engine_b.reset_scanline_affine();
        }
        let va = EngineVram { bg: &views.a_bg, obj: &views.a_obj, bg_ext: &views.a_bg_ext, obj_ext: &views.a_obj_ext };
        let vb = EngineVram { bg: &views.b_bg, obj: &views.b_obj, bg_ext: &views.b_bg_ext, obj_ext: &views.b_obj_ext };
        self.engine_a.render_scanline(line, &va, Some(&self.engine_3d.rasterizer.color_buffer));
        self.engine_b.render_scanline(line, &vb, None);

        // DISPCNT bits 16-17 pick what the engine actually outputs.
        // Engine A: 0 = display off (white), 1 = graphics, 2 = VRAM display
        // (one 128 KiB LCDC bank as a raw 256x192 BGR555 bitmap, the mode
        // armwrestler/rockwrestler and many homebrew use), 3 = main-memory
        // FIFO (not emulated; shows graphics). Engine B: only 0 and 1.
        let row = line * SCREEN_WIDTH;
        match (self.engine_a.dispcnt >> 16) & 3 {
            0 => self.engine_a.framebuffer[row..row + SCREEN_WIDTH].fill(0xFFFF_FFFF),
            2 => {
                let bank = lcdc[((self.engine_a.dispcnt >> 18) & 3) as usize];
                let base = line * SCREEN_WIDTH * 2;
                for x in 0..SCREEN_WIDTH {
                    let i = base + x * 2;
                    let c = match bank.get(i..i + 2) {
                        Some(b) => u16::from_le_bytes([b[0], b[1]]),
                        None => 0,
                    };
                    self.engine_a.framebuffer[row + x] = Engine2D::bgr555_to_rgba(c);
                }
            }
            _ => {}
        }
        if (self.engine_b.dispcnt >> 16) & 1 == 0 {
            self.engine_b.framebuffer[row..row + SCREEN_WIDTH].fill(0xFFFF_FFFF);
        }

        // Copy scanlines into dual framebuffer based on POWCNT1 display swap.
        // Bit 15 of POWCNT1 (GBATEK): 1 = Engine A on the upper screen,
        // 0 = Engine A on the lower screen.
        let swap_screens = (self.powcnt1 & (1 << 15)) == 0;

        let top_src = if swap_screens {
            &self.engine_b.framebuffer[line * SCREEN_WIDTH..(line + 1) * SCREEN_WIDTH]
        } else {
            &self.engine_a.framebuffer[line * SCREEN_WIDTH..(line + 1) * SCREEN_WIDTH]
        };

        let bottom_src = if swap_screens {
            &self.engine_a.framebuffer[line * SCREEN_WIDTH..(line + 1) * SCREEN_WIDTH]
        } else {
            &self.engine_b.framebuffer[line * SCREEN_WIDTH..(line + 1) * SCREEN_WIDTH]
        };

        // Top screen: lines 0..191
        let top_dest_offset = line * SCREEN_WIDTH;
        self.framebuffer[top_dest_offset..top_dest_offset + SCREEN_WIDTH].copy_from_slice(top_src);

        // Bottom screen: lines 192..383
        let bot_dest_offset = (SCREEN_HEIGHT + line) * SCREEN_WIDTH;
        self.framebuffer[bot_dest_offset..bot_dest_offset + SCREEN_WIDTH].copy_from_slice(bottom_src);

        self.engine_a.step_scanline_affine();
        self.engine_b.step_scanline_affine();
    }

    /// Read 32-bit I/O register for Engine A and 3D Engine (0x0400_0000..=0x0400_06A0)
    pub fn read_io_a(&mut self, addr: u32) -> u32 {
        match addr {
            0x0400_0004 => (self.dispstat_a as u32) | ((self.vcount as u32) << 16),
            0x0400_0060 => self.engine_3d.rasterizer.disp3dcnt as u32,
            0x0400_0320..=0x0400_06A0 => self.engine_3d.read_io(addr),
            0x0400_0000..=0x0400_006F => {
                let o = addr & 0xFC;
                self.engine_a.read_reg16(o) as u32 | ((self.engine_a.read_reg16(o + 2) as u32) << 16)
            }
            _ => 0,
        }
    }

    /// Write 32-bit I/O register for Engine A and 3D Engine
    pub fn write_io_a(&mut self, addr: u32, val: u32) {
        match addr {
            0x0400_0004 => self.dispstat_a = (self.dispstat_a & 0x07) | ((val as u16) & !0x07),
            0x0400_0060 => self.engine_3d.rasterizer.disp3dcnt = val as u16,
            0x0400_0320..=0x0400_06A0 => self.engine_3d.write_io(addr, val),
            0x0400_0000..=0x0400_006F => {
                self.engine_a.write_reg16(addr & 0xFC, val as u16);
                self.engine_a.write_reg16((addr & 0xFC) + 2, (val >> 16) as u16);
            }
            _ => {}
        }
    }

    /// Write 16-bit I/O register for Engine A and 3D Engine
    pub fn write_io_a_u16(&mut self, addr: u32, val: u16) {
        match addr {
            0x0400_0004 => self.dispstat_a = (self.dispstat_a & 0x07) | (val & !0x07),
            0x0400_0060 => self.engine_3d.rasterizer.disp3dcnt = val,
            0x0400_0320..=0x0400_06A0 => self.engine_3d.write_io_u16(addr, val),
            0x0400_0000..=0x0400_006F => self.engine_a.write_reg16(addr & 0xFE, val),
            _ => {}
        }
    }

    /// Read 32-bit I/O register for Engine B (0x0400_1000..0x0400_106F)
    pub fn read_io_b(&self, addr: u32) -> u32 {
        match addr {
            0x0400_1004 => (self.dispstat_b as u32) | ((self.vcount as u32) << 16),
            _ => {
                let o = addr & 0xFC;
                self.engine_b.read_reg16(o) as u32 | ((self.engine_b.read_reg16(o + 2) as u32) << 16)
            }
        }
    }

    /// Write 32-bit I/O register for Engine B
    pub fn write_io_b(&mut self, addr: u32, val: u32) {
        match addr {
            0x0400_1004 => self.dispstat_b = (self.dispstat_b & 0x07) | ((val as u16) & !0x07),
            _ => {
                self.engine_b.write_reg16(addr & 0xFC, val as u16);
                self.engine_b.write_reg16((addr & 0xFC) + 2, (val >> 16) as u16);
            }
        }
    }

    /// Write 16-bit I/O register for Engine B
    pub fn write_io_b_u16(&mut self, addr: u32, val: u16) {
        match addr {
            0x0400_1004 => self.dispstat_b = (self.dispstat_b & 0x07) | (val & !0x07),
            _ => self.engine_b.write_reg16(addr & 0xFE, val),
        }
    }
}
