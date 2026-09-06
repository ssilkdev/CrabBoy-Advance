//! ARM7TDMI ALU & Barrel Shifter

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShiftType {
    Lsl = 0,
    Lsr = 1,
    Asr = 2,
    Ror = 3,
}

impl ShiftType {
    pub fn from_u32(val: u32) -> Self {
        match val & 3 {
            0 => ShiftType::Lsl,
            1 => ShiftType::Lsr,
            2 => ShiftType::Asr,
            3 => ShiftType::Ror,
            _ => unreachable!(),
        }
    }
}

/// Computes barrel shift with carry out for immediate and register shifts
#[inline]
pub fn barrel_shift(shift_type: ShiftType, value: u32, shift_amount: u32, carry_in: bool, immediate: bool) -> (u32, bool) {
    match shift_type {
        ShiftType::Lsl => {
            if shift_amount == 0 {
                (value, carry_in)
            } else if shift_amount < 32 {
                let carry_out = (value & (1 << (32 - shift_amount))) != 0;
                (value << shift_amount, carry_out)
            } else if shift_amount == 32 {
                let carry_out = (value & 1) != 0;
                (0, carry_out)
            } else {
                (0, false)
            }
        }
        ShiftType::Lsr => {
            if shift_amount == 0 {
                if immediate {
                    // LSR #0 is encoded as LSR #32
                    let carry_out = (value & 0x8000_0000) != 0;
                    (0, carry_out)
                } else {
                    (value, carry_in)
                }
            } else if shift_amount < 32 {
                let carry_out = (value & (1 << (shift_amount - 1))) != 0;
                (value >> shift_amount, carry_out)
            } else if shift_amount == 32 {
                let carry_out = (value & 0x8000_0000) != 0;
                (0, carry_out)
            } else {
                (0, false)
            }
        }
        ShiftType::Asr => {
            if shift_amount == 0 {
                if immediate {
                    // ASR #0 is encoded as ASR #32
                    let sign = (value & 0x8000_0000) != 0;
                    (if sign { 0xFFFF_FFFF } else { 0 }, sign)
                } else {
                    (value, carry_in)
                }
            } else if shift_amount < 32 {
                let carry_out = (value & (1 << (shift_amount - 1))) != 0;
                (((value as i32) >> shift_amount) as u32, carry_out)
            } else {
                let sign = (value & 0x8000_0000) != 0;
                (if sign { 0xFFFF_FFFF } else { 0 }, sign)
            }
        }
        ShiftType::Ror => {
            if shift_amount == 0 {
                if immediate {
                    // ROR #0 is encoded as RRX (Rotate Right with eXtend)
                    let carry_out = (value & 1) != 0;
                    let result = (if carry_in { 0x8000_0000 } else { 0 }) | (value >> 1);
                    (result, carry_out)
                } else {
                    (value, carry_in)
                }
            } else {
                let eff_shift = shift_amount & 31;
                if eff_shift == 0 {
                    let carry_out = (value & 0x8000_0000) != 0;
                    (value, carry_out)
                } else {
                    let carry_out = (value & (1 << (eff_shift - 1))) != 0;
                    (value.rotate_right(eff_shift), carry_out)
                }
            }
        }
    }
}

#[inline]
pub fn add_with_carry(a: u32, b: u32, carry_in: bool) -> (u32, bool, bool) {
    let c_in = carry_in as u64;
    let sum = (a as u64) + (b as u64) + c_in;
    let res = sum as u32;
    let carry_out = sum > 0xFFFF_FFFF;
    // Overflow occurs if signs of a and b are identical, and sign of result differs
    let overflow = (!(a ^ b) & (a ^ res) & 0x8000_0000) != 0;
    (res, carry_out, overflow)
}

#[inline]
pub fn sub_with_borrow(a: u32, b: u32, carry_in: bool) -> (u32, bool, bool) {
    // In ARM, SUB carry flag is NOT-borrow: carry = 1 if no borrow occurred (a >= b + !c_in)
    add_with_carry(a, !b, carry_in)
}
