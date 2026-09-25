//! Nintendo DS 2D Graphics Engine (Engine A / Engine B)
//!
//! Per GBATEK "DS Video": text, affine, extended (rot/scal 16-bit map, 256
//! colour and direct colour bitmap) and large-bitmap backgrounds, the 3D
//! layer as BG0, sprites (tiled 1D/2D, bitmap, affine, semi-transparent,
//! OBJ window), windows 0/1/OBJ, colour special effects, extended palettes
//! and master brightness.

pub const SCREEN_WIDTH: usize = 256;
pub const SCREEN_HEIGHT: usize = 192;

/// VRAM an engine sees while drawing a scanline.
#[derive(Clone, Copy, Default)]
pub struct EngineVram<'a> {
    pub bg: &'a [u8],
    pub obj: &'a [u8],
    /// BG extended palettes: 4 slots of 16 x 256 colours (0x2000 bytes each).
    pub bg_ext: &'a [u8],
    /// OBJ extended palette: 16 x 256 colours.
    pub obj_ext: &'a [u8],
}

/// Layer pixel: bit 31 set = opaque, low 15 bits = BGR555.
const OPAQUE: u32 = 1 << 31;
const BACKDROP: usize = 5;
const OBJ: usize = 4;

#[inline(always)]
fn rd16(buf: &[u8], off: usize) -> u16 {
    match buf.get(off..off + 2) {
        Some(b) => u16::from_le_bytes([b[0], b[1]]),
        None => 0,
    }
}

#[inline(always)]
fn rd8(buf: &[u8], off: usize) -> u8 {
    buf.get(off).copied().unwrap_or(0)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BgKind {
    Off,
    Text,
    Affine,
    Extended,
    Large,
    ThreeD,
}

/// One scanline of sprite output.
struct ObjLine {
    color: [u32; SCREEN_WIDTH],
    prio: [u8; SCREEN_WIDTH],
    /// 0 = opaque, 1..=16 = forced alpha blend with this EVA (semi-transparent
    /// or bitmap sprites).
    alpha: [u8; SCREEN_WIDTH],
    window: [bool; SCREEN_WIDTH],
}

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
    pub mosaic: u16,
    /// Last value written to each register halfword, for byte writes.
    pub shadow: [u16; 0x38],

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
            bgpa: [0x100, 0x100],
            bgpb: [0, 0],
            bgpc: [0, 0],
            bgpd: [0x100, 0x100],
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
            mosaic: 0,
            shadow: [0; 0x38],
            palette: [0; 1024],
            oam: [0; 1024],
            framebuffer: vec![0xFF000000; SCREEN_WIDTH * SCREEN_HEIGHT],
        }
    }

    /// Reload the affine reference points (start of frame).
    pub fn reset_scanline_affine(&mut self) {
        self.internal_x = self.bgx;
        self.internal_y = self.bgy;
    }

    pub fn step_scanline_affine(&mut self) {
        for i in 0..2 {
            self.internal_x[i] = self.internal_x[i].wrapping_add(self.bgpb[i] as i32);
            self.internal_y[i] = self.internal_y[i].wrapping_add(self.bgpd[i] as i32);
        }
    }

    /// Write a 16-bit engine register; `off` is relative to the engine's
    /// register base (0x0400_0000 or 0x0400_1000).
    pub fn write_reg16(&mut self, off: u32, val: u16) {
        if let Some(r) = self.shadow.get_mut((off / 2) as usize) {
            *r = val;
        }
        // 28-bit signed reference point halves.
        fn set_ref(r: &mut i32, hi: bool, val: u16) {
            let raw = *r as u32 & 0x0FFF_FFFF;
            let raw = if hi { (raw & 0xFFFF) | (((val & 0x0FFF) as u32) << 16) } else { (raw & 0x0FFF_0000) | val as u32 };
            *r = ((raw << 4) as i32) >> 4;
        }
        match off {
            0x00 => self.dispcnt = (self.dispcnt & 0xFFFF_0000) | val as u32,
            0x02 => self.dispcnt = (self.dispcnt & 0x0000_FFFF) | ((val as u32) << 16),
            0x08 | 0x0A | 0x0C | 0x0E => self.bgcnt[((off - 0x08) / 2) as usize] = val,
            0x10..=0x1E => {
                let i = ((off - 0x10) / 4) as usize;
                if off & 2 == 0 {
                    self.bghofs[i] = val & 0x1FF;
                } else {
                    self.bgvofs[i] = val & 0x1FF;
                }
            }
            0x20..=0x3E => {
                let i = ((off - 0x20) / 0x10) as usize;
                match off & 0xF {
                    0x0 => self.bgpa[i] = val as i16,
                    0x2 => self.bgpb[i] = val as i16,
                    0x4 => self.bgpc[i] = val as i16,
                    0x6 => self.bgpd[i] = val as i16,
                    0x8 | 0xA => {
                        set_ref(&mut self.bgx[i], off & 2 != 0, val);
                        self.internal_x[i] = self.bgx[i];
                    }
                    _ => {
                        set_ref(&mut self.bgy[i], off & 2 != 0, val);
                        self.internal_y[i] = self.bgy[i];
                    }
                }
            }
            0x40 => self.win0h = val,
            0x42 => self.win1h = val,
            0x44 => self.win0v = val,
            0x46 => self.win1v = val,
            0x48 => self.winin = val,
            0x4A => self.winout = val,
            0x4C => self.mosaic = val,
            0x50 => self.bldcnt = val,
            0x52 => self.bldalpha = val,
            0x54 => self.bldy = val,
            0x6C => self.master_bright = val,
            _ => {}
        }
    }

    /// Byte write: merged into the last halfword written.
    pub fn write_reg8(&mut self, off: u32, val: u8) {
        let old = self.shadow.get((off / 2) as usize).copied().unwrap_or(0);
        let sh = (off & 1) * 8;
        self.write_reg16(off & !1, (old & !(0xFF << sh)) | ((val as u16) << sh));
    }

    /// Read a 16-bit engine register (write-only ones read as 0).
    pub fn read_reg16(&self, off: u32) -> u16 {
        match off {
            0x00 => self.dispcnt as u16,
            0x02 => (self.dispcnt >> 16) as u16,
            0x08 | 0x0A | 0x0C | 0x0E => self.bgcnt[((off - 0x08) / 2) as usize],
            0x48 => self.winin & 0x3F3F,
            0x4A => self.winout & 0x3F3F,
            0x50 => self.bldcnt & 0x3FFF,
            0x52 => self.bldalpha & 0x1F1F,
            0x6C => self.master_bright & 0xC01F,
            _ => 0,
        }
    }

    /// Convert 15-bit BGR555 to 32-bit RGBA (0xAABBGGRR)
    #[inline(always)]
    pub fn bgr555_to_rgba(c: u16) -> u32 {
        let r = ((c & 0x1F) as u32) << 3;
        let g = (((c >> 5) & 0x1F) as u32) << 3;
        let b = (((c >> 10) & 0x1F) as u32) << 3;
        let r = r | (r >> 5);
        let g = g | (g >> 5);
        let b = b | (b >> 5);
        0xFF000000 | (b << 16) | (g << 8) | r
    }

    #[inline(always)]
    pub fn read_bg_palette(&self, index: usize) -> u16 {
        let offset = (index * 2) & 0x1FE;
        u16::from_le_bytes([self.palette[offset], self.palette[offset + 1]])
    }

    #[inline(always)]
    pub fn read_obj_palette(&self, index: usize) -> u16 {
        let offset = 0x200 + ((index * 2) & 0x1FE);
        u16::from_le_bytes([self.palette[offset], self.palette[offset + 1]])
    }

    fn bg_kind(&self, i: usize) -> BgKind {
        let mode = self.dispcnt & 7;
        if i == 0 && self.is_main_engine && self.dispcnt & (1 << 3) != 0 {
            return BgKind::ThreeD;
        }
        match (mode, i) {
            (0..=6, 0) | (0..=5, 1) => BgKind::Text,
            (0 | 1 | 3, 2) | (0, 3) => BgKind::Text,
            (2 | 4, 2) | (1 | 2, 3) => BgKind::Affine,
            (5, 2) | (3..=5, 3) => BgKind::Extended,
            (6, 2) if self.is_main_engine => BgKind::Large,
            _ => BgKind::Off,
        }
    }

    /// Render a single scanline (0..191). `layer3d` is the 3D engine's line
    /// (RGBA8888), used as BG0 on the main engine when DISPCNT bit 3 is set.
    pub fn render_scanline(&mut self, line: usize, vram: &EngineVram<'_>, layer3d: Option<&[u32]>) {
        if line >= SCREEN_HEIGHT {
            return;
        }
        let row_start = line * SCREEN_WIDTH;
        if self.dispcnt & (1 << 7) != 0 {
            self.framebuffer[row_start..row_start + SCREEN_WIDTH].fill(0xFFFF_FFFF);
            return;
        }

        let mut bgs = [[0u32; SCREEN_WIDTH]; 4];
        let mut alpha3d = [31u8; SCREEN_WIDTH];
        let mut bg_on = [false; 4];
        for (i, out) in bgs.iter_mut().enumerate() {
            if self.dispcnt & (1 << (8 + i)) == 0 {
                continue;
            }
            match self.bg_kind(i) {
                BgKind::Off => continue,
                BgKind::Text => self.render_text(i, line, vram, out),
                BgKind::Affine | BgKind::Extended | BgKind::Large => self.render_affine(i, vram, out),
                BgKind::ThreeD => {
                    if let Some(l3d) = layer3d {
                        for (x, px) in out.iter_mut().enumerate() {
                            let c = l3d.get(line * SCREEN_WIDTH + x).copied().unwrap_or(0);
                            let a = c >> 24;
                            if a != 0 {
                                let (r, g, b) = ((c & 0xFF) >> 3, ((c >> 8) & 0xFF) >> 3, ((c >> 16) & 0xFF) >> 3);
                                *px = OPAQUE | (b << 10) | (g << 5) | r;
                                alpha3d[x] = (a >> 3) as u8;
                            }
                        }
                    }
                }
            }
            bg_on[i] = true;
        }

        let mut obj = ObjLine {
            color: [0; SCREEN_WIDTH],
            prio: [4; SCREEN_WIDTH],
            alpha: [0; SCREEN_WIDTH],
            window: [false; SCREEN_WIDTH],
        };
        if self.dispcnt & (1 << 12) != 0 {
            self.render_sprites(line, vram, &mut obj);
        }

        let backdrop = self.read_bg_palette(0) as u32;
        let any_window = self.dispcnt & 0xE000 != 0;
        let in_v = |reg: u16| {
            let (y1, y2) = ((reg >> 8) as usize, (reg & 0xFF) as usize);
            if y1 <= y2 { line >= y1 && line < y2 } else { line >= y1 || line < y2 }
        };
        let win0_v = self.dispcnt & (1 << 13) != 0 && in_v(self.win0v);
        let win1_v = self.dispcnt & (1 << 14) != 0 && in_v(self.win1v);
        let in_h = |reg: u16, x: usize| {
            let (x1, x2) = ((reg >> 8) as usize, (reg & 0xFF) as usize);
            if x1 <= x2 { x >= x1 && x < x2 } else { x >= x1 || x < x2 }
        };

        let bg_prio: [u8; 4] = std::array::from_fn(|i| (self.bgcnt[i] & 3) as u8);
        let effect = (self.bldcnt >> 6) & 3;
        let eva = (self.bldalpha & 0x1F).min(16) as u32;
        let evb = ((self.bldalpha >> 8) & 0x1F).min(16) as u32;
        let evy = (self.bldy & 0x1F).min(16) as u32;

        let mut out = [0u32; SCREEN_WIDTH];
        for x in 0..SCREEN_WIDTH {
            let mask: u16 = if !any_window {
                0x3F
            } else if win0_v && in_h(self.win0h, x) {
                self.winin & 0x3F
            } else if win1_v && in_h(self.win1h, x) {
                (self.winin >> 8) & 0x3F
            } else if self.dispcnt & (1 << 15) != 0 && obj.window[x] {
                (self.winout >> 8) & 0x3F
            } else {
                self.winout & 0x3F
            };

            // Top two layers: (layer id, colour).
            let mut top = (BACKDROP, backdrop);
            let mut second = (BACKDROP, backdrop);
            let mut found = 0;
            'prio: for p in 0..4u8 {
                // Sprites win ties with backgrounds; lower BG numbers win ties.
                let obj_here = mask & (1 << OBJ) != 0 && obj.prio[x] == p && obj.color[x] & OPAQUE != 0;
                let layers = std::iter::once(OBJ).filter(|_| obj_here).chain(
                    (0..4).filter(|&i| bg_on[i] && bg_prio[i] == p && mask & (1 << i) != 0 && bgs[i][x] & OPAQUE != 0),
                );
                for l in layers {
                    let c = if l == OBJ { obj.color[x] } else { bgs[l][x] };
                    if found == 0 {
                        top = (l, c);
                        found = 1;
                    } else {
                        second = (l, c);
                        break 'prio;
                    }
                }
            }

            let blend = |a: u32, b: u32, ea: u32, eb: u32| -> u32 {
                let ch = |s: u32| (((a >> s) & 0x1F) * ea + ((b >> s) & 0x1F) * eb) / 16;
                ch(0).min(31) | (ch(5).min(31) << 5) | (ch(10).min(31) << 10)
            };
            let second_target = self.bldcnt & (1 << (8 + second.0)) != 0;
            let top_target = self.bldcnt & (1 << top.0) != 0;
            let sfx = mask & 0x20 != 0;
            let mut c = top.1 & 0x7FFF;
            if top.0 == OBJ && obj.alpha[x] != 0 && second_target {
                let a = obj.alpha[x] as u32;
                let (ea, eb) = if a > 16 { (eva, evb) } else { (a, 16 - a) };
                c = blend(c, second.1, ea, eb);
            } else if top.0 == 0 && self.bg_kind(0) == BgKind::ThreeD && alpha3d[x] < 31 && second_target {
                let a = alpha3d[x] as u32 + 1;
                c = blend(c, second.1, a / 2, 16 - a / 2);
            } else if sfx && top_target {
                c = match effect {
                    1 if second_target => blend(c, second.1, eva, evb),
                    2 => blend(c, 0x7FFF, 16 - evy, evy),
                    3 => blend(c, 0, 16 - evy, evy),
                    _ => c,
                };
            }
            out[x] = Self::bgr555_to_rgba(c as u16);
        }

        // Master brightness
        let bright_mode = (self.master_bright >> 14) & 3;
        let factor = (self.master_bright & 0x1F).min(16) as u32;
        if factor > 0 && (bright_mode == 1 || bright_mode == 2) {
            for px in out.iter_mut() {
                let ch = |s: u32| {
                    let v = (*px >> s) & 0xFF;
                    if bright_mode == 1 { v + ((255 - v) * factor) / 16 } else { v - (v * factor) / 16 }
                };
                *px = 0xFF00_0000 | (ch(16) << 16) | (ch(8) << 8) | ch(0);
            }
        }

        self.framebuffer[row_start..row_start + SCREEN_WIDTH].copy_from_slice(&out);
    }

    #[inline(always)]
    fn bases(&self, cnt: u16) -> (usize, usize) {
        let (cb, sb) = if self.is_main_engine {
            (((self.dispcnt >> 24) & 7) as usize * 0x10000, ((self.dispcnt >> 27) & 7) as usize * 0x10000)
        } else {
            (0, 0)
        };
        (cb + ((cnt >> 2) & 0xF) as usize * 0x4000, sb + ((cnt >> 8) & 0x1F) as usize * 0x800)
    }

    /// 256-colour lookup: extended palette `slot` if enabled, else the
    /// standard BG palette.
    #[inline(always)]
    fn bg_color256(&self, vram: &EngineVram<'_>, slot: usize, pal: usize, idx: usize) -> u16 {
        if self.dispcnt & (1 << 30) != 0 {
            rd16(vram.bg_ext, slot * 0x2000 + pal * 0x200 + idx * 2)
        } else {
            self.read_bg_palette(idx)
        }
    }

    fn render_text(&self, i: usize, line: usize, vram: &EngineVram<'_>, out: &mut [u32; SCREEN_WIDTH]) {
        let cnt = self.bgcnt[i];
        let (char_base, screen_base) = self.bases(cnt);
        let bpp8 = cnt & (1 << 7) != 0;
        let (w, h) = [(256, 256), (512, 256), (256, 512), (512, 512)][(cnt >> 14) as usize];
        let ext_slot = if i < 2 && cnt & (1 << 13) != 0 { i + 2 } else { i };
        let y = (self.bgvofs[i] as usize + line) % h;
        let mut y_block = screen_base + ((y % 256) / 8) * 64;
        if y >= 256 {
            y_block += if w == 512 { 0x1000 } else { 0x800 };
        }
        let hofs = self.bghofs[i] as usize;

        let mut x = 0;
        while x < SCREEN_WIDTH {
            let px = (hofs + x) % w;
            let mut map = y_block + ((px % 256) / 8) * 2;
            if px >= 256 {
                map += 0x800;
            }
            let entry = rd16(vram.bg, map) as usize;
            let tile = entry & 0x3FF;
            let pal = entry >> 12;
            let fy = if entry & (1 << 11) != 0 { 7 - (y & 7) } else { y & 7 };
            // Draw the rest of this tile.
            let mut fx0 = px & 7;
            while fx0 < 8 && x < SCREEN_WIDTH {
                let fx = if entry & (1 << 10) != 0 { 7 - fx0 } else { fx0 };
                let color = if bpp8 {
                    let idx = rd8(vram.bg, char_base + tile * 64 + fy * 8 + fx) as usize;
                    (idx != 0).then(|| self.bg_color256(vram, ext_slot, pal, idx))
                } else {
                    let b = rd8(vram.bg, char_base + tile * 32 + fy * 4 + fx / 2);
                    let idx = if fx & 1 == 0 { b & 0xF } else { b >> 4 } as usize;
                    (idx != 0).then(|| self.read_bg_palette(pal * 16 + idx))
                };
                if let Some(c) = color {
                    out[x] = OPAQUE | c as u32;
                }
                fx0 += 1;
                x += 1;
            }
        }
    }

    /// Affine, extended and large-bitmap backgrounds (BG2/BG3).
    fn render_affine(&self, i: usize, vram: &EngineVram<'_>, out: &mut [u32; SCREEN_WIDTH]) {
        let a = i - 2;
        let cnt = self.bgcnt[i];
        let kind = self.bg_kind(i);
        let wrap = cnt & (1 << 13) != 0;
        let (char_base, screen_base) = self.bases(cnt);
        let sz = (cnt >> 14) as usize;

        // (width, height) and a sampler returning a BGR555 colour.
        enum Src {
            Affine8,
            Map16,
            Bitmap8(usize),
            Direct(usize),
            Large,
        }
        let (w, h, src) = match kind {
            BgKind::Affine => (128 << sz, 128 << sz, Src::Affine8),
            BgKind::Large => {
                if sz & 1 == 0 { (512, 1024, Src::Large) } else { (1024, 512, Src::Large) }
            }
            _ if cnt & (1 << 7) == 0 => (128 << sz, 128 << sz, Src::Map16),
            _ => {
                let (w, h) = [(128, 128), (256, 256), (512, 256), (512, 512)][sz];
                let base = ((cnt >> 8) & 0x1F) as usize * 0x4000;
                if cnt & (1 << 2) == 0 { (w, h, Src::Bitmap8(base)) } else { (w, h, Src::Direct(base)) }
            }
        };

        let (mut rx, mut ry) = (self.internal_x[a], self.internal_y[a]);
        let (pa, pc) = (self.bgpa[a] as i32, self.bgpc[a] as i32);
        for px in out.iter_mut() {
            let (mut tx, mut ty) = (rx >> 8, ry >> 8);
            rx = rx.wrapping_add(pa);
            ry = ry.wrapping_add(pc);
            if wrap {
                tx = tx.rem_euclid(w as i32);
                ty = ty.rem_euclid(h as i32);
            } else if tx < 0 || ty < 0 || tx >= w as i32 || ty >= h as i32 {
                continue;
            }
            let (tx, ty) = (tx as usize, ty as usize);
            let color = match src {
                Src::Affine8 => {
                    let tile = rd8(vram.bg, screen_base + (ty / 8) * (w / 8) + tx / 8) as usize;
                    let idx = rd8(vram.bg, char_base + tile * 64 + (ty & 7) * 8 + (tx & 7)) as usize;
                    (idx != 0).then(|| self.read_bg_palette(idx))
                }
                Src::Map16 => {
                    let entry = rd16(vram.bg, screen_base + ((ty / 8) * (w / 8) + tx / 8) * 2) as usize;
                    let fx = if entry & (1 << 10) != 0 { 7 - (tx & 7) } else { tx & 7 };
                    let fy = if entry & (1 << 11) != 0 { 7 - (ty & 7) } else { ty & 7 };
                    let idx = rd8(vram.bg, char_base + (entry & 0x3FF) * 64 + fy * 8 + fx) as usize;
                    (idx != 0).then(|| self.bg_color256(vram, i, entry >> 12, idx))
                }
                Src::Bitmap8(base) => {
                    let idx = rd8(vram.bg, base + ty * w + tx) as usize;
                    (idx != 0).then(|| self.read_bg_palette(idx))
                }
                Src::Direct(base) => {
                    let c = rd16(vram.bg, base + (ty * w + tx) * 2);
                    (c & 0x8000 != 0).then_some(c & 0x7FFF)
                }
                Src::Large => {
                    let idx = rd8(vram.bg, ty * w + tx) as usize;
                    (idx != 0).then(|| self.read_bg_palette(idx))
                }
            };
            if let Some(c) = color {
                *px = OPAQUE | c as u32;
            }
        }
    }

    fn render_sprites(&self, line: usize, vram: &EngineVram<'_>, out: &mut ObjLine) {
        const SIZES: [[(usize, usize); 4]; 3] = [
            [(8, 8), (16, 16), (32, 32), (64, 64)],
            [(16, 8), (32, 8), (32, 16), (64, 32)],
            [(8, 16), (8, 32), (16, 32), (32, 64)],
        ];
        let one_d = self.dispcnt & (1 << 4) != 0;
        let boundary = 32usize << ((self.dispcnt >> 20) & 3);
        let ext_pal = self.dispcnt & (1 << 31) != 0;

        for n in 0..128 {
            let o = n * 8;
            let a0 = u16::from_le_bytes([self.oam[o], self.oam[o + 1]]);
            let a1 = u16::from_le_bytes([self.oam[o + 2], self.oam[o + 3]]);
            let a2 = u16::from_le_bytes([self.oam[o + 4], self.oam[o + 5]]);
            let affine = a0 & (1 << 8) != 0;
            if !affine && a0 & (1 << 9) != 0 {
                continue; // disabled
            }
            let shape = (a0 >> 14) as usize;
            if shape == 3 {
                continue;
            }
            let (w, h) = SIZES[shape][(a1 >> 14) as usize];
            let (bw, bh) = if affine && a0 & (1 << 9) != 0 { (w * 2, h * 2) } else { (w, h) };
            let ly = (line as i32 - (a0 & 0xFF) as i32) & 0xFF;
            if ly as usize >= bh {
                continue;
            }
            let mode = (a0 >> 10) & 3;
            let x0 = { let x = (a1 & 0x1FF) as i32; if x >= 256 { x - 512 } else { x } };
            let prio = ((a2 >> 10) & 3) as u8;
            let tile = (a2 & 0x3FF) as usize;
            let pal = (a2 >> 12) as usize;
            let bpp8 = a0 & (1 << 13) != 0;

            let params = if affine {
                let p = ((a1 >> 9) & 0x1F) as usize * 32;
                let rd = |k: usize| i16::from_le_bytes([self.oam[p + k * 8 + 6], self.oam[p + k * 8 + 7]]) as i32;
                Some((rd(0), rd(1), rd(2), rd(3)))
            } else {
                None
            };

            for bx in 0..bw as i32 {
                let sx = x0 + bx;
                if !(0..SCREEN_WIDTH as i32).contains(&sx) {
                    continue;
                }
                let sx = sx as usize;
                let (tx, ty) = match params {
                    Some((pa, pb, pc, pd)) => {
                        let (dx, dy) = (bx - bw as i32 / 2, ly - bh as i32 / 2);
                        let tx = ((pa * dx + pb * dy) >> 8) + w as i32 / 2;
                        let ty = ((pc * dx + pd * dy) >> 8) + h as i32 / 2;
                        if tx < 0 || ty < 0 || tx >= w as i32 || ty >= h as i32 {
                            continue;
                        }
                        (tx as usize, ty as usize)
                    }
                    None => {
                        let tx = if a1 & (1 << 12) != 0 { w - 1 - bx as usize } else { bx as usize };
                        let ty = if a1 & (1 << 13) != 0 { h - 1 - ly as usize } else { ly as usize };
                        (tx, ty)
                    }
                };

                let (color, alpha) = if mode == 3 {
                    // Bitmap sprite: direct colour, alpha from the palette field.
                    if pal == 0 {
                        continue;
                    }
                    let addr = if self.dispcnt & (1 << 6) != 0 {
                        tile * (128 << ((self.dispcnt >> 22) & 1)) + (ty * w + tx) * 2
                    } else if self.dispcnt & (1 << 5) != 0 {
                        (tile & 0x1F) * 0x10 + (tile & 0x3E0) * 0x80 + ty * 0x200 + tx * 2
                    } else {
                        (tile & 0x0F) * 0x10 + (tile & 0x3F0) * 0x80 + ty * 0x100 + tx * 2
                    };
                    let c = rd16(vram.obj, addr);
                    if c & 0x8000 == 0 {
                        continue;
                    }
                    (c & 0x7FFF, pal as u8 + 1)
                } else {
                    let (tcol, trow, fx, fy) = (tx / 8, ty / 8, tx & 7, ty & 7);
                    let idx = if bpp8 {
                        let addr = if one_d {
                            tile * boundary + (trow * (w / 8) + tcol) * 64
                        } else {
                            (tile & !1) * 32 + trow * 1024 + tcol * 64
                        };
                        rd8(vram.obj, addr + fy * 8 + fx) as usize
                    } else {
                        let addr = if one_d {
                            tile * boundary + (trow * (w / 8) + tcol) * 32
                        } else {
                            tile * 32 + trow * 1024 + tcol * 32
                        };
                        let b = rd8(vram.obj, addr + fy * 4 + fx / 2);
                        (if fx & 1 == 0 { b & 0xF } else { b >> 4 }) as usize
                    };
                    if idx == 0 {
                        continue;
                    }
                    let c = if !bpp8 {
                        self.read_obj_palette(pal * 16 + idx)
                    } else if ext_pal {
                        rd16(vram.obj_ext, pal * 0x200 + idx * 2)
                    } else {
                        self.read_obj_palette(idx)
                    };
                    // 17 = semi-transparent: use BLDALPHA.
                    (c, if mode == 1 { 17 } else { 0 })
                };

                if mode == 2 {
                    out.window[sx] = true;
                    continue;
                }
                if prio < out.prio[sx] {
                    out.color[sx] = OPAQUE | color as u32;
                    out.prio[sx] = prio;
                    out.alpha[sx] = alpha;
                }
            }
        }
    }
}
