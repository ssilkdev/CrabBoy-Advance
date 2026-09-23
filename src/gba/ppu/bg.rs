//! GBA PPU Background Layers Rendering (Modes 0 to 5)
#![allow(clippy::needless_range_loop)]
// Note: range loops used here are idiomatic for pixel rendering where the index

use super::blend::Pixel;

#[allow(clippy::too_many_arguments)]
pub fn render_text_bg(
    bg_idx: u8,
    y: u32,
    _dispcnt: u16,
    bgcnt: u16,
    hofs: u16,
    vofs: u16,
    vram: &[u8],
    palette_ram: &[u8],
    mosaic: Option<(u32, u32)>,
    line_buf: &mut [Pixel; 240],
) {
    let priority = (bgcnt & 3) as u8;
    let char_base = (((bgcnt >> 2) & 3) as usize) * 16384;
    let is_8bpp = (bgcnt & (1 << 7)) != 0;
    let screen_base = (((bgcnt >> 8) & 0x1F) as usize) * 2048;
    let screen_size = (bgcnt >> 14) & 3;

    let (map_width, map_height) = match screen_size {
        0 => (256, 256),
        1 => (512, 256),
        2 => (256, 512),
        3 => (512, 512),
        _ => (256, 256),
    };

    // Mosaic sample-and-hold: source coordinates are snapped down to the
    // nearest block boundary, but each screen pixel is still written
    // individually (the whole block just samples the same source texel).
    let sample_y = match mosaic {
        Some((_, v)) if v > 1 => y - (y % v),
        _ => y,
    };
    let scrolled_y = (sample_y + vofs as u32) % map_height;

    for x in 0..240 {
        let sample_x = match mosaic {
            Some((h, _)) if h > 1 => (x as u32) - ((x as u32) % h),
            _ => x as u32,
        };
        let scrolled_x = (sample_x + hofs as u32) % map_width;

        // Calculate screen block index based on screen size
        let block_x = scrolled_x / 256;
        let block_y = scrolled_y / 256;
        let block_offset = match screen_size {
            0 => 0,
            1 => block_x * 2048,
            2 => block_y * 2048,
            3 => (block_y * 2 + block_x) * 2048,
            _ => 0,
        };

        let tile_x = (scrolled_x % 256) / 8;
        let tile_y = (scrolled_y % 256) / 8;
        let map_entry_addr = screen_base + block_offset as usize + ((tile_y * 32 + tile_x) * 2) as usize;

        if map_entry_addr + 1 >= vram.len() {
            continue;
        }

        let map_entry = (vram[map_entry_addr] as u16) | ((vram[map_entry_addr + 1] as u16) << 8);
        let tile_num = (map_entry & 0x3FF) as usize;
        let hflip = (map_entry & (1 << 10)) != 0;
        let vflip = (map_entry & (1 << 11)) != 0;
        let pal_num = ((map_entry >> 12) & 0x0F) as usize;

        let mut py = (scrolled_y % 8) as usize;
        let mut px = (scrolled_x % 8) as usize;
        if hflip {
            px = 7 - px;
        }
        if vflip {
            py = 7 - py;
        }

        let (color_idx, is_trans) = if is_8bpp {
            let tile_addr = char_base + tile_num * 64 + py * 8 + px;
            if tile_addr < vram.len() {
                let idx = vram[tile_addr];
                (idx as usize, idx == 0)
            } else {
                (0, true)
            }
        } else {
            let tile_addr = char_base + tile_num * 32 + py * 4 + (px / 2);
            if tile_addr < vram.len() {
                let byte = vram[tile_addr];
                let idx = if px.is_multiple_of(2) { byte & 0x0F } else { (byte >> 4) & 0x0F };
                (pal_num * 16 + (idx as usize), idx == 0)
            } else {
                (0, true)
            }
        };

        if !is_trans {
            let pal_addr = color_idx * 2;
            if pal_addr + 1 < palette_ram.len() {
                let color = (palette_ram[pal_addr] as u16) | ((palette_ram[pal_addr + 1] as u16) << 8);
                if priority < line_buf[x].priority || line_buf[x].is_transparent {
                    line_buf[x] = Pixel {
                        color,
                        layer: bg_idx,
                        priority,
                        is_transparent: false,
                        is_obj_alpha: false,
                    };
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn render_affine_bg(
    bg_idx: u8,
    y: u32,
    bgcnt: u16,
    ref_x: i32,
    ref_y: i32,
    pa: i16,
    pb: i16,
    pc: i16,
    pd: i16,
    vram: &[u8],
    palette_ram: &[u8],
    mosaic: Option<(u32, u32)>,
    line_buf: &mut [Pixel; 240],
) {
    let priority = (bgcnt & 3) as u8;
    let char_base = (((bgcnt >> 2) & 3) as usize) * 16384;
    let screen_base = (((bgcnt >> 8) & 0x1F) as usize) * 2048;
    let wrap = (bgcnt & (1 << 13)) != 0;
    let size_shift = ((bgcnt >> 14) & 3) as usize; // 0=128x128, 1=256x256, 2=512x512, 3=1024x1024
    let size_px = 128 << size_shift;
    let tiles_per_row = 16 << size_shift;

    // Mosaic (ROADMAP M1): vertically, every line of a block uses the
    // reference point of the block's first line (the internal reference
    // advances by PB/PD per line, so step it back); horizontally, every
    // pixel of a block uses the sample of the block's first pixel.
    let (mos_h, mos_v) = mosaic.unwrap_or((1, 1));
    let back = (y % mos_v.max(1)) as i32;
    let mut cur_x = ref_x - back * pb as i32;
    let mut cur_y = ref_y - back * pd as i32;
    let mos_h = mos_h.max(1) as usize;
    let (mut px, mut py) = (0, 0);

    for x in 0..240 {
        if x % mos_h == 0 {
            px = cur_x >> 8;
            py = cur_y >> 8;
        }

        cur_x += pa as i32;
        cur_y += pc as i32;

        let in_bounds = px >= 0 && px < size_px && py >= 0 && py < size_px;
        if !in_bounds && !wrap {
            continue;
        }

        let map_x = px.rem_euclid(size_px) as usize;
        let map_y = py.rem_euclid(size_px) as usize;

        let tile_x = map_x / 8;
        let tile_y = map_y / 8;
        let map_entry_addr = screen_base + tile_y * tiles_per_row + tile_x;

        if map_entry_addr >= vram.len() {
            continue;
        }

        let tile_num = vram[map_entry_addr] as usize;
        let in_tile_x = map_x % 8;
        let in_tile_y = map_y % 8;

        let pixel_addr = char_base + tile_num * 64 + in_tile_y * 8 + in_tile_x;
        if pixel_addr >= vram.len() {
            continue;
        }

        let color_idx = vram[pixel_addr] as usize;
        if color_idx != 0 {
            let pal_addr = color_idx * 2;
            if pal_addr + 1 < palette_ram.len() {
                let color = (palette_ram[pal_addr] as u16) | ((palette_ram[pal_addr + 1] as u16) << 8);
                if priority < line_buf[x].priority || line_buf[x].is_transparent {
                    line_buf[x] = Pixel {
                        color,
                        layer: bg_idx,
                        priority,
                        is_transparent: false,
                        is_obj_alpha: false,
                    };
                }
            }
        }
    }
}

pub fn render_bitmap_bg(
    mode: u8,
    frame: bool,
    y: u32,
    vram: &[u8],
    palette_ram: &[u8],
    priority: u16,
    mosaic: Option<(u32, u32)>,
    line_buf: &mut [Pixel; 240],
) {
    let priority = (priority & 3) as u8;
    let sample_y = match mosaic {
        Some((_, v)) if v > 1 => y - (y % v),
        _ => y,
    };
    let snap_x = |x: usize| -> usize {
        match mosaic {
            Some((h, _)) if h > 1 => x - (x % h as usize),
            _ => x,
        }
    };
    match mode {
        3 => {
            // 240x160 15-bit color
            let row_offset = (sample_y as usize) * 240 * 2;
            for x in 0..240 {
                let addr = row_offset + snap_x(x) * 2;
                if addr + 1 < vram.len() {
                    let color = (vram[addr] as u16) | ((vram[addr + 1] as u16) << 8);
                    line_buf[x] = Pixel {
                        color,
                        layer: 2, // BG2
                        priority,
                        is_transparent: false,
                        is_obj_alpha: false,
                    };
                }
            }
        }
        4 => {
            // 240x160 8-bit paletted, dual frame
            let base = if frame { 0xA000 } else { 0x0000 };
            let row_offset = base + (sample_y as usize) * 240;
            for x in 0..240 {
                let addr = row_offset + snap_x(x);
                if addr < vram.len() {
                    let color_idx = vram[addr] as usize;
                    if color_idx != 0 {
                        let pal_addr = color_idx * 2;
                        if pal_addr + 1 < palette_ram.len() {
                            let color = (palette_ram[pal_addr] as u16) | ((palette_ram[pal_addr + 1] as u16) << 8);
                            line_buf[x] = Pixel {
                                color,
                                layer: 2,
                                priority,
                                is_transparent: false,
                                is_obj_alpha: false,
                            };
                        }
                    }
                }
            }
        }
        5
            // 160x128 15-bit color
            if y < 128 => {
                let base = if frame { 0xA000 } else { 0x0000 };
                let row_offset = base + (sample_y as usize) * 160 * 2;
                for x in 0..160 {
                    let addr = row_offset + snap_x(x) * 2;
                    if addr + 1 < vram.len() {
                        let color = (vram[addr] as u16) | ((vram[addr + 1] as u16) << 8);
                        line_buf[x] = Pixel {
                            color,
                            layer: 2,
                            priority,
                            is_transparent: false,
                            is_obj_alpha: false,
                        };
                    }
                }
            }
        _ => {}
    }
}

#[cfg(test)]
mod affine_mosaic_tests {
    use super::*;

    /// 128x128 affine BG whose tile row 0 holds tiles 0..16, where tile n is
    /// filled with colour index n+1, so each 8-pixel column has its own
    /// colour.
    fn scene() -> (Vec<u8>, Vec<u8>) {
        let mut vram = vec![0u8; 0x10000];
        for t in 0..16usize {
            vram[0x800 + t] = t as u8; // screen base 1 (0x800), row 0
            for p in 0..64 {
                vram[t * 64 + p] = (t + 1) as u8;
            }
        }
        let mut pal = vec![0u8; 512];
        for i in 0..32usize {
            pal[i * 2] = i as u8;
        }
        (vram, pal)
    }

    fn render(mosaic: Option<(u32, u32)>) -> Vec<u16> {
        let (vram, pal) = scene();
        let mut line = [Pixel::default(); 240];
        let bgcnt = 1 << 8; // screen base 1, 128x128, no wrap
        render_affine_bg(2, 0, bgcnt, 0, 0, 0x100, 0, 0, 0x100, &vram, &pal, mosaic, &mut line);
        line[..32].iter().map(|p| p.color).collect()
    }

    #[test]
    fn horizontal_mosaic_holds_block_sample() {
        let plain = render(None);
        assert_eq!(&plain[6..10], &[1, 1, 2, 2]);
        // 16-pixel blocks: pixels 0-15 all take the sample at x=0.
        let mos = render(Some((16, 1)));
        assert!(mos[..16].iter().all(|&c| c == 1));
        assert!(mos[16..32].iter().all(|&c| c == 3));
    }
}

#[cfg(test)]
mod mosaic_tests {
    use super::*;

    #[test]
    fn bitmap_mode3_mosaic_holds_source_across_block() {
        // Mode 3: 240x160, 2 bytes/pixel, direct 15-bit color. Encode each
        // pixel's color as its own x-coordinate so mosaic grouping is easy
        // to detect: without mosaic every pixel would differ from its
        // neighbors; with a 4-pixel mosaic block, groups of 4 must share
        // the color sampled at the block's start.
        let mut vram = vec![0u8; 240 * 160 * 2];
        for x in 0..240usize {
            let addr = x * 2;
            vram[addr] = (x & 0xFF) as u8;
            vram[addr + 1] = ((x >> 8) & 0xFF) as u8;
        }
        let palette = vec![0u8; 4];
        let mut line_buf = [Pixel::default(); 240];

        render_bitmap_bg(3, false, 0, &vram, &palette, 0, Some((4, 1)), &mut line_buf);

        for block_start in (0..240usize).step_by(4) {
            let expected = line_buf[block_start].color;
            for x in block_start..(block_start + 4).min(240) {
                assert_eq!(line_buf[x].color, expected, "pixel {} should match block start {}", x, block_start);
            }
        }
        // And the value actually sampled should be the block-start x itself.
        assert_eq!(line_buf[5].color, 4);
        assert_eq!(line_buf[9].color, 8);
    }

    #[test]
    fn bitmap_mode3_no_mosaic_samples_every_pixel_independently() {
        let mut vram = vec![0u8; 240 * 160 * 2];
        for x in 0..240usize {
            vram[x * 2] = (x & 0xFF) as u8;
        }
        let palette = vec![0u8; 4];
        let mut line_buf = [Pixel::default(); 240];
        render_bitmap_bg(3, false, 0, &vram, &palette, 0, None, &mut line_buf);
        assert_eq!(line_buf[5].color, 5);
        assert_eq!(line_buf[9].color, 9);
    }
}
