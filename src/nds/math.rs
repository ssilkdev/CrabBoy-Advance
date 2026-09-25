//! ARM9 hardware maths unit: DIVCNT/SQRTCNT at 0x0400_0280..0x0400_02BF
//! (GBATEK "DS Maths Div/Sqrt"). Results are computed on write; the busy
//! bit always reads 0, which only makes the results appear earlier than on
//! hardware.

#[derive(Debug, Clone, Default)]
pub struct MathUnit {
    pub divcnt: u32,
    pub div_numer: u64,
    pub div_denom: u64,
    pub div_result: u64,
    pub div_rem: u64,
    pub sqrtcnt: u32,
    pub sqrt_param: u64,
    pub sqrt_result: u32,
}

impl MathUnit {
    fn divide(&mut self) {
        let mode = self.divcnt & 3;
        // Division by zero flag looks at the full 64-bit denominator.
        if self.div_denom == 0 {
            self.divcnt |= 1 << 14;
        } else {
            self.divcnt &= !(1 << 14);
        }
        let (num, den): (i64, i64) = match mode {
            0 => (self.div_numer as u32 as i32 as i64, self.div_denom as u32 as i32 as i64),
            1 => (self.div_numer as i64, self.div_denom as u32 as i32 as i64),
            _ => (self.div_numer as i64, self.div_denom as i64),
        };
        if den == 0 {
            // Hardware quirk: quotient is +/-1, remainder the numerator.
            self.div_rem = num as u64;
            self.div_result = if num < 0 { 1 } else { u64::MAX };
            if mode == 0 {
                // 32-bit mode: the upper result word is inverted (not sign-extended).
                self.div_result ^= 0xFFFF_FFFF_0000_0000;
            }
            return;
        }
        if mode == 0 && num == i32::MIN as i64 && den == -1 {
            // Overflow: result is 0x80000000, upper word 0.
            self.div_result = 0x8000_0000;
            self.div_rem = 0;
            return;
        }
        if num == i64::MIN && den == -1 {
            self.div_result = i64::MIN as u64;
            self.div_rem = 0;
            return;
        }
        self.div_result = (num / den) as u64;
        self.div_rem = (num % den) as u64;
    }

    fn sqrt(&mut self) {
        let v = if self.sqrtcnt & 1 != 0 { self.sqrt_param } else { self.sqrt_param & 0xFFFF_FFFF };
        // Integer square root (floor), exact for all 64-bit inputs.
        let mut r = (v as f64).sqrt() as u64;
        while r.checked_mul(r).map_or(true, |sq| sq > v) {
            r -= 1;
        }
        while (r + 1).checked_mul(r + 1).map_or(false, |sq| sq <= v) {
            r += 1;
        }
        self.sqrt_result = r as u32;
    }

    pub fn owns(addr: u32) -> bool {
        (0x0400_0280..0x0400_02C0).contains(&addr)
    }

    pub fn read32(&self, addr: u32) -> u32 {
        match addr & !3 {
            0x0400_0280 => self.divcnt & 0x4003,
            0x0400_0290 => self.div_numer as u32,
            0x0400_0294 => (self.div_numer >> 32) as u32,
            0x0400_0298 => self.div_denom as u32,
            0x0400_029C => (self.div_denom >> 32) as u32,
            0x0400_02A0 => self.div_result as u32,
            0x0400_02A4 => (self.div_result >> 32) as u32,
            0x0400_02A8 => self.div_rem as u32,
            0x0400_02AC => (self.div_rem >> 32) as u32,
            0x0400_02B0 => self.sqrtcnt & 1,
            0x0400_02B4 => self.sqrt_result,
            0x0400_02B8 => self.sqrt_param as u32,
            0x0400_02BC => (self.sqrt_param >> 32) as u32,
            _ => 0,
        }
    }

    pub fn write32(&mut self, addr: u32, val: u32) {
        let lo = |x: u64| (x & 0xFFFF_FFFF_0000_0000) | val as u64;
        let hi = |x: u64| (x & 0xFFFF_FFFF) | ((val as u64) << 32);
        match addr & !3 {
            0x0400_0280 => { self.divcnt = val & 3; self.divide(); }
            0x0400_0290 => { self.div_numer = lo(self.div_numer); self.divide(); }
            0x0400_0294 => { self.div_numer = hi(self.div_numer); self.divide(); }
            0x0400_0298 => { self.div_denom = lo(self.div_denom); self.divide(); }
            0x0400_029C => { self.div_denom = hi(self.div_denom); self.divide(); }
            0x0400_02B0 => { self.sqrtcnt = val & 1; self.sqrt(); }
            0x0400_02B8 => { self.sqrt_param = lo(self.sqrt_param); self.sqrt(); }
            0x0400_02BC => { self.sqrt_param = hi(self.sqrt_param); self.sqrt(); }
            _ => {}
        }
    }

    pub fn read16(&self, addr: u32) -> u16 {
        (self.read32(addr) >> ((addr & 2) * 8)) as u16
    }

    pub fn write16(&mut self, addr: u32, val: u16) {
        let shift = (addr & 2) * 8;
        let old = self.read32(addr);
        let merged = (old & !(0xFFFF << shift)) | ((val as u32) << shift);
        self.write32(addr, merged);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn div(mode: u32, num: u64, den: u64) -> (u64, u64, bool) {
        let mut m = MathUnit::default();
        m.write32(0x0400_0280, mode);
        m.write32(0x0400_0290, num as u32);
        m.write32(0x0400_0294, (num >> 32) as u32);
        m.write32(0x0400_0298, den as u32);
        m.write32(0x0400_029C, (den >> 32) as u32);
        (m.div_result, m.div_rem, m.read32(0x0400_0280) & (1 << 14) != 0)
    }

    #[test]
    fn div32_matches_hardware_quirks() {
        // (numLo, numHi, denLo, denHi) -> (resLo, resHi, remLo, remHi) from rockwrestler
        let (r, m, _) = div(0, (44u64 << 32) | (-140i32 as u32 as u64), (1u64 << 32) | 18);
        assert_eq!((r as u32 as i32, (r >> 32) as i32, m as u32 as i32, (m >> 32) as i32), (-7, -1, -14, -1));
        let (r, m, _) = div(0, 0x8000_0000, 0xFFFF_FFFF);
        assert_eq!((r as u32, (r >> 32) as u32, m), (0x8000_0000, 0, 0));
        let (r, m, e) = div(0, 123, 555u64 << 32);
        assert_eq!((r as u32 as i32, (r >> 32) as u32, m as u32, (m >> 32) as u32), (-1, 0, 123, 0));
        assert!(!e, "den64 != 0 so no div-by-zero flag");
        let (r, m, _) = div(0, -123i32 as u32 as u64, 555u64 << 32);
        assert_eq!((r as u32, (r >> 32) as i32, m as u32 as i32, (m >> 32) as i32), (1, -1, -123, -1));
    }

    #[test]
    fn sqrt_floors() {
        let mut m = MathUnit::default();
        for (v, want, mode64) in [(145u64 | (444 << 32), 12u32, false), (4294967295, 65535, false), (u64::MAX, 0xFFFF_FFFF, true)] {
            m.write32(0x0400_02B0, mode64 as u32);
            m.write32(0x0400_02B8, v as u32);
            m.write32(0x0400_02BC, (v >> 32) as u32);
            assert_eq!(m.sqrt_result, want);
        }
    }
}
