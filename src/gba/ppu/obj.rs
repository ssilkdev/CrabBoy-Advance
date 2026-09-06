//! GBA PPU Sprite (OBJ) Layer Rendering

use super::blend::Pixel;

pub fn get_sprite_size(shape: u16, size: u16) -> (usize, usize) {
    match shape {
        0 => match size {
            // Square
            0 => (8, 8),
            1 => (16, 16),
            2 => (32, 32),
            3 => (64, 64),
            _ => (8, 8),
        },
        1 => match size {
            // Horizontal
            0 => (16, 8),
            1 => (32, 8),
            2 => (32, 16),
            3 => (64, 32),
            _ => (8, 8),
        },
        2 => match size {
            // Vertical
            0 => (8, 16),
            1 => (8, 32),
            2 => (16, 32),
            3 => (32, 64),
            _ => (8, 8),
        },
        _ => (8, 8),
    }
}

pub fn render_sprites(
    y: u32,
    dispcnt: u16,
    oam: &[u8],
    vram: &[u8],
    palette_ram: &[u8],
    line_buf: &mut [Pixel; 240],
    objwin_buf: &mut [bool; 240],
) {
    let mapping_1d = (dispcnt & (1 << 6)) != 0;
    // OBJ tile data in VRAM starts at 0x10000 (64KB)
    let obj_char_base = 0x10000;

    let mode = (dispcnt & 7) as u8;

    // Render sprites in reverse order (127 down to 0) so earlier sprites overwrite later sprites
    for i in (0..128).rev() {
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
        let is_objwin = obj_mode == 2;
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

        // Vertical positioning and wrapping around 256
        let raw_y = (attr0 & 0xFF) as i32;
        let py_i32 = if raw_y + (bound_h as i32) > 256 && (y as i32) < (raw_y + (bound_h as i32) - 256) {
            (y as i32) + 256 - raw_y
        } else if (y as i32) >= raw_y && (y as i32) < raw_y + (bound_h as i32) {
            (y as i32) - raw_y
        } else {
            continue;
        };

        // Horizontal positioning: 9-bit coordinate (-256..255)
        let raw_x = (attr1 & 0x1FF) as i32;
        let sprite_x = if raw_x >= 256 { raw_x - 512 } else { raw_x };

        // Offscreen horizontal culling
        if sprite_x >= 240 || sprite_x + (bound_w as i32) <= 0 {
            continue;
        }

        let raw_tile = (attr2 & 0x3FF) as usize;
        if mode >= 3 && raw_tile < 512 {
            continue;
        }
        // In 2D mapping mode, 8bpp forces bit 0 of tile_base to 0
        let tile_base = if is_8bpp && !mapping_1d {
            raw_tile & !1
        } else {
            raw_tile
        };

        let priority = ((attr2 >> 10) & 3) as u8;
        let pal_num = ((attr2 >> 12) & 0x0F) as usize;

        if !is_affine {
            let hflip = (attr1 & (1 << 12)) != 0;
            let vflip = (attr1 & (1 << 13)) != 0;

            let mut py = py_i32 as usize;
            if vflip {
                py = orig_h - 1 - py;
            }

            for bx in 0..orig_w {
                let screen_x = sprite_x + bx as i32;
                if screen_x < 0 || screen_x >= 240 {
                    continue;
                }
                let sx = screen_x as usize;

                let mut px = bx;
                if hflip {
                    px = orig_w - 1 - px;
                }

                // Compute tile number inside sprite
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
                } else {
                    // 2D mapping: 32x32 tiles grid in VRAM
                    if is_8bpp {
                        let tile_col = ((tile_base & 0x1F) + tile_x * 2) & 0x1F;
                        let tile_row = (((tile_base >> 5) & 0x1F) + tile_y) & 0x1F;
                        (tile_row * 32 + tile_col) & 0x3FF
                    } else {
                        let tile_col = ((tile_base & 0x1F) + tile_x) & 0x1F;
                        let tile_row = (((tile_base >> 5) & 0x1F) + tile_y) & 0x1F;
                        (tile_row * 32 + tile_col) & 0x3FF
                    }
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
                    if is_objwin {
                        objwin_buf[sx] = true;
                    } else {
                        let pal_addr = 0x200 + color_idx * 2; // OBJ palette starts at 0x200
                        if pal_addr + 1 < palette_ram.len() {
                            let color = (palette_ram[pal_addr] as u16) | ((palette_ram[pal_addr + 1] as u16) << 8);
                            if priority <= line_buf[sx].priority || line_buf[sx].is_transparent {
                                line_buf[sx] = Pixel {
                                    color,
                                    layer: 4, // OBJ
                                    priority,
                                    is_transparent: false,
                                    is_obj_alpha: is_semi_trans,
                                };
                            }
                        }
                    }
                }
            }
        } else {
            // Affine sprite
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

            let iy = py_i32 - half_bound_h;

            for bx in 0..bound_w {
                let screen_x = sprite_x + bx as i32;
                if screen_x < 0 || screen_x >= 240 {
                    continue;
                }
                let sx = screen_x as usize;

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
                } else {
                    if is_8bpp {
                        let tile_col = ((tile_base & 0x1F) + tile_x * 2) & 0x1F;
                        let tile_row = (((tile_base >> 5) & 0x1F) + tile_y) & 0x1F;
                        (tile_row * 32 + tile_col) & 0x3FF
                    } else {
                        let tile_col = ((tile_base & 0x1F) + tile_x) & 0x1F;
                        let tile_row = (((tile_base >> 5) & 0x1F) + tile_y) & 0x1F;
                        (tile_row * 32 + tile_col) & 0x3FF
                    }
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
                    if is_objwin {
                        objwin_buf[sx] = true;
                    } else {
                        let pal_addr = 0x200 + color_idx * 2;
                        if pal_addr + 1 < palette_ram.len() {
                            let color = (palette_ram[pal_addr] as u16) | ((palette_ram[pal_addr + 1] as u16) << 8);
                            if priority <= line_buf[sx].priority || line_buf[sx].is_transparent {
                                line_buf[sx] = Pixel {
                                    color,
                                    layer: 4,
                                    priority,
                                    is_transparent: false,
                                    is_obj_alpha: is_semi_trans,
                                };
                            }
                        }
                    }
                }
            }
        }
    }
}
