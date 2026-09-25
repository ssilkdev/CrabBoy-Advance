//! GBA High-Level Emulation (HLE) BIOS
//! Implements all standard GBA BIOS SWI routines so games can boot and run without a proprietary ROM.

use super::super::cpu::Arm7Tdmi;

pub fn execute_hle_swi(swi_number: u8, cpu: &mut Arm7Tdmi, read_mem8: &impl Fn(u32) -> u8, write_mem8: &mut impl FnMut(u32, u8), write_mem16: &mut impl FnMut(u32, u16), write_mem32: &mut impl FnMut(u32, u32)) -> Option<u16> {
    match swi_number {
        0x01 => {
            // RegisterRamReset
            let flags = cpu.regs[0] as u8;
            // bit 0: EWRAM
            if (flags & 0x01) != 0 {
                for addr in (0x0200_0000..0x0204_0000).step_by(4) {
                    write_mem32(addr, 0);
                }
            }
            // bit 1: IWRAM (except top 0x200 bytes)
            if (flags & 0x02) != 0 {
                for addr in (0x0300_0000..0x0300_7E00).step_by(4) {
                    write_mem32(addr, 0);
                }
            }
            // bit 2: Palette
            if (flags & 0x04) != 0 {
                for addr in (0x0500_0000..0x0500_0400).step_by(4) {
                    write_mem32(addr, 0);
                }
            }
            // bit 3: VRAM
            if (flags & 0x08) != 0 {
                for addr in (0x0600_0000..0x0601_8000).step_by(4) {
                    write_mem32(addr, 0);
                }
            }
            // bit 4: OAM
            if (flags & 0x10) != 0 {
                for addr in (0x0700_0000..0x0700_0400).step_by(4) {
                    write_mem32(addr, 0);
                }
            }
        }
        0x02 => {
            // Halt
            cpu.halted = true;
        }
        0x03 => {
            // Stop
            cpu.halted = true;
        }
        0x04 | 0x05 => {
            // IntrWait (SWI 0x04) / VBlankIntrWait (SWI 0x05)
            let (discard, wait_mask) = if swi_number == 0x05 {
                (true, 1u16)
            } else {
                (cpu.regs[0] != 0, cpu.regs[1] as u16)
            };

            let low = read_mem8(0x0300_7FF8) as u16;
            let high = read_mem8(0x0300_7FF9) as u16;
            let mut flags = low | (high << 8);

            if discard {
                flags &= !wait_mask;
                write_mem16(0x0300_7FF8, flags);
            }

            if (flags & wait_mask) != 0 {
                write_mem16(0x0300_7FF8, flags & !wait_mask);
                cpu.halted = false;
                return None;
            } else {
                cpu.halted = true;
                return Some(wait_mask);
            }
        }
        0x06 => {
            // Div(num, den) -> R0 = num / den, R1 = num % den, R3 = abs(num / den)
            let num = cpu.regs[0] as i32;
            let den = cpu.regs[1] as i32;
            if den == 0 {
                cpu.regs[0] = if num >= 0 { 0x7FFF_FFFF } else { -0x8000_0000i32 as u32 };
                cpu.regs[1] = num as u32;
                cpu.regs[3] = 0x7FFF_FFFF;
            } else if num == i32::MIN && den == -1 {
                // Special case: i32::MIN / -1 overflows in Rust
                cpu.regs[0] = i32::MIN as u32;
                cpu.regs[1] = 0;
                cpu.regs[3] = i32::MIN as u32; // abs(MIN) overflows to MIN on GBA
            } else {
                let div = num / den;
                let rem = num % den;
                cpu.regs[0] = div as u32;
                cpu.regs[1] = rem as u32;
                cpu.regs[3] = div.unsigned_abs();
            }
        }
        0x07 => {
            // DivArm(den, num) -> R0 = num / den, R1 = num % den, R3 = abs(num / den)
            let den = cpu.regs[0] as i32;
            let num = cpu.regs[1] as i32;
            if den == 0 {
                cpu.regs[0] = if num >= 0 { 0x7FFF_FFFF } else { -0x8000_0000i32 as u32 };
                cpu.regs[1] = num as u32;
                cpu.regs[3] = 0x7FFF_FFFF;
            } else if num == i32::MIN && den == -1 {
                cpu.regs[0] = i32::MIN as u32;
                cpu.regs[1] = 0;
                cpu.regs[3] = i32::MIN as u32;
            } else {
                let div = num / den;
                let rem = num % den;
                cpu.regs[0] = div as u32;
                cpu.regs[1] = rem as u32;
                cpu.regs[3] = div.unsigned_abs();
            }
        }
        0x08 => {
            // Sqrt(val)
            let val = cpu.regs[0] as u64;
            let res = (val as f64).sqrt() as u32;
            cpu.regs[0] = res;
        }
        0x09 => {
            // ArcTan(val)
            let val = (cpu.regs[0] as i16) as f64 / 16384.0;
            let angle = val.atan() / (2.0 * std::f64::consts::PI);
            cpu.regs[0] = (angle * 65536.0) as u32;
        }
        0x0A => {
            // ArcTan2(x, y)
            let x = (cpu.regs[0] as i16) as f64;
            let y = (cpu.regs[1] as i16) as f64;
            let angle = y.atan2(x) / (2.0 * std::f64::consts::PI);
            cpu.regs[0] = (angle * 65536.0) as u32;
        }
        0x0B => {
            // CpuSet(src, dst, control)
            let mut src = cpu.regs[0];
            let mut dst = cpu.regs[1];
            let control = cpu.regs[2];
            let count = (control & 0x001F_FFFF) as usize;
            let is_32bit = (control & (1 << 26)) != 0;
            let fixed_src = (control & (1 << 24)) != 0;
            if is_32bit {
                for _ in 0..count {
                    let b0 = read_mem8(src) as u32;
                    let b1 = read_mem8(src.wrapping_add(1)) as u32;
                    let b2 = read_mem8(src.wrapping_add(2)) as u32;
                    let b3 = read_mem8(src.wrapping_add(3)) as u32;
                    let val = b0 | (b1 << 8) | (b2 << 16) | (b3 << 24);
                    write_mem32(dst, val);
                    if !fixed_src {
                        src = src.wrapping_add(4);
                    }
                    dst = dst.wrapping_add(4);
                }
            } else {
                for _ in 0..count {
                    let b0 = read_mem8(src) as u16;
                    let b1 = read_mem8(src.wrapping_add(1)) as u16;
                    let val = b0 | (b1 << 8);
                    write_mem16(dst, val);
                    if !fixed_src {
                        src = src.wrapping_add(2);
                    }
                    dst = dst.wrapping_add(2);
                }
            }
            cpu.regs[0] = src;
            cpu.regs[1] = dst;
            cpu.regs[3] = 0x170;
        }
        0x0C => {
            // CpuFastSet(src, dst, control)
            let mut src = cpu.regs[0];
            let mut dst = cpu.regs[1];
            let control = cpu.regs[2];
            let count = ((control & 0x001F_FFFF) as usize + 7) & !7;
            let fixed_src = (control & (1 << 24)) != 0;

            for _ in 0..count {
                let b0 = read_mem8(src) as u32;
                let b1 = read_mem8(src.wrapping_add(1)) as u32;
                let b2 = read_mem8(src.wrapping_add(2)) as u32;
                let b3 = read_mem8(src.wrapping_add(3)) as u32;
                let val = b0 | (b1 << 8) | (b2 << 16) | (b3 << 24);
                write_mem32(dst, val);
                if !fixed_src {
                    src = src.wrapping_add(4);
                }
                dst = dst.wrapping_add(4);
            }
            cpu.regs[0] = src;
            cpu.regs[1] = dst;
        }
        0x0D => {
            // BiosChecksum
            cpu.regs[0] = 0xBAAE_187F;
        }
        0x0E => {
            // BgAffineSet(src, dst, num)
            let mut src = cpu.regs[0];
            let mut dst = cpu.regs[1];
            let num = cpu.regs[2];
            for _ in 0..num {
                let cx = (read_mem8(src) as i32) | ((read_mem8(src + 1) as i32) << 8) | ((read_mem8(src + 2) as i32) << 16) | ((read_mem8(src + 3) as i32) << 24);
                let cy = (read_mem8(src + 4) as i32) | ((read_mem8(src + 5) as i32) << 8) | ((read_mem8(src + 6) as i32) << 16) | ((read_mem8(src + 7) as i32) << 24);
                let dx = (read_mem8(src + 8) as i16) | ((read_mem8(src + 9) as i16) << 8);
                let dy = (read_mem8(src + 10) as i16) | ((read_mem8(src + 11) as i16) << 8);
                let sx = (read_mem8(src + 12) as i16) | ((read_mem8(src + 13) as i16) << 8);
                let sy = (read_mem8(src + 14) as i16) | ((read_mem8(src + 15) as i16) << 8);
                let alpha = (read_mem8(src + 16) as u16) | ((read_mem8(src + 17) as u16) << 8);
                src = src.wrapping_add(20);

                let angle = ((alpha >> 8) as f64) / 128.0 * std::f64::consts::PI;
                let sin_a = angle.sin();
                let cos_a = angle.cos();

                let pa = ((sx as f64) * cos_a).round() as i16;
                let pb = (-(sx as f64) * sin_a).round() as i16;
                let pc = ((sy as f64) * sin_a).round() as i16;
                let pd = ((sy as f64) * cos_a).round() as i16;

                write_mem16(dst, pa as u16);
                write_mem16(dst + 2, pb as u16);
                write_mem16(dst + 4, pc as u16);
                write_mem16(dst + 6, pd as u16);

                let rx = cx - ((pa as i32) * (dx as i32) + (pb as i32) * (dy as i32));
                let ry = cy - ((pc as i32) * (dx as i32) + (pd as i32) * (dy as i32));

                write_mem32(dst + 8, rx as u32);
                write_mem32(dst + 12, ry as u32);
                dst = dst.wrapping_add(16);
            }
        }
        0x0F => {
            // ObjAffineSet(src, dst, num, diff)
            let mut src = cpu.regs[0];
            let mut dst = cpu.regs[1];
            let num = cpu.regs[2];
            let diff = cpu.regs[3];
            for _ in 0..num {
                let sx = (read_mem8(src) as i16) | ((read_mem8(src + 1) as i16) << 8);
                let sy = (read_mem8(src + 2) as i16) | ((read_mem8(src + 3) as i16) << 8);
                let alpha = (read_mem8(src + 4) as u16) | ((read_mem8(src + 5) as u16) << 8);
                src = src.wrapping_add(8);

                let theta = ((alpha >> 8) as f64) / 128.0 * std::f64::consts::PI;
                let sin_a = theta.sin();
                let cos_a = theta.cos();

                let pa = ((sx as f64) * cos_a).round() as i16;
                let pb = (-(sx as f64) * sin_a).round() as i16;
                let pc = ((sy as f64) * sin_a).round() as i16;
                let pd = ((sy as f64) * cos_a).round() as i16;

                write_mem16(dst, pa as u16);
                write_mem16(dst.wrapping_add(diff), pb as u16);
                write_mem16(dst.wrapping_add(diff * 2), pc as u16);
                write_mem16(dst.wrapping_add(diff * 3), pd as u16);
                dst = dst.wrapping_add(diff * 4);
            }
        }
        0x10 => {
            // BitUnPack(src, dst, info)
            let mut src = cpu.regs[0];
            let mut dst = cpu.regs[1];
            let info = cpu.regs[2];

            let mut src_len = (read_mem8(info) as u16) | ((read_mem8(info + 1) as u16) << 8);
            let src_width = read_mem8(info + 2);
            let dst_width = read_mem8(info + 3);
            let bias = (read_mem8(info + 4) as u32)
                | ((read_mem8(info + 5) as u32) << 8)
                | ((read_mem8(info + 6) as u32) << 16)
                | ((read_mem8(info + 7) as u32) << 24);

            if src_width != 0 && (dst_width == 1 || dst_width == 2 || dst_width == 4 || dst_width == 8 || dst_width == 16 || dst_width == 32) {
                let mask = (1u32 << src_width) - 1;
                let add_zero = (bias & 0x8000_0000) != 0;
                let offset_data = bias & 0x7FFF_FFFF;

                let mut in_byte: u8 = 0;
                let mut out_word: u32 = 0;
                let mut bits_rem = 0;
                let mut bits_eaten = 0;

                while src_len > 0 || bits_rem > 0 {
                    if bits_rem == 0 {
                        in_byte = read_mem8(src);
                        src = src.wrapping_add(1);
                        src_len -= 1;
                        bits_rem = 8;
                    }

                    let mut scaled = (in_byte as u32) & mask;
                    in_byte >>= src_width;
                    if scaled != 0 || add_zero {
                        scaled = scaled.wrapping_add(offset_data);
                    }
                    bits_rem -= src_width as i32;

                    out_word |= (scaled & ((1u64 << dst_width) - 1) as u32) << bits_eaten;
                    bits_eaten += dst_width as i32;

                    if bits_eaten >= 32 {
                        write_mem32(dst, out_word);
                        dst = dst.wrapping_add(4);
                        bits_eaten = 0;
                        out_word = 0;
                    }
                }
                if bits_eaten > 0 {
                    write_mem32(dst, out_word);
                }
            }
        }
        0x11 | 0x12 => {
            // LZ77UnCompWram / LZ77UnCompVram
            let mut src = cpu.regs[0];
            let mut dst = cpu.regs[1];
            let is_vram = swi_number == 0x12;

            let header = (read_mem8(src) as u32)
                | ((read_mem8(src + 1) as u32) << 8)
                | ((read_mem8(src + 2) as u32) << 16)
                | ((read_mem8(src + 3) as u32) << 24);
            src = src.wrapping_add(4);

            let decomp_len = (header >> 8) as usize;
            let mut decompressed = Vec::with_capacity(decomp_len);

            while decompressed.len() < decomp_len {
                let flags = read_mem8(src);
                src = src.wrapping_add(1);

                for bit in (0..8).rev() {
                    if decompressed.len() >= decomp_len {
                        break;
                    }

                    if (flags & (1 << bit)) == 0 {
                        // Uncompressed byte
                        decompressed.push(read_mem8(src));
                        src = src.wrapping_add(1);
                    } else {
                        // Compressed block
                        let b0 = read_mem8(src) as usize;
                        let b1 = read_mem8(src.wrapping_add(1)) as usize;
                        src = src.wrapping_add(2);

                        let length = (b0 >> 4) + 3;
                        let disp = (((b0 & 0x0F) << 8) | b1) + 1;

                        for _ in 0..length {
                            if decompressed.len() >= decomp_len {
                                break;
                            }
                            let back_idx = decompressed.len().saturating_sub(disp);
                            let val = decompressed[back_idx];
                            decompressed.push(val);
                        }
                    }
                }
            }

            if is_vram {
                // Write halfwords
                for i in (0..decompressed.len()).step_by(2) {
                    let b0 = decompressed[i] as u16;
                    let b1 = if i + 1 < decompressed.len() { decompressed[i + 1] as u16 } else { 0 };
                    write_mem16(dst, b0 | (b1 << 8));
                    dst = dst.wrapping_add(2);
                }
            } else {
                for b in decompressed {
                    write_mem8(dst, b);
                    dst = dst.wrapping_add(1);
                }
            }
            cpu.regs[0] = src;
            cpu.regs[1] = dst;
            cpu.regs[3] = 0;
        }
        0x13 => {
            // HuffUnCompReadNormal (GBATEK "BIOS Decompression Functions").
            //
            // Header: bits 0-3 = data size (4 or 8), bits 4-7 = 2 (Huffman),
            // bits 8-31 = decompressed size. Then the tree: one byte
            // `tree_size` (table length / 2 - 1), the root node, and the
            // rest of the node table; the bitstream starts right after the
            // table, word aligned, read as 32-bit little-endian words, most
            // significant bit first. Node byte: bits 0-5 = offset to the
            // children (at (node_addr & !1) + offset * 2 + 2, left then
            // right), bit 7 = left child is data, bit 6 = right child is
            // data. Output is written in 32-bit units (4-bit data packs low
            // nibble first).
            let src = cpu.regs[0];
            let mut dst = cpu.regs[1];
            let read32 = |a: u32| {
                (read_mem8(a) as u32)
                    | ((read_mem8(a.wrapping_add(1)) as u32) << 8)
                    | ((read_mem8(a.wrapping_add(2)) as u32) << 16)
                    | ((read_mem8(a.wrapping_add(3)) as u32) << 24)
            };
            let header = read32(src);
            let bits = header & 0xF;
            let decomp_len = (header >> 8) as usize;
            if (bits == 4 || bits == 8) && decomp_len > 0 {
                let tree_size = read_mem8(src.wrapping_add(4)) as u32;
                let root = src.wrapping_add(5);
                let mut stream = src.wrapping_add(4).wrapping_add((tree_size + 1) * 2);
                let mut out_word = 0u32;
                let mut out_bits = 0u32;
                let mut written = 0usize;
                let mut node = root;
                'outer: while written < decomp_len {
                    let word = read32(stream);
                    stream = stream.wrapping_add(4);
                    for i in (0..32).rev() {
                        let n = read_mem8(node);
                        let child = (node & !1).wrapping_add(((n & 0x3F) as u32) * 2 + 2);
                        let right = (word >> i) & 1 != 0;
                        let (addr, is_data) =
                            if right { (child.wrapping_add(1), n & 0x40 != 0) } else { (child, n & 0x80 != 0) };
                        if is_data {
                            let value = read_mem8(addr) as u32 & ((1 << bits) - 1);
                            out_word |= value << out_bits;
                            out_bits += bits;
                            if out_bits == 32 {
                                write_mem32(dst, out_word);
                                dst = dst.wrapping_add(4);
                                written += 4;
                                out_word = 0;
                                out_bits = 0;
                                if written >= decomp_len {
                                    break 'outer;
                                }
                            }
                            node = root;
                        } else {
                            node = addr;
                        }
                    }
                }
                cpu.regs[0] = stream;
                cpu.regs[1] = dst;
            } else {
                log::warn!("HuffUnComp: bad header {header:#010x} at {src:#010x}");
            }
        }
        0x16 | 0x17 | 0x18 => {
            // Diff8bitUnFilterWram (0x16) / Diff8bitUnFilterVram (0x17) /
            // Diff16bitUnFilter (0x18): header like the decompressors (bits
            // 8-31 = size), then the first unit followed by deltas; each
            // output unit is the running sum. 8-bit VRAM output is written
            // in halfwords (VRAM can't take byte writes).
            let mut src = cpu.regs[0];
            let mut dst = cpu.regs[1];
            let header = (read_mem8(src) as u32)
                | ((read_mem8(src.wrapping_add(1)) as u32) << 8)
                | ((read_mem8(src.wrapping_add(2)) as u32) << 16)
                | ((read_mem8(src.wrapping_add(3)) as u32) << 24);
            src = src.wrapping_add(4);
            let len = (header >> 8) as usize;
            if swi_number == 0x18 {
                let mut acc = 0u16;
                for _ in 0..len / 2 {
                    let d = read_mem8(src) as u16 | (read_mem8(src.wrapping_add(1)) as u16) << 8;
                    src = src.wrapping_add(2);
                    acc = acc.wrapping_add(d);
                    write_mem16(dst, acc);
                    dst = dst.wrapping_add(2);
                }
            } else {
                let mut acc = 0u8;
                let mut out = Vec::with_capacity(len);
                for _ in 0..len {
                    acc = acc.wrapping_add(read_mem8(src));
                    src = src.wrapping_add(1);
                    out.push(acc);
                }
                if swi_number == 0x16 {
                    for b in out {
                        write_mem8(dst, b);
                        dst = dst.wrapping_add(1);
                    }
                } else {
                    for pair in out.chunks(2) {
                        let v = pair[0] as u16 | (*pair.get(1).unwrap_or(&0) as u16) << 8;
                        write_mem16(dst, v);
                        dst = dst.wrapping_add(2);
                    }
                }
            }
            cpu.regs[0] = src;
            cpu.regs[1] = dst;
        }
        0x14 | 0x15 => {
            // RLUnCompWram / RLUnCompVram
            let mut src = cpu.regs[0];
            let mut dst = cpu.regs[1];
            let is_vram = swi_number == 0x15;

            let header = (read_mem8(src) as u32)
                | ((read_mem8(src + 1) as u32) << 8)
                | ((read_mem8(src + 2) as u32) << 16)
                | ((read_mem8(src + 3) as u32) << 24);
            src = src.wrapping_add(4);

            let decomp_len = (header >> 8) as usize;
            let mut decompressed = Vec::with_capacity(decomp_len);

            while decompressed.len() < decomp_len {
                let flag = read_mem8(src);
                src = src.wrapping_add(1);

                let is_compressed = (flag & 0x80) != 0;
                let length = ((flag & 0x7F) as usize) + (if is_compressed { 3 } else { 1 });

                if is_compressed {
                    let data = read_mem8(src);
                    src = src.wrapping_add(1);
                    for _ in 0..length {
                        if decompressed.len() < decomp_len {
                            decompressed.push(data);
                        }
                    }
                } else {
                    for _ in 0..length {
                        if decompressed.len() < decomp_len {
                            decompressed.push(read_mem8(src));
                            src = src.wrapping_add(1);
                        }
                    }
                }
            }

            if is_vram {
                for i in (0..decompressed.len()).step_by(2) {
                    let b0 = decompressed[i] as u16;
                    let b1 = if i + 1 < decompressed.len() { decompressed[i + 1] as u16 } else { 0 };
                    write_mem16(dst, b0 | (b1 << 8));
                    dst = dst.wrapping_add(2);
                }
            } else {
                for b in decompressed {
                    write_mem8(dst, b);
                    dst = dst.wrapping_add(1);
                }
            }
            cpu.regs[0] = src;
            cpu.regs[1] = dst;
        }
        _ => {
            log::warn!(
                "Unhandled HLE SWI: 0x{:02X} (r0=0x{:08X}, r1=0x{:08X}, r2=0x{:08X}, r3=0x{:08X})",
                swi_number, cpu.regs[0], cpu.regs[1], cpu.regs[2], cpu.regs[3]
            );
        }
    }
    None
}

#[cfg(test)]
mod decompression_tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    /// Run one HLE SWI against a flat memory map (address -> byte).
    fn run(swi: u8, mem: &mut BTreeMap<u32, u8>, r0: u32, r1: u32) -> Arm7Tdmi {
        let mut cpu = Arm7Tdmi::new();
        cpu.regs[0] = r0;
        cpu.regs[1] = r1;
        let snapshot = mem.clone();
        let out = RefCell::new(BTreeMap::new());
        let read = |a: u32| *snapshot.get(&a).unwrap_or(&0);
        let mut w8 = |a: u32, v: u8| {
            out.borrow_mut().insert(a, v);
        };
        let mut w16 = |a: u32, v: u16| {
            let mut o = out.borrow_mut();
            o.insert(a, v as u8);
            o.insert(a + 1, (v >> 8) as u8);
        };
        let mut w32 = |a: u32, v: u32| {
            let mut o = out.borrow_mut();
            for i in 0..4 {
                o.insert(a + i, (v >> (8 * i)) as u8);
            }
        };
        execute_hle_swi(swi, &mut cpu, &read, &mut w8, &mut w16, &mut w32);
        mem.extend(out.into_inner());
        cpu
    }

    fn load(mem: &mut BTreeMap<u32, u8>, at: u32, data: &[u8]) {
        for (i, b) in data.iter().enumerate() {
            mem.insert(at + i as u32, *b);
        }
    }

    fn read_out(mem: &BTreeMap<u32, u8>, at: u32, n: usize) -> Vec<u8> {
        (0..n as u32).map(|i| *mem.get(&(at + i)).unwrap_or(&0)).collect()
    }

    /// Minimal Huffman encoder producing the GBA BIOS format, used to check
    /// the decoder round-trips. Builds a canonical-ish tree and lays nodes
    /// out breadth-first so child offsets stay within 6 bits for small
    /// alphabets.
    fn huff_encode(data: &[u8], bits: u32) -> Vec<u8> {
        #[derive(Clone)]
        enum T {
            Leaf(u8),
            Node(Box<T>, Box<T>),
        }
        let syms: Vec<u8> = if bits == 8 {
            data.to_vec()
        } else {
            data.iter().flat_map(|b| [b & 0xF, b >> 4]).collect()
        };
        let mut freq = BTreeMap::new();
        for &s in &syms {
            *freq.entry(s).or_insert(0usize) += 1;
        }
        let mut heap: Vec<(usize, T)> = freq.iter().map(|(&s, &f)| (f, T::Leaf(s))).collect();
        if heap.len() == 1 {
            let only = heap[0].1.clone();
            heap.push((0, only));
        }
        while heap.len() > 1 {
            heap.sort_by(|a, b| b.0.cmp(&a.0));
            let (fa, a) = heap.pop().unwrap();
            let (fb, b) = heap.pop().unwrap();
            heap.push((fa + fb, T::Node(Box::new(a), Box::new(b))));
        }
        let root = heap.pop().unwrap().1;
        // Codes.
        fn codes(t: &T, prefix: Vec<bool>, out: &mut BTreeMap<u8, Vec<bool>>) {
            match t {
                T::Leaf(s) => {
                    out.entry(*s).or_insert(prefix);
                }
                T::Node(l, r) => {
                    let mut pl = prefix.clone();
                    pl.push(false);
                    codes(l, pl, out);
                    let mut pr = prefix;
                    pr.push(true);
                    codes(r, pr, out);
                }
            }
        }
        let mut code = BTreeMap::new();
        codes(&root, vec![], &mut code);
        // Table: slot 0 = root node; children pairs appended breadth-first.
        let mut table: Vec<u8> = vec![0];
        let mut queue: std::collections::VecDeque<(usize, T)> = std::collections::VecDeque::new();
        queue.push_back((0, root));
        while let Some((slot, t)) = queue.pop_front() {
            if let T::Node(l, r) = t {
                let child = table.len();
                // Child pair index must be (slot & !1) + off*2 + 2, relative
                // to table positions offset by 1 (root sits at src+5, odd).
                let base = ((slot + 1) & !1) + 2; // in table-byte units, +1 for the size byte
                let off = (child + 1 - base) / 2;
                assert!(off < 64 && (child + 1 - base) % 2 == 0, "tree too wide for the test encoder");
                let mut flags = off as u8;
                table.push(0);
                table.push(0);
                for (i, c) in [(0usize, &*l), (1, &*r)] {
                    match c {
                        T::Leaf(s) => {
                            table[child + i] = *s;
                            flags |= if i == 0 { 0x80 } else { 0x40 };
                        }
                        T::Node(..) => queue.push_back((child + i, c.clone())),
                    }
                }
                table[slot] = flags;
            }
        }
        // tree_size byte: table length (incl. this byte) / 2 - 1, table padded
        // so the stream is word aligned (header 4 + size byte + table).
        while (1 + table.len()) % 4 != 0 {
            table.push(0);
        }
        let tree_size = ((1 + table.len()) / 2 - 1) as u8;
        let mut out = Vec::new();
        out.extend_from_slice(&((data.len() as u32) << 8 | 0x20 | bits).to_le_bytes());
        out.push(tree_size);
        out.extend_from_slice(&table);
        // Bitstream: 32-bit words, MSB first.
        let mut word = 0u32;
        let mut n = 0;
        for s in syms {
            for &b in &code[&s] {
                word = (word << 1) | b as u32;
                n += 1;
                if n == 32 {
                    out.extend_from_slice(&word.to_le_bytes());
                    word = 0;
                    n = 0;
                }
            }
        }
        if n > 0 {
            word <<= 32 - n;
            out.extend_from_slice(&word.to_le_bytes());
        }
        out
    }

    #[test]
    fn huffman_8bit_and_4bit_round_trip() {
        let mut data = Vec::new();
        for i in 0..512u32 {
            data.push(b"synthwave!"[(i as usize * 7 + i as usize / 13) % 10]);
        }
        for bits in [8u32, 4] {
            let enc = huff_encode(&data, bits);
            let mut mem = BTreeMap::new();
            load(&mut mem, 0x0800_0000, &enc);
            let cpu = run(0x13, &mut mem, 0x0800_0000, 0x0200_0000);
            assert_eq!(read_out(&mem, 0x0200_0000, data.len()), data, "{bits}-bit");
            assert_eq!(cpu.regs[1], 0x0200_0000 + data.len() as u32);
        }
    }

    #[test]
    fn huffman_single_symbol() {
        let data = vec![0xAB; 64];
        let enc = huff_encode(&data, 8);
        let mut mem = BTreeMap::new();
        load(&mut mem, 0x0800_0000, &enc);
        run(0x13, &mut mem, 0x0800_0000, 0x0200_0000);
        assert_eq!(read_out(&mem, 0x0200_0000, 64), data);
    }

    #[test]
    fn diff_unfilters() {
        // 8-bit: [5, +1, +1, -2 (0xFE)] -> 5 6 7 5
        let mut mem = BTreeMap::new();
        load(&mut mem, 0x0800_0000, &[0x81, 4, 0, 0, 5, 1, 1, 0xFE]);
        run(0x16, &mut mem, 0x0800_0000, 0x0200_0000);
        assert_eq!(read_out(&mem, 0x0200_0000, 4), [5, 6, 7, 5]);
        let mut mem = BTreeMap::new();
        load(&mut mem, 0x0800_0000, &[0x81, 4, 0, 0, 5, 1, 1, 0xFE]);
        run(0x17, &mut mem, 0x0800_0000, 0x0600_0000);
        assert_eq!(read_out(&mem, 0x0600_0000, 4), [5, 6, 7, 5]);
        // 16-bit: [0x1000, +0x0010, -0x0020] -> 0x1000 0x1010 0x0FF0
        let mut mem = BTreeMap::new();
        load(&mut mem, 0x0800_0000, &[0x82, 6, 0, 0, 0x00, 0x10, 0x10, 0x00, 0xE0, 0xFF]);
        run(0x18, &mut mem, 0x0800_0000, 0x0200_0000);
        assert_eq!(read_out(&mem, 0x0200_0000, 6), [0x00, 0x10, 0x10, 0x10, 0xF0, 0x0F]);
    }
}
