//! Bit-exact GBA BIOS math routines with cycle estimates (ROADMAP M1).
//!
//! Games rely on the BIOS's integer results, not on mathematically exact
//! ones: ArcTan is a fixed polynomial, Sqrt an iterative integer root, and
//! Div leaves specific values in r1/r3. These functions reproduce the
//! algorithms of the real BIOS (as documented in GBATEK and by the mGBA
//! project's reverse engineering of it) along with how long each takes, so
//! code that times them, or depends on their rounding, behaves as on
//! hardware.

/// Cycles an ARM7TDMI multiply spends on its multiplier early-termination
/// check, based on the top bits of the operand.
fn mul_wait(r: i32) -> u32 {
    let r = r as u32;
    if r & 0xFFFF_FF00 == 0xFFFF_FF00 || r & 0xFFFF_FF00 == 0 {
        1
    } else if r & 0xFFFF_0000 == 0xFFFF_0000 || r & 0xFFFF_0000 == 0 {
        2
    } else if r & 0xFF00_0000 == 0xFF00_0000 || r & 0xFF00_0000 == 0 {
        3
    } else {
        4
    }
}

/// SWI 0x06 Div result: (r0, r1, r3, cycles).
///
/// Division by zero hangs on hardware for |num| > 1; like other HLE BIOSes
/// this returns +-1 instead of hanging.
pub fn div(num: i32, den: i32) -> (u32, u32, u32, u32) {
    let (q, r, a) = if den == 0 {
        let q: i32 = if num < 0 { -1 } else { 1 };
        (q as u32, num as u32, 1)
    } else if num == i32::MIN && den == -1 {
        (i32::MIN as u32, 0, i32::MIN as u32)
    } else {
        let q = num / den;
        (q as u32, (num % den) as u32, q.unsigned_abs())
    };
    // The BIOS divides by repeated shift-and-subtract: one 13-cycle loop
    // per bit of difference between the operands' magnitudes.
    let loops = (den.leading_zeros() as i32 - num.leading_zeros() as i32).max(1) as u32;
    (q, r, a, 4 + 13 * loops + 7)
}

/// SWI 0x08 Sqrt: (result, cycles).
pub fn sqrt(x: u32) -> (u32, u32) {
    if x == 0 {
        return (0, 53);
    }
    let mut cycles = 15u32;
    let mut upper = x;
    let mut bound: u32 = 1;
    while bound < upper {
        upper >>= 1;
        bound <<= 1;
        cycles += 6;
    }
    loop {
        cycles += 6;
        upper = x;
        let mut accum: u32 = 0;
        let mut lower = bound;
        loop {
            cycles += 5;
            let old_lower = lower;
            if lower <= upper >> 1 {
                lower <<= 1;
            }
            if old_lower >= upper >> 1 {
                break;
            }
        }
        loop {
            cycles += 8;
            accum <<= 1;
            if upper >= lower {
                accum += 1;
                upper -= lower;
            }
            if lower == bound {
                break;
            }
            lower >>= 1;
        }
        let old_bound = bound;
        bound = bound.wrapping_add(accum) >> 1;
        if bound >= old_bound {
            return (old_bound, cycles);
        }
    }
}

/// SWI 0x09 ArcTan: (r0, r1, r3, cycles). `i` is tan in 1.14 fixed point.
pub fn arctan(i: i32) -> (u32, u32, u32, u32) {
    let mut cycles = 37u32;
    let sq = i.wrapping_mul(i);
    cycles += mul_wait(sq);
    let a = -(sq >> 14);
    let mut b = 0xA9i32.wrapping_mul(a);
    cycles += mul_wait(b);
    b = (b >> 14) + 0x390;
    for c in [0x91C, 0xFB6, 0x16AA, 0x2081, 0x3651, 0xA2F9] {
        let p = b.wrapping_mul(a);
        cycles += mul_wait(p);
        b = (p >> 14) + c;
    }
    let r0 = (i.wrapping_mul(b) >> 16) as i16 as i32 as u32;
    (r0, a as u32, b as u32, cycles)
}

/// SWI 0x0A ArcTan2: (r0, r1, cycles). Returns the angle 0..0xFFFF.
pub fn arctan2(x: i32, y: i32) -> (u32, u32, u32) {
    let at = |t: i32| {
        let (r0, r1, _, c) = arctan(t);
        (r0 as i32, r1, c)
    };
    if y == 0 {
        return (if x >= 0 { 0 } else { 0x8000 }, 0, 11);
    }
    if x == 0 {
        return (if y >= 0 { 0x4000 } else { 0xC000 }, 0, 11);
    }
    let (angle, r1, c) = if y >= 0 {
        if x >= 0 && x >= y {
            at((y << 14) / x)
        } else if x < 0 && -x >= y {
            let (a, r1, c) = at((y << 14) / x);
            (a + 0x8000, r1, c)
        } else {
            let (a, r1, c) = at((x << 14) / y);
            (0x4000 - a, r1, c)
        }
    } else if x <= 0 && -x > -y {
        let (a, r1, c) = at((y << 14) / x);
        (a + 0x8000, r1, c)
    } else if x > 0 && x >= -y {
        let (a, r1, c) = at((y << 14) / x);
        (a + 0x10000, r1, c)
    } else {
        let (a, r1, c) = at((x << 14) / y);
        (0xC000 - a, r1, c)
    };
    ((angle as u32) & 0xFFFF, r1, c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqrt_is_integer_floor() {
        for x in [1u32, 2, 3, 4, 15, 16, 17, 1 << 20, 0xFFFF_FFFF] {
            let (r, _) = sqrt(x);
            let r = r as u64;
            assert!(r * r <= x as u64 && (r + 1) * (r + 1) > x as u64, "sqrt({x}) = {r}");
        }
    }

    #[test]
    fn arctan2_quadrants() {
        assert_eq!(arctan2(1, 0).0, 0);
        assert_eq!(arctan2(0, 1).0, 0x4000);
        assert_eq!(arctan2(-1, 0).0, 0x8000);
        assert_eq!(arctan2(0, -1).0, 0xC000);
        // 45 degrees is 0x2000 (within the polynomial's rounding).
        let a = arctan2(0x100, 0x100).0 as i32;
        assert!((a - 0x2000).abs() <= 2, "{a:#x}");
    }

    #[test]
    fn div_matches_bios_registers() {
        let (q, r, a, _) = div(-7, 2);
        assert_eq!((q, r, a), ((-3i32) as u32, (-1i32) as u32, 3));
        assert_eq!(div(5, 0).0, 1);
    }
}
