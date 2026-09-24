//! GBA Picture Processing Unit (PPU)
#![allow(clippy::needless_range_loop)]

pub mod bg;
pub mod blend;
pub mod hd_mode7;
pub mod layers;
pub mod obj;

use bg::{render_affine_bg, render_bitmap_bg, render_text_bg};
use blend::{apply_color_effects, bgr555_to_rgb888, Pixel};
pub use hd_mode7::{render_hd_mode7, HdFrame, HdMode7Config, HdScale};
pub use layers::{LayerDrawCommand, LayerKind, PpuLayer, PpuLayerBuffers};
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

    // Mosaic: bits 0-3 BG H-size, 4-7 BG V-size, 8-11 OBJ H-size, 12-15 OBJ V-size
    // (stored value + 1 = block size in pixels)
    pub mosaic: u16,

    // Scanline timing state
    pub cycle_in_scanline: u32,
    pub frame_ready: bool,
    // Layer toggle mask: bit 0..3 = BG0..3, bit 4 = OBJ (default 0x1F = all enabled)
    pub layer_mask: u8,

    // Front/back RGBA8888 framebuffers (240x160)
    pub framebuffer: Box<[u32; SCREEN_WIDTH * SCREEN_HEIGHT]>,
    /// Last complete frame, copied at VBlank. `run_frame` stops on a cycle
    /// count, not at VBlank, so `framebuffer` is usually part new frame and
    /// part old: showing it tears wherever the frame stopped.
    pub completed_frame: Box<[u32; SCREEN_WIDTH * SCREEN_HEIGHT]>,

    // Per-layer capture and draw commands (ROADMAP M5)
    pub layer_capture: bool,
    pub layer_buffers: Option<PpuLayerBuffers>,
    pub draw_commands: Vec<LayerDrawCommand>,
    /// Layers and commands of the last *completed* frame, latched at VBlank.
    /// `run_frame` runs a fixed cycle count, so it doesn't end at VBlank:
    /// its phase drifts and a loaded save state can start mid-frame. The live
    /// buffers above are then only partly drawn when the HD renderers run,
    /// which showed up as a black lower part of the HD picture.
    pub completed_layers: Option<PpuLayerBuffers>,
    pub completed_commands: Vec<LayerDrawCommand>,

    // HD Mode 7 Rendering (ROADMAP M6)
    pub hd_config: HdMode7Config,

    // HD Sprite and Tile Replacement Packs (ROADMAP M7)
    pub hd_pack: Option<crate::gba::hd_pack::HdPack>,
    pub hd_pack_enabled: bool,

    // Per-Game Widescreen (ROADMAP M8)
    pub widescreen_config: crate::gba::widescreen::WidescreenConfig,
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
            mosaic: 0,
            cycle_in_scanline: 0,
            frame_ready: false,
            layer_mask: 0x1F,
            framebuffer: Box::new([0xFF00_0000; SCREEN_WIDTH * SCREEN_HEIGHT]),
            completed_frame: Box::new([0xFF00_0000; SCREEN_WIDTH * SCREEN_HEIGHT]),
            layer_capture: false,
            layer_buffers: None,
            completed_layers: None,
            completed_commands: Vec::new(),
            draw_commands: Vec::new(),
            hd_config: HdMode7Config::default(),
            hd_pack: None,
            hd_pack_enabled: true,
            widescreen_config: crate::gba::widescreen::WidescreenConfig::default(),
        }
    }

    /// Enable or disable isolated per-layer framebuffer capture and draw command emission (ROADMAP M5).
    pub fn set_layer_capture(&mut self, enable: bool) {
        self.layer_capture = enable;
        if enable && self.layer_buffers.is_none() {
            self.layer_buffers = Some(PpuLayerBuffers::new());
        } else if !enable {
            self.layer_buffers = None;
            self.draw_commands.clear();
            self.completed_layers = None;
            self.completed_commands.clear();
        }
    }

    /// Configure HD Mode 7 rendering (ROADMAP M6).
    pub fn set_hd_mode7_config(&mut self, config: HdMode7Config) {
        if config.scale != HdScale::Off {
            self.set_layer_capture(true);
        }
        self.hd_config = config;
    }

    /// Load an HD Sprite and Tile replacement pack (ROADMAP M7).
    pub fn load_hd_pack(&mut self, pack: crate::gba::hd_pack::HdPack) {
        self.set_layer_capture(true);
        self.hd_pack = Some(pack);
    }

    /// Toggle HD replacement pack active state.
    pub fn set_hd_pack_enabled(&mut self, enabled: bool) {
        if enabled && self.hd_pack.is_some() {
            self.set_layer_capture(true);
        }
        self.hd_pack_enabled = enabled;
    }

    /// Query if an HD replacement pack is actively loaded and enabled.
    pub fn is_hd_pack_enabled(&self) -> bool {
        self.hd_pack_enabled && self.hd_pack.as_ref().map_or(false, |p| p.enabled && !p.is_empty())
    }

    /// Borrow loaded HD replacement pack if present.
    pub fn hd_pack(&self) -> Option<&crate::gba::hd_pack::HdPack> {
        self.hd_pack.as_ref()
    }

    /// Mutably borrow loaded HD replacement pack if present.
    pub fn hd_pack_mut(&mut self) -> Option<&mut crate::gba::hd_pack::HdPack> {
        self.hd_pack.as_mut()
    }

    /// Configure Widescreen rendering (ROADMAP M8).
    pub fn set_widescreen_config(&mut self, config: crate::gba::widescreen::WidescreenConfig) {
        if config.enabled && config.mode != crate::gba::widescreen::WidescreenMode::Off {
            self.set_layer_capture(true);
        }
        self.widescreen_config = config;
    }

    /// Toggle widescreen active state.
    pub fn set_widescreen_enabled(&mut self, enabled: bool) {
        if enabled && self.widescreen_config.mode != crate::gba::widescreen::WidescreenMode::Off {
            self.set_layer_capture(true);
        }
        self.widescreen_config.enabled = enabled;
    }

    /// Query if widescreen rendering is actively enabled.
    pub fn is_widescreen_enabled(&self) -> bool {
        self.widescreen_config.enabled && self.widescreen_config.mode != crate::gba::widescreen::WidescreenMode::Off
    }

    /// Borrow active widescreen config.
    pub fn widescreen_config(&self) -> &crate::gba::widescreen::WidescreenConfig {
        &self.widescreen_config
    }

    /// Mutably borrow active widescreen config.
    pub fn widescreen_config_mut(&mut self) -> &mut crate::gba::widescreen::WidescreenConfig {
        &mut self.widescreen_config
    }

    /// Render widescreen frame directly.
    pub fn render_widescreen_frame(&self) -> Option<HdFrame> {
        crate::gba::widescreen::render_widescreen(self, &self.widescreen_config, &self.hd_config)
    }

    /// Renders an HD Mode 7, HD Pack, or Widescreen frame if active.
    pub fn render_hd_frame(&self) -> Option<HdFrame> {
        if self.is_widescreen_enabled() {
            crate::gba::widescreen::render_widescreen(self, &self.widescreen_config, &self.hd_config)
        } else {
            render_hd_mode7(self, &self.hd_config)
        }
    }

    /// Access isolated RGBA framebuffer surface for a specific layer.
    pub fn get_layer_framebuffer(&self, layer: PpuLayer) -> Option<&[u32; SCREEN_WIDTH * SCREEN_HEIGHT]> {
        self.frame_layers().map(|lb| lb.get_layer(layer))
    }

    /// Access all isolated layer framebuffers.
    pub fn get_layer_framebuffers(&self) -> Option<&PpuLayerBuffers> {
        self.frame_layers()
    }

    /// All 160 lines are drawn: keep this frame's layers and commands for
    /// the HD renderers. Swaps buffers (no copying); the live ones are
    /// cleared when line 0 of the next frame is drawn.
    fn latch_completed_frame(&mut self) {
        if !self.layer_capture {
            return;
        }
        if self.completed_layers.is_none() {
            self.completed_layers = Some(PpuLayerBuffers::new());
        }
        std::mem::swap(&mut self.layer_buffers, &mut self.completed_layers);
        std::mem::swap(&mut self.draw_commands, &mut self.completed_commands);
    }

    /// Layers of the last complete frame (live buffers until one exists).
    pub fn frame_layers(&self) -> Option<&PpuLayerBuffers> {
        self.completed_layers.as_ref().or(self.layer_buffers.as_ref())
    }

    /// Draw commands of the last complete frame (live until one exists).
    pub fn frame_commands(&self) -> &[LayerDrawCommand] {
        if self.completed_layers.is_some() {
            &self.completed_commands
        } else {
            &self.draw_commands
        }
    }

    /// Access recorded draw commands for the current frame.
    pub fn get_draw_commands(&self) -> &[LayerDrawCommand] {
        self.frame_commands()
    }

    pub fn bg_mosaic_h(&self) -> u32 {
        (self.mosaic & 0xF) as u32 + 1
    }

    pub fn bg_mosaic_v(&self) -> u32 {
        ((self.mosaic >> 4) & 0xF) as u32 + 1
    }

    pub fn obj_mosaic_h(&self) -> u32 {
        ((self.mosaic >> 8) & 0xF) as u32 + 1
    }

    pub fn obj_mosaic_v(&self) -> u32 {
        ((self.mosaic >> 12) & 0xF) as u32 + 1
    }

    /// Returns the mosaic block size for a BG layer if BGCNT bit 6 (mosaic
    /// enable) is set, or `None` otherwise.
    fn bg_mosaic_for(&self, bgcnt: u16) -> Option<(u32, u32)> {
        if (bgcnt & (1 << 6)) != 0 {
            Some((self.bg_mosaic_h(), self.bg_mosaic_v()))
        } else {
            None
        }
    }

    /// Advances PPU by elapsed cycles. Returns (irq_vblank, irq_hblank, irq_vcounter, dma_vblank, dma_hblank)
    /// Cycles until the next HBlank start or scanline end.
    pub fn cycles_to_next_boundary(&self) -> u32 {
        if self.cycle_in_scanline < HDRAW_CYCLES {
            HDRAW_CYCLES - self.cycle_in_scanline
        } else {
            SCANLINE_CYCLES.saturating_sub(self.cycle_in_scanline).max(1)
        }
    }

    pub fn step(&mut self, cycles: u32) -> (bool, bool, bool, bool, bool) {
        let mut irq_vblank = false;
        let mut irq_hblank = false;
        let mut irq_vcounter = false;
        let mut dma_vblank = false;
        let mut dma_hblank = false;

        // Consume the cycle budget in pieces that never cross more than one
        // scanline boundary.
        //
        // `step()` is called once per instruction with that instruction's cycle
        // count, and a long DMA burst or slow instruction can hand us several
        // scanlines' worth at once. The previous code did a single
        // `if cycle_in_scanline >= SCANLINE_CYCLES { -= SCANLINE_CYCLES }`,
        // which advances vcount at most once per call -- so large chunks
        // silently dropped scanlines (measured: 46 of 160 visible lines never
        // rendered at a 2464-cycle chunk). That is what produced the torn,
        // mostly-black battle transition in Pokemon Emerald.
        let mut remaining = cycles;
        while remaining > 0 {
            // How many cycles until the next event boundary on this line?
            let next_boundary = if self.cycle_in_scanline < HDRAW_CYCLES {
                HDRAW_CYCLES - self.cycle_in_scanline
            } else {
                SCANLINE_CYCLES - self.cycle_in_scanline
            };
            let chunk = remaining.min(next_boundary.max(1));
            remaining -= chunk;
            self.cycle_in_scanline += chunk;

            // Check HBlank transition (at cycle 960)
            let old_hblank = (self.dispstat & 2) != 0;
            let in_hblank = self.cycle_in_scanline >= HDRAW_CYCLES;

            // HBlank flag/IRQ/DMA fire on all 228 scanlines, including during
            // VBlank (lines 160-227) -- real hardware does not suppress HBlank
            // there, and several games rely on HBlank DMA/IRQ continuing
            // through VBlank.
            if !old_hblank && in_hblank {
                // Render the visible scanline HERE, at the moment HDraw ends
                // and before the HBlank IRQ handler runs.
                //
                // Games drive raster effects (windows, scroll, blend) by
                // rewriting PPU registers from the HBlank IRQ to set up the
                // NEXT line. Rendering at the *end* of the scanline instead
                // would use values the handler already advanced -- Pokemon
                // Emerald's battle-entry transition rewrites WIN0H every 1232
                // cycles (exactly one scanline), so that timing error shows up
                // directly as a broken transition.
                if self.vcount < 160 {
                    self.render_scanline(self.vcount as u32);
                }

                self.dispstat |= 2;
                if (self.dispstat & (1 << 4)) != 0 {
                    irq_hblank = true;
                }
                dma_hblank = true;
            }

            if self.cycle_in_scanline >= SCANLINE_CYCLES {
                self.cycle_in_scanline -= SCANLINE_CYCLES;
                self.dispstat &= !2; // Exit HBlank

                self.vcount += 1;
                if self.vcount == 160 {
                    // Entering VBlank
                    self.dispstat |= 1;
                    self.frame_ready = true;
                    self.completed_frame.copy_from_slice(&self.framebuffer[..]);
                    self.latch_completed_frame();

                    if (self.dispstat & (1 << 3)) != 0 {
                        irq_vblank = true;
                    }
                    dma_vblank = true;
                } else if self.vcount == 227 {
                    // On scanline 227, VBlank flag in DISPSTAT is cleared
                    // according to GBA specs
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
        }

        (irq_vblank, irq_hblank, irq_vcounter, dma_vblank, dma_hblank)
    }

    pub fn render_scanline(&mut self, y: u32) {
        if y == 0 && self.layer_capture {
            if let Some(ref mut lb) = self.layer_buffers {
                lb.clear();
            }
            self.draw_commands.clear();
        }

        if (self.dispcnt & (1 << 7)) != 0 {
            // Forced blank: render white
            let row = (y as usize) * SCREEN_WIDTH;
            for x in 0..SCREEN_WIDTH {
                self.framebuffer[row + x] = 0xFFFF_FFFF;
            }
            if self.layer_capture {
                if let Some(ref mut lb) = self.layer_buffers {
                    let y_us = y as usize;
                    let b_slice = &mut lb.buffers[PpuLayer::Backdrop.index()][y_us * SCREEN_WIDTH..(y_us + 1) * SCREEN_WIDTH];
                    b_slice.fill(0xFFFF_FFFF);
                }
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

        let scanline_affine_origin = [
            [self.bg_x_internal[0], self.bg_y_internal[0]],
            [self.bg_x_internal[1], self.bg_y_internal[1]],
        ];

        // Render BG layers based on mode
        match mode {
            0 => {
                // Mode 0: Text BG 0, 1, 2, 3
                for i in 0..4 {
                    if (self.dispcnt & (1 << (8 + i))) != 0 && (self.layer_mask & (1 << i)) != 0 {
                        let mosaic = self.bg_mosaic_for(self.bgcnt[i]);
                        render_text_bg(
                            i as u8,
                            y,
                            self.dispcnt,
                            self.bgcnt[i],
                            self.bghofs[i],
                            self.bgvofs[i],
                            &self.vram[..],
                            &self.palette_ram[..],
                            mosaic,
                            &mut bg_layer_bufs[i],
                        );
                    }
                }
            }
            1 => {
                // Mode 1: BG 0, 1 Text, BG 2 Affine
                for i in 0..2 {
                    if (self.dispcnt & (1 << (8 + i))) != 0 && (self.layer_mask & (1 << i)) != 0 {
                        let mosaic = self.bg_mosaic_for(self.bgcnt[i]);
                        render_text_bg(
                            i as u8,
                            y,
                            self.dispcnt,
                            self.bgcnt[i],
                            self.bghofs[i],
                            self.bgvofs[i],
                            &self.vram[..],
                            &self.palette_ram[..],
                            mosaic,
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
                        self.bg_mosaic_for(self.bgcnt[2]),
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
                            self.bg_mosaic_for(self.bgcnt[bg_idx]),
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
                    let mosaic = self.bg_mosaic_for(self.bgcnt[2]);
                    render_bitmap_bg(
                        mode,
                        frame,
                        y,
                        &self.vram[..],
                        &self.palette_ram[..],
                        self.bgcnt[2],
                        mosaic,
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
                (self.obj_mosaic_h(), self.obj_mosaic_v()),
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

        if self.layer_capture {
            let (br, bg, bb) = bgr555_to_rgb888(backdrop_color);
            let backdrop_rgba = 0xFF00_0000 | ((bb as u32) << 16) | ((bg as u32) << 8) | (br as u32);
            if let Some(ref mut lb) = self.layer_buffers {
                let y_us = y as usize;
                let b_slice = &mut lb.buffers[PpuLayer::Backdrop.index()][y_us * SCREEN_WIDTH..(y_us + 1) * SCREEN_WIDTH];
                b_slice.fill(backdrop_rgba);

                // Populate BG0..3
                let layers = [PpuLayer::Bg0, PpuLayer::Bg1, PpuLayer::Bg2, PpuLayer::Bg3];
                for i in 0..4 {
                    let bg_buf = &mut lb.buffers[layers[i].index()];
                    for x in 0..SCREEN_WIDTH {
                        let pix = &bg_layer_bufs[i][x];
                        if !pix.is_transparent {
                            let (r, g, b) = bgr555_to_rgb888(pix.color);
                            bg_buf[y_us * SCREEN_WIDTH + x] = 0xFF00_0000 | ((b as u32) << 16) | ((g as u32) << 8) | (r as u32);
                        }
                    }
                }

                // Populate OBJ
                let obj_layer_buf = &mut lb.buffers[PpuLayer::Obj.index()];
                for x in 0..SCREEN_WIDTH {
                    let pix = &obj_buf[x];
                    if !pix.is_transparent {
                        let (r, g, b) = bgr555_to_rgb888(pix.color);
                        obj_layer_buf[y_us * SCREEN_WIDTH + x] = 0xFF00_0000 | ((b as u32) << 16) | ((g as u32) << 8) | (r as u32);
                    }
                }
            }

            // Record draw commands for the scanline
            let bldcnt_mode = ((self.bldcnt >> 6) & 3) as u8;
            for i in 0..4 {
                if (self.dispcnt & (1 << (8 + i))) != 0 {
                    let kind = match mode {
                        0 => LayerKind::Text,
                        1 if i < 2 => LayerKind::Text,
                        1 if i == 2 => LayerKind::Affine,
                        2 if i >= 2 => LayerKind::Affine,
                        3..=5 if i == 2 => LayerKind::Bitmap,
                        _ => LayerKind::Text,
                    };
                    let (matrix, origin) = if kind == LayerKind::Affine && (i == 2 || i == 3) {
                        let aff_idx = i - 2;
                        ([self.bg_pa[aff_idx], self.bg_pb[aff_idx], self.bg_pc[aff_idx], self.bg_pd[aff_idx]],
                         scanline_affine_origin[aff_idx])
                    } else if kind == LayerKind::Bitmap {
                        ([self.bg_pa[0], self.bg_pb[0], self.bg_pc[0], self.bg_pd[0]],
                         scanline_affine_origin[0])
                    } else {
                        ([0x0100, 0, 0, 0x0100], [0, 0])
                    };
                    self.draw_commands.push(LayerDrawCommand {
                        scanline: y as u16,
                        layer: match i {
                            0 => PpuLayer::Bg0,
                            1 => PpuLayer::Bg1,
                            2 => PpuLayer::Bg2,
                            _ => PpuLayer::Bg3,
                        },
                        kind,
                        priority: (self.bgcnt[i] & 3) as u8,
                        bgcnt: self.bgcnt[i],
                        h_offset: self.bghofs[i],
                        v_offset: self.bgvofs[i],
                        affine_matrix: matrix,
                        affine_origin: origin,
                        blend_mode: bldcnt_mode,
                        bldcnt: self.bldcnt,
                        bldalpha: self.bldalpha,
                        bldy: self.bldy,
                        window_enabled: any_win,
                    });
                }
            }
            if (self.dispcnt & (1 << 12)) != 0 {
                self.draw_commands.push(LayerDrawCommand {
                    scanline: y as u16,
                    layer: PpuLayer::Obj,
                    kind: LayerKind::Obj,
                    priority: 0,
                    bgcnt: 0,
                    h_offset: 0,
                    v_offset: 0,
                    affine_matrix: [0x0100, 0, 0, 0x0100],
                    affine_origin: [0, 0],
                    blend_mode: bldcnt_mode,
                    bldcnt: self.bldcnt,
                    bldalpha: self.bldalpha,
                    bldy: self.bldy,
                    window_enabled: any_win,
                });
            }
        }
    }
}

/// PPU registers and internal state (ROADMAP M2). VRAM, palette and OAM are
/// part of the v1 layout; the framebuffer is not saved (the next frame
/// redraws it).
impl crate::gba::state::Snapshot for Ppu {
    fn save(&self, w: &mut crate::gba::state::StateWriter) {
        for v in [self.dispcnt, self.dispstat, self.vcount] { w.u16(v); }
        for i in 0..4 { w.u16(self.bgcnt[i]); w.u16(self.bghofs[i]); w.u16(self.bgvofs[i]); }
        for i in 0..2 {
            for v in [self.bg_pa[i], self.bg_pb[i], self.bg_pc[i], self.bg_pd[i]] { w.u16(v as u16); }
            for v in [self.bg_x[i], self.bg_y[i], self.bg_x_internal[i], self.bg_y_internal[i]] { w.i32(v); }
        }
        for v in [self.win0h, self.win1h, self.win0v, self.win1v, self.winin, self.winout,
                  self.bldcnt, self.bldalpha, self.bldy, self.mosaic] { w.u16(v); }
        w.u32(self.cycle_in_scanline);
        w.bool(self.frame_ready);
    }
    fn load(&mut self, r: &mut crate::gba::state::StateReader) -> Option<()> {
        self.dispcnt = r.u16()?; self.dispstat = r.u16()?; self.vcount = r.u16()?;
        for i in 0..4 { self.bgcnt[i] = r.u16()?; self.bghofs[i] = r.u16()?; self.bgvofs[i] = r.u16()?; }
        for i in 0..2 {
            self.bg_pa[i] = r.u16()? as i16; self.bg_pb[i] = r.u16()? as i16;
            self.bg_pc[i] = r.u16()? as i16; self.bg_pd[i] = r.u16()? as i16;
            self.bg_x[i] = r.i32()?; self.bg_y[i] = r.i32()?;
            self.bg_x_internal[i] = r.i32()?; self.bg_y_internal[i] = r.i32()?;
        }
        self.win0h = r.u16()?; self.win1h = r.u16()?; self.win0v = r.u16()?; self.win1v = r.u16()?;
        self.winin = r.u16()?; self.winout = r.u16()?;
        self.bldcnt = r.u16()?; self.bldalpha = r.u16()?; self.bldy = r.u16()?; self.mosaic = r.u16()?;
        self.cycle_in_scanline = r.u32()?;
        self.frame_ready = r.bool()?;
        Some(())
    }
}
