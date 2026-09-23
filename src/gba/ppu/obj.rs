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

#[allow(clippy::too_many_arguments)]
pub fn render_sprites(
    y: u32,
    dispcnt: u16,
    oam: &[u8],
    vram: &[u8],
    palette_ram: &[u8],
    obj_mosaic: (u32, u32),
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
        let mosaic_enabled = (attr0 & (1 << 12)) != 0;

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

            // Mosaic sample-and-hold in sprite-local space: the sampled row/
            // column is snapped to a block boundary (before flip is applied)
            // so a whole block reads the same source texel, while output
            // still writes one screen pixel at a time.
            let py_sample = if mosaic_enabled && obj_mosaic.1 > 1 {
                py_i32 - (py_i32 % obj_mosaic.1 as i32)
            } else {
                py_i32
            };
            let mut py = py_sample as usize;
            if vflip {
                py = orig_h - 1 - py;
            }

            for bx in 0..orig_w {
                let screen_x = sprite_x + bx as i32;
                if !(0..240).contains(&screen_x) {
                    continue;
                }
                let sx = screen_x as usize;

                let bx_sample = if mosaic_enabled && obj_mosaic.0 > 1 {
                    bx - (bx % obj_mosaic.0 as usize)
                } else {
                    bx
                };
                let mut px = bx_sample;
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

            // Mosaic (ROADMAP M1): like regular sprites, sample-and-hold
            // in the sprite's bounding box before the affine transform, so
            // each block shows the texel under its first pixel.
            let py_sample = if mosaic_enabled && obj_mosaic.1 > 1 {
                py_i32 - (py_i32 % obj_mosaic.1 as i32)
            } else {
                py_i32
            };
            let iy = py_sample - half_bound_h;

            for bx in 0..bound_w {
                let screen_x = sprite_x + bx as i32;
                if !(0..240).contains(&screen_x) {
                    continue;
                }
                let sx = screen_x as usize;

                let bx_sample = if mosaic_enabled && obj_mosaic.0 > 1 {
                    bx - (bx % obj_mosaic.0 as usize)
                } else {
                    bx
                };
                let ix = (bx_sample as i32) - half_bound_w;

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
                        let idx = if in_tile_x.is_multiple_of(2) { byte & 0x0F } else { (byte >> 4) & 0x0F };
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

#[cfg(test)]
mod mosaic_tests {
    use super::*;

    fn build_single_8x8_sprite_oam(mosaic_enabled: bool) -> Vec<u8> {
        let mut oam = vec![0u8; 1024];
        // attr0: y=0, non-affine, not disabled, normal mode, square shape,
        // mosaic bit (12) per test, 4bpp
        let attr0: u16 = if mosaic_enabled { 1 << 12 } else { 0 };
        // attr1: x=0, size=0 (8x8 for square shape)
        let attr1: u16 = 0;
        // attr2: tile 0, priority 0, palette 0
        let attr2: u16 = 0;
        oam[0..2].copy_from_slice(&attr0.to_le_bytes());
        oam[2..4].copy_from_slice(&attr1.to_le_bytes());
        oam[4..6].copy_from_slice(&attr2.to_le_bytes());
        oam
    }

    /// Builds a 64KB VRAM with OBJ tile 0 (4bpp, row 0) holding 8 distinct
    /// non-zero color indices, one per column: px0=1, px1=2, ..., px7=8.
    fn build_single_row_tile_vram() -> Vec<u8> {
        let mut vram = vec![0u8; 96 * 1024];
        let obj_char_base = 0x10000;
        vram[obj_char_base] = 0x21; // px0=1 (low nibble), px1=2 (high nibble)
        vram[obj_char_base + 1] = 0x43; // px2=3, px3=4
        vram[obj_char_base + 2] = 0x65; // px4=5, px5=6
        vram[obj_char_base + 3] = 0x87; // px6=7, px7=8
        vram
    }

    fn build_palette_with_indices_1_to_8() -> Vec<u8> {
        let mut palette = vec![0u8; 1024];
        for idx in 1u16..=8 {
            let addr = 0x200 + (idx as usize) * 2;
            palette[addr..addr + 2].copy_from_slice(&idx.to_le_bytes());
        }
        palette
    }

    #[test]
    fn obj_mosaic_holds_source_across_block() {
        let oam = build_single_8x8_sprite_oam(true);
        let vram = build_single_row_tile_vram();
        let palette = build_palette_with_indices_1_to_8();
        let mut line_buf = [Pixel::default(); 240];
        let mut objwin_buf = [false; 240];

        render_sprites(0, 0, &oam, &vram, &palette, (4, 1), &mut line_buf, &mut objwin_buf);

        // Columns 0-3 must all sample column 0's color (1); columns 4-7
        // must all sample column 4's color (5).
        for x in 0..4 {
            assert_eq!(line_buf[x].color, 1, "column {} should hold block-start color", x);
        }
        for x in 4..8 {
            assert_eq!(line_buf[x].color, 5, "column {} should hold block-start color", x);
        }
    }

    #[test]
    fn obj_without_mosaic_bit_samples_every_column_independently() {
        let oam = build_single_8x8_sprite_oam(false);
        let vram = build_single_row_tile_vram();
        let palette = build_palette_with_indices_1_to_8();
        let mut line_buf = [Pixel::default(); 240];
        let mut objwin_buf = [false; 240];

        // Pass a nonzero obj_mosaic size, but the sprite's own mosaic bit
        // is off, so it must still sample every column independently.
        render_sprites(0, 0, &oam, &vram, &palette, (4, 1), &mut line_buf, &mut objwin_buf);

        for (x, expected) in (0u16..8).enumerate() {
            assert_eq!(line_buf[x].color, expected + 1, "column {} should sample its own color", x);
        }
    }

    /// An affine 8x8 sprite with the identity matrix and mosaic on must
    /// sample-and-hold like a regular one.
    #[test]
    fn affine_obj_mosaic_holds_source_across_block() {
        let mut oam = build_single_8x8_sprite_oam(true);
        let attr0: u16 = (1 << 12) | (1 << 8); // mosaic + affine
        oam[0..2].copy_from_slice(&attr0.to_le_bytes());
        // Affine group 0: PA=PD=0x100, PB=PC=0 (entries at +6, +14, +22, +30).
        oam[6..8].copy_from_slice(&0x100u16.to_le_bytes());
        oam[30..32].copy_from_slice(&0x100u16.to_le_bytes());
        let vram = build_single_row_tile_vram();
        let palette = build_palette_with_indices_1_to_8();

        // Affine sprites sample around the centre, so render the sprite's
        // row 0 (screen line 0) and compare against the non-mosaic output.
        let render = |oam: &[u8], mos| {
            let mut line_buf = [Pixel::default(); 240];
            let mut objwin_buf = [false; 240];
            render_sprites(0, 0, oam, &vram, &palette, mos, &mut line_buf, &mut objwin_buf);
            line_buf[..8].iter().map(|p| p.color).collect::<Vec<_>>()
        };
        assert_eq!(render(&oam, (1, 1)), vec![1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(render(&oam, (4, 1)), vec![1, 1, 1, 1, 5, 5, 5, 5]);
    }
}
