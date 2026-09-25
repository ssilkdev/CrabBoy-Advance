//! High-level emulation of the Nintendo DS BIOS software interrupts.
//!
//! SWI numbers follow GBATEK "BIOS Functions" for the NDS, which differ from
//! the GBA (e.g. 03h is WaitByLoop, 06h Halt, 09h Div, 0Dh Sqrt, 0Eh
//! GetCRC16). ARM-state SWIs carry the number in comment bits 16-23;
//! Thumb SWIs in bits 0-7.

use super::cpu::executor::{CpuBus, CpuState};
use crate::gba::cpu::FLAG_I;

/// Handle a SWI. `comment` is the raw comment field; `thumb` says which
/// encoding it came from. Returns false if the SWI was not emulated.
pub fn hle_swi<C: CpuState, B: CpuBus>(cpu: &mut C, bus: &mut B, comment: u32, thumb: bool) -> bool {
    let num = if thumb { comment & 0xFF } else { (comment >> 16) & 0xFF };
    let arm9 = cpu.is_armv5();
    match num {
        0x03 => {
            // WaitByLoop: r0 iterations of a 4-cycle loop.
            cpu.set_reg(0, 0);
        }
        0x04 => intr_wait(cpu, bus, thumb),
        0x05 => {
            // VBlankIntrWait = IntrWait(1, 1)
            cpu.set_reg(0, 1);
            cpu.set_reg(1, 1);
            intr_wait(cpu, bus, thumb);
        }
        0x06 => cpu.set_halted(true),
        0x07 if !arm9 => cpu.set_halted(true), // Sleep (approximation)
        0x08 if !arm9 => {
            // SoundBias: 0 = level 0, else 200h.
            let level = if cpu.reg(0) != 0 { 0x200 } else { 0 };
            bus.write16(0x0400_0504, level);
        }
        0x09 => {
            // Div: r0 = r0/r1, r1 = r0%r1, r3 = |r0/r1|
            let num = cpu.reg(0) as i32;
            let den = cpu.reg(1) as i32;
            if den != 0 {
                let q = num.wrapping_div(den);
                cpu.set_reg(0, q as u32);
                cpu.set_reg(1, num.wrapping_rem(den) as u32);
                cpu.set_reg(3, q.unsigned_abs());
            }
        }
        0x0B => cpu_set(cpu, bus),
        0x0C => cpu_fast_set(cpu, bus),
        0x0D => {
            let v = cpu.reg(0);
            cpu.set_reg(0, isqrt(v as u64) as u32);
        }
        0x0E => {
            // GetCRC16(crc, addr, len)
            let mut crc = cpu.reg(0) & 0xFFFF;
            let addr = cpu.reg(1);
            let len = cpu.reg(2);
            for i in 0..len {
                crc ^= bus.read8(addr.wrapping_add(i)) as u32;
                for _ in 0..8 {
                    let carry = crc & 1;
                    crc >>= 1;
                    if carry != 0 {
                        crc ^= 0xA001;
                    }
                }
            }
            cpu.set_reg(0, crc);
        }
        0x0F => cpu.set_reg(0, 0), // IsDebugger: retail unit
        0x10 => bit_unpack(cpu, bus),
        0x11 => {
            let (src, dst) = (cpu.reg(0), cpu.reg(1));
            let out = lz77(bus, src);
            write_out(bus, dst, &out, false);
        }
        0x14 => {
            let (src, dst) = (cpu.reg(0), cpu.reg(1));
            let out = run_length(bus, src);
            write_out(bus, dst, &out, false);
        }
        0x16 if arm9 => diff_unfilter(cpu, bus, false),
        0x18 if arm9 => diff_unfilter(cpu, bus, true),
        0x1A if !arm9 => {
            // GetSineTable: sin(i * 90deg / 64) * 0x8000 (i = 0..3Fh)
            let i = (cpu.reg(0) & 0x3F) as f64;
            let v = ((i * std::f64::consts::FRAC_PI_2 / 64.0).sin() * 32768.0).round() as u32;
            cpu.set_reg(0, v.min(0xFFFF));
        }
        0x1B if !arm9 => {
            // GetPitchTable: (2^(i/768) - 1) * 0x10000 (i = 0..2FFh)
            let i = (cpu.reg(0) % 768) as f64;
            let v = (((i / 768.0).exp2() - 1.0) * 65536.0) as u32;
            cpu.set_reg(0, v & 0xFFFF);
        }
        0x1C if !arm9 => cpu.set_reg(0, volume_table(cpu.reg(0)) as u32),
        0x1D if !arm9 => cpu.set_reg(0, 0), // GetBootProcs
        0x1F if arm9 => bus.write32(0x0400_0300, cpu.reg(0)), // CustomPost
        0x00 => log::warn!("NDS SWI SoftReset not emulated"),
        0x12 | 0x13 | 0x15 => {
            // Read-by-callback decompressors call back into game code.
            log::warn!("NDS SWI {num:02X} (callback decompressor) not emulated");
        }
        _ => return false,
    }
    true
}

/// IntrWait(r0 = discard old flags, r1 = wanted flags). The IRQ handler
/// sets bits in the BIOS check word; the BIOS halts until one of the wanted
/// bits appears. HLE: if not yet set, rewind to the SWI, halt, and re-run it
/// (with r0 = 0) after the next interrupt.
fn intr_wait<C: CpuState, B: CpuBus>(cpu: &mut C, bus: &mut B, thumb: bool) {
    let flags_addr = if cpu.is_armv5() {
        (cpu.mrc(9, 1, 0, 0) & 0xFFFF_F000).wrapping_add(0x3FF8)
    } else {
        0x0380_FFF8
    };
    bus.write32(0x0400_0208, 1); // IME = 1
    let wanted = cpu.reg(1);
    let mut flags = bus.read32(flags_addr);
    if cpu.reg(0) != 0 {
        flags &= !wanted;
        bus.write32(flags_addr, flags);
        cpu.set_reg(0, 0);
    }
    if flags & wanted != 0 {
        bus.write32(flags_addr, flags & !wanted);
        return;
    }
    // Wait: IRQs must be able to fire, then re-execute this SWI.
    cpu.set_cpsr(cpu.cpsr() & !FLAG_I);
    let back = if thumb { 2 } else { 4 };
    cpu.set_reg(15, cpu.reg(15).wrapping_sub(back));
    cpu.set_halted(true);
}

fn cpu_set<C: CpuState, B: CpuBus>(cpu: &mut C, bus: &mut B) {
    let (mut src, mut dst, cnt) = (cpu.reg(0), cpu.reg(1), cpu.reg(2));
    let count = cnt & 0x1F_FFFF;
    let fixed = cnt & (1 << 24) != 0;
    if cnt & (1 << 26) != 0 {
        let (mut s, mut d) = (src & !3, dst & !3);
        for _ in 0..count {
            let v = bus.read32(s);
            bus.write32(d, v);
            d = d.wrapping_add(4);
            if !fixed {
                s = s.wrapping_add(4);
            }
        }
    } else {
        src &= !1;
        dst &= !1;
        for _ in 0..count {
            let v = bus.read16(src);
            bus.write16(dst, v);
            dst = dst.wrapping_add(2);
            if !fixed {
                src = src.wrapping_add(2);
            }
        }
    }
}

fn cpu_fast_set<C: CpuState, B: CpuBus>(cpu: &mut C, bus: &mut B) {
    let (mut src, mut dst, cnt) = (cpu.reg(0) & !3, cpu.reg(1) & !3, cpu.reg(2));
    // Word count, rounded up to a multiple of 8.
    let count = ((cnt & 0x1F_FFFF) + 7) & !7;
    let fixed = cnt & (1 << 24) != 0;
    for _ in 0..count {
        let v = bus.read32(src);
        bus.write32(dst, v);
        dst = dst.wrapping_add(4);
        if !fixed {
            src = src.wrapping_add(4);
        }
    }
}

fn isqrt(v: u64) -> u64 {
    let mut r = (v as f64).sqrt() as u64;
    while r * r > v {
        r -= 1;
    }
    while (r + 1) * (r + 1) <= v {
        r += 1;
    }
    r
}

/// Volume table (724 bytes) for index = dB*10 + 723, dB in -72.3..0.
/// The Nitro sound driver pairs it with a divider shift of 0/1/2/4
/// (>= -6 dB, >= -12 dB, >= -24 dB, below), so each segment is scaled up.
fn volume_table(index: u32) -> u8 {
    let i = index.min(723) as i32;
    let db10 = i - 723; // tenths of a dB, <= 0
    let scale = if db10 >= -60 {
        1.0
    } else if db10 >= -120 {
        2.0
    } else if db10 >= -240 {
        4.0
    } else {
        16.0
    };
    let v = 127.0 * 10f64.powf(db10 as f64 / 200.0) * scale;
    v.round().min(127.0) as u8
}

const MAX_OUT: usize = 16 << 20;

fn lz77<B: CpuBus>(bus: &mut B, src: u32) -> Vec<u8> {
    let header = bus.read32(src);
    let size = ((header >> 8) as usize).min(MAX_OUT);
    let mut out = Vec::with_capacity(size);
    let mut p = src.wrapping_add(4);
    while out.len() < size {
        let flags = bus.read8(p);
        p = p.wrapping_add(1);
        for bit in (0..8).rev() {
            if out.len() >= size {
                break;
            }
            if flags & (1 << bit) == 0 {
                out.push(bus.read8(p));
                p = p.wrapping_add(1);
            } else {
                let b0 = bus.read8(p) as usize;
                let b1 = bus.read8(p.wrapping_add(1)) as usize;
                p = p.wrapping_add(2);
                let len = (b0 >> 4) + 3;
                let disp = (((b0 & 0xF) << 8) | b1) + 1;
                for _ in 0..len {
                    if out.len() >= size {
                        break;
                    }
                    let v = if disp <= out.len() { out[out.len() - disp] } else { 0 };
                    out.push(v);
                }
            }
        }
    }
    out
}

fn run_length<B: CpuBus>(bus: &mut B, src: u32) -> Vec<u8> {
    let header = bus.read32(src);
    let size = ((header >> 8) as usize).min(MAX_OUT);
    let mut out = Vec::with_capacity(size);
    let mut p = src.wrapping_add(4);
    while out.len() < size {
        let flag = bus.read8(p);
        p = p.wrapping_add(1);
        if flag & 0x80 != 0 {
            let len = (flag & 0x7F) as usize + 3;
            let v = bus.read8(p);
            p = p.wrapping_add(1);
            for _ in 0..len.min(size - out.len()) {
                out.push(v);
            }
        } else {
            let len = (flag & 0x7F) as usize + 1;
            for _ in 0..len.min(size - out.len()) {
                out.push(bus.read8(p));
                p = p.wrapping_add(1);
            }
        }
    }
    out
}

fn write_out<B: CpuBus>(bus: &mut B, dst: u32, data: &[u8], halfwords: bool) {
    if halfwords {
        for (i, pair) in data.chunks(2).enumerate() {
            let v = pair[0] as u16 | (*pair.get(1).unwrap_or(&0) as u16) << 8;
            bus.write16(dst.wrapping_add(i as u32 * 2), v);
        }
    } else {
        for (i, &b) in data.iter().enumerate() {
            bus.write8(dst.wrapping_add(i as u32), b);
        }
    }
}

fn bit_unpack<C: CpuState, B: CpuBus>(cpu: &mut C, bus: &mut B) {
    let (src, dst, info) = (cpu.reg(0), cpu.reg(1), cpu.reg(2));
    let src_len = bus.read16(info) as u32;
    let src_w = bus.read8(info.wrapping_add(2)) as u32;
    let dst_w = bus.read8(info.wrapping_add(3)) as u32;
    let data = bus.read32(info.wrapping_add(4));
    let offset = data & 0x7FFF_FFFF;
    let zero_too = data & 0x8000_0000 != 0;
    if !matches!(src_w, 1 | 2 | 4 | 8) || !matches!(dst_w, 1 | 2 | 4 | 8 | 16 | 32) {
        return;
    }
    let mut out: u32 = 0;
    let mut out_bits = 0;
    let mut d = dst;
    for i in 0..src_len {
        let byte = bus.read8(src.wrapping_add(i)) as u32;
        let mut bit = 0;
        while bit < 8 {
            let mut v = (byte >> bit) & ((1 << src_w) - 1);
            if v != 0 || zero_too {
                v = v.wrapping_add(offset);
            }
            out |= (v & (((1u64 << dst_w) - 1) as u32)) << out_bits;
            out_bits += dst_w;
            if out_bits >= 32 {
                bus.write32(d, out);
                d = d.wrapping_add(4);
                out = 0;
                out_bits = 0;
            }
            bit += src_w;
        }
    }
}

fn diff_unfilter<C: CpuState, B: CpuBus>(cpu: &mut C, bus: &mut B, sixteen: bool) {
    let (src, dst) = (cpu.reg(0), cpu.reg(1));
    let size = (bus.read32(src) >> 8).min(MAX_OUT as u32);
    let mut p = src.wrapping_add(4);
    if sixteen {
        let mut acc: u16 = 0;
        for i in (0..size).step_by(2) {
            acc = acc.wrapping_add(bus.read16(p));
            p = p.wrapping_add(2);
            bus.write16(dst.wrapping_add(i), acc);
        }
    } else {
        let mut acc: u8 = 0;
        for i in 0..size {
            acc = acc.wrapping_add(bus.read8(p));
            p = p.wrapping_add(1);
            bus.write8(dst.wrapping_add(i), acc);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_table_endpoints() {
        assert_eq!(volume_table(723), 127);
        assert_eq!(volume_table(0), 0);
        // At exactly -6 dB the value halves (~64); one step lower the x2
        // channel divider kicks in, so the table jumps back to ~127.
        assert_eq!(volume_table(723 - 60), 64);
        assert!(volume_table(723 - 61) >= 126);
    }

    #[test]
    fn isqrt_exact() {
        assert_eq!(isqrt(0), 0);
        assert_eq!(isqrt(15), 3);
        assert_eq!(isqrt(16), 4);
        assert_eq!(isqrt(u32::MAX as u64), 65535);
    }
}
