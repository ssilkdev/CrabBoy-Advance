//! GBA PPU Background Layers Rendering (Modes 0 to 5)

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

    let scrolled_y = (y + vofs as u32) % map_height;

    for x in 0..240 {
        let scrolled_x = (x as u32 + hofs as u32) % map_width;

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
    _y: u32,
    bgcnt: u16,
    ref_x: i32,
    ref_y: i32,
    pa: i16,
    _pb: i16,
    pc: i16,
    _pd: i16,
    vram: &[u8],
    palette_ram: &[u8],
    line_buf: &mut [Pixel; 240],
) {
    let priority = (bgcnt & 3) as u8;
    let char_base = (((bgcnt >> 2) & 3) as usize) * 16384;
    let screen_base = (((bgcnt >> 8) & 0x1F) as usize) * 2048;
    let wrap = (bgcnt & (1 << 13)) != 0;
    let size_shift = ((bgcnt >> 14) & 3) as usize; // 0=128x128, 1=256x256, 2=512x512, 3=1024x1024
    let size_px = 128 << size_shift;
    let tiles_per_row = 16 << size_shift;

    let mut cur_x = ref_x;
    let mut cur_y = ref_y;

    for x in 0..240 {
        let px = cur_x >> 8;
        let py = cur_y >> 8;

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
    line_buf: &mut [Pixel; 240],
) {
    match mode {
        3 => {
            // 240x160 15-bit color
            let row_offset = (y as usize) * 240 * 2;
            for x in 0..240 {
                let addr = row_offset + x * 2;
                if addr + 1 < vram.len() {
                    let color = (vram[addr] as u16) | ((vram[addr + 1] as u16) << 8);
                    line_buf[x] = Pixel {
                        color,
                        layer: 2, // BG2
                        priority: 2,
                        is_transparent: false,
                        is_obj_alpha: false,
                    };
                }
            }
        }
        4 => {
            // 240x160 8-bit paletted, dual frame
            let base = if frame { 0xA000 } else { 0x0000 };
            let row_offset = base + (y as usize) * 240;
            for x in 0..240 {
                let addr = row_offset + x;
                if addr < vram.len() {
                    let color_idx = vram[addr] as usize;
                    if color_idx != 0 {
                        let pal_addr = color_idx * 2;
                        if pal_addr + 1 < palette_ram.len() {
                            let color = (palette_ram[pal_addr] as u16) | ((palette_ram[pal_addr + 1] as u16) << 8);
                            line_buf[x] = Pixel {
                                color,
                                layer: 2,
                                priority: 2,
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
                let row_offset = base + (y as usize) * 160 * 2;
                for x in 0..160 {
                    let addr = row_offset + x * 2;
                    if addr + 1 < vram.len() {
                        let color = (vram[addr] as u16) | ((vram[addr + 1] as u16) << 8);
                        line_buf[x] = Pixel {
                            color,
                            layer: 2,
                            priority: 2,
                            is_transparent: false,
                            is_obj_alpha: false,
                        };
                    }
                }
            }
        _ => {}
    }
}
