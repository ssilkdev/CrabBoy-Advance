//! Nintendo DS PPU Coordinator
//!
//! Controls dual 2D graphics engines (Engine A & Engine B), VBlank/HBlank timing,
//! POWCNT1 display routing, and dual-framebuffer (256x384) output.

pub mod engine_2d;
pub mod engine_3d;

use crate::nds::bus::VramViews;
use engine_2d::{Engine2D, SCREEN_HEIGHT, SCREEN_WIDTH};
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

        // Render both 2D engines for this scanline
        self.engine_a.render_scanline(line, &views.a_bg, &views.a_obj);
        self.engine_b.render_scanline(line, &views.b_bg, &views.b_obj);

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
        let vram_display = (self.engine_a.dispcnt >> 16) & 3 == 2;

        // If Engine A has 3D display enabled (DISPCNT bit 3), composite 3D layer into Engine A
        if !vram_display && (self.engine_a.dispcnt & (1 << 3)) != 0 {
            let offset_3d = line * SCREEN_WIDTH;
            for x in 0..SCREEN_WIDTH {
                let pixel_3d = self.engine_3d.rasterizer.color_buffer[offset_3d + x];
                if (pixel_3d & 0xFF00_0000) != 0 {
                    self.engine_a.framebuffer[offset_3d + x] = pixel_3d;
                }
            }
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
            0x0400_0000 => self.engine_a.dispcnt,
            0x0400_0004 => (self.dispstat_a as u32) | ((self.vcount as u32) << 16),
            0x0400_0008 => (self.engine_a.bgcnt[0] as u32) | ((self.engine_a.bgcnt[1] as u32) << 16),
            0x0400_000C => (self.engine_a.bgcnt[2] as u32) | ((self.engine_a.bgcnt[3] as u32) << 16),
            0x0400_0050 => (self.engine_a.bldcnt as u32) | ((self.engine_a.bldalpha as u32) << 16),
            0x0400_0060 => self.engine_3d.rasterizer.disp3dcnt as u32,
            0x0400_006C => self.engine_a.master_bright as u32,
            0x0400_0320..=0x0400_06A0 => self.engine_3d.read_io(addr),
            _ => 0,
        }
    }

    /// Write 32-bit I/O register for Engine A and 3D Engine
    pub fn write_io_a(&mut self, addr: u32, val: u32) {
        match addr {
            0x0400_0000 => self.engine_a.dispcnt = val,
            0x0400_0004 => {
                self.dispstat_a = (self.dispstat_a & 0x07) | ((val as u16) & !0x07);
            }
            0x0400_0008 => {
                self.engine_a.bgcnt[0] = val as u16;
                self.engine_a.bgcnt[1] = (val >> 16) as u16;
            }
            0x0400_000C => {
                self.engine_a.bgcnt[2] = val as u16;
                self.engine_a.bgcnt[3] = (val >> 16) as u16;
            }
            0x0400_0010 => {
                self.engine_a.bghofs[0] = val as u16;
                self.engine_a.bgvofs[0] = (val >> 16) as u16;
            }
            0x0400_0014 => {
                self.engine_a.bghofs[1] = val as u16;
                self.engine_a.bgvofs[1] = (val >> 16) as u16;
            }
            0x0400_0018 => {
                self.engine_a.bghofs[2] = val as u16;
                self.engine_a.bgvofs[2] = (val >> 16) as u16;
            }
            0x0400_001C => {
                self.engine_a.bghofs[3] = val as u16;
                self.engine_a.bgvofs[3] = (val >> 16) as u16;
            }
            0x0400_0050 => {
                self.engine_a.bldcnt = val as u16;
                self.engine_a.bldalpha = (val >> 16) as u16;
            }
            0x0400_0060 => self.engine_3d.rasterizer.disp3dcnt = val as u16,
            0x0400_006C => self.engine_a.master_bright = val as u16,
            0x0400_0320..=0x0400_06A0 => self.engine_3d.write_io(addr, val),
            _ => {}
        }
    }

    /// Write 16-bit I/O register for Engine A and 3D Engine
    pub fn write_io_a_u16(&mut self, addr: u32, val: u16) {
        match addr {
            0x0400_0000 => self.engine_a.dispcnt = (self.engine_a.dispcnt & 0xFFFF_0000) | (val as u32),
            0x0400_0002 => self.engine_a.dispcnt = (self.engine_a.dispcnt & 0x0000_FFFF) | ((val as u32) << 16),
            0x0400_0004 => self.dispstat_a = (self.dispstat_a & 0x07) | (val & !0x07),
            0x0400_0008 => self.engine_a.bgcnt[0] = val,
            0x0400_000A => self.engine_a.bgcnt[1] = val,
            0x0400_000C => self.engine_a.bgcnt[2] = val,
            0x0400_000E => self.engine_a.bgcnt[3] = val,
            0x0400_0010 => self.engine_a.bghofs[0] = val,
            0x0400_0012 => self.engine_a.bgvofs[0] = val,
            0x0400_0014 => self.engine_a.bghofs[1] = val,
            0x0400_0016 => self.engine_a.bgvofs[1] = val,
            0x0400_0018 => self.engine_a.bghofs[2] = val,
            0x0400_001A => self.engine_a.bgvofs[2] = val,
            0x0400_001C => self.engine_a.bghofs[3] = val,
            0x0400_001E => self.engine_a.bgvofs[3] = val,
            0x0400_0050 => self.engine_a.bldcnt = val,
            0x0400_0052 => self.engine_a.bldalpha = val,
            0x0400_0054 => self.engine_a.bldy = val,
            0x0400_0060 => self.engine_3d.rasterizer.disp3dcnt = val,
            0x0400_006C => self.engine_a.master_bright = val,
            0x0400_0320..=0x0400_06A0 => self.engine_3d.write_io_u16(addr, val),
            _ => {}
        }
    }

    /// Read 32-bit I/O register for Engine B (0x0400_1000..0x0400_1060)
    pub fn read_io_b(&self, addr: u32) -> u32 {
        match addr {
            0x0400_1000 => self.engine_b.dispcnt,
            0x0400_1004 => (self.dispstat_b as u32) | ((self.vcount as u32) << 16),
            0x0400_1008 => (self.engine_b.bgcnt[0] as u32) | ((self.engine_b.bgcnt[1] as u32) << 16),
            0x0400_100C => (self.engine_b.bgcnt[2] as u32) | ((self.engine_b.bgcnt[3] as u32) << 16),
            0x0400_1050 => (self.engine_b.bldcnt as u32) | ((self.engine_b.bldalpha as u32) << 16),
            0x0400_106C => self.engine_b.master_bright as u32,
            _ => 0,
        }
    }

    /// Write 32-bit I/O register for Engine B
    pub fn write_io_b(&mut self, addr: u32, val: u32) {
        match addr {
            0x0400_1000 => self.engine_b.dispcnt = val,
            0x0400_1004 => {
                self.dispstat_b = (self.dispstat_b & 0x07) | ((val as u16) & !0x07);
            }
            0x0400_1008 => {
                self.engine_b.bgcnt[0] = val as u16;
                self.engine_b.bgcnt[1] = (val >> 16) as u16;
            }
            0x0400_100C => {
                self.engine_b.bgcnt[2] = val as u16;
                self.engine_b.bgcnt[3] = (val >> 16) as u16;
            }
            0x0400_1010 => {
                self.engine_b.bghofs[0] = val as u16;
                self.engine_b.bgvofs[0] = (val >> 16) as u16;
            }
            0x0400_1014 => {
                self.engine_b.bghofs[1] = val as u16;
                self.engine_b.bgvofs[1] = (val >> 16) as u16;
            }
            0x0400_1018 => {
                self.engine_b.bghofs[2] = val as u16;
                self.engine_b.bgvofs[2] = (val >> 16) as u16;
            }
            0x0400_101C => {
                self.engine_b.bghofs[3] = val as u16;
                self.engine_b.bgvofs[3] = (val >> 16) as u16;
            }
            0x0400_1050 => {
                self.engine_b.bldcnt = val as u16;
                self.engine_b.bldalpha = (val >> 16) as u16;
            }
            0x0400_106C => self.engine_b.master_bright = val as u16,
            _ => {}
        }
    }

    /// Write 16-bit I/O register for Engine B
    pub fn write_io_b_u16(&mut self, addr: u32, val: u16) {
        match addr {
            0x0400_1000 => self.engine_b.dispcnt = (self.engine_b.dispcnt & 0xFFFF_0000) | (val as u32),
            0x0400_1002 => self.engine_b.dispcnt = (self.engine_b.dispcnt & 0x0000_FFFF) | ((val as u32) << 16),
            0x0400_1004 => self.dispstat_b = (self.dispstat_b & 0x07) | (val & !0x07),
            0x0400_1008 => self.engine_b.bgcnt[0] = val,
            0x0400_100A => self.engine_b.bgcnt[1] = val,
            0x0400_100C => self.engine_b.bgcnt[2] = val,
            0x0400_100E => self.engine_b.bgcnt[3] = val,
            0x0400_1010 => self.engine_b.bghofs[0] = val,
            0x0400_1012 => self.engine_b.bgvofs[0] = val,
            0x0400_1014 => self.engine_b.bghofs[1] = val,
            0x0400_1016 => self.engine_b.bgvofs[1] = val,
            0x0400_1018 => self.engine_b.bghofs[2] = val,
            0x0400_101A => self.engine_b.bgvofs[2] = val,
            0x0400_101C => self.engine_b.bghofs[3] = val,
            0x0400_101E => self.engine_b.bgvofs[3] = val,
            0x0400_1050 => self.engine_b.bldcnt = val,
            0x0400_1052 => self.engine_b.bldalpha = val,
            0x0400_1054 => self.engine_b.bldy = val,
            0x0400_106C => self.engine_b.master_bright = val,
            _ => {}
        }
    }
}
