//! Minimal DS Wi-Fi hardware (ARM7, 0x0480_0000..0x0480_FFFF).
//!
//! No networking: this only gives the firmware/game Wi-Fi drivers the
//! registers they probe at start-up so they initialise cleanly (GBATEK "DS
//! Wireless Communications"). Pokemon Platinum, for one, reports "A
//! communication error has occurred" when the baseband chip never answers.
//! - Registers 0x000..0xFFF (mirrored in both 32 KiB halves) read back what
//!   was written, with a few read-only / live ones.
//! - Wi-Fi RAM at 0x4000..0x5FFF (8 KiB).
//! - Baseband (BB) and RF serial ports complete instantly.

pub struct Wifi {
    regs: Vec<u16>,
    ram: Vec<u8>,
    bb: [u8; 0x100],
    bb_read: u16,
    random: u16,
}

impl Default for Wifi {
    fn default() -> Self {
        Self::new()
    }
}

impl Wifi {
    pub fn new() -> Self {
        let mut bb = [0u8; 0x100];
        bb[0x00] = 0x6D; // chip ID
        bb[0x01] = 0x9E;
        bb[0x5D] = 0x01;
        bb[0x64] = 0xFF;
        let mut w = Self { regs: vec![0; 0x800], ram: vec![0; 0x2000], bb, bb_read: 0, random: 0x7FF };
        // Power-on values from the GBATEK I/O map.
        w.regs[0x02C / 2] = 0x0707; // W_TX_RETRYLIMIT
        w.regs[0x036 / 2] = 0x0001; // W_POWER_US
        w.regs[0x038 / 2] = 0x0003; // W_POWER_TX
        w.regs[0x03C / 2] = 0x0200; // W_POWERSTATE
        w.regs[0x050 / 2] = 0x4000; // W_RXBUF_BEGIN
        w.regs[0x052 / 2] = 0x4800; // W_RXBUF_END
        w.regs[0x08C / 2] = 0x0064; // W_BEACONINT
        w
    }

    #[inline]
    fn owns_ram(off: u32) -> Option<usize> {
        (0x4000..0x6000).contains(&off).then(|| (off - 0x4000) as usize)
    }

    pub fn read16(&mut self, addr: u32) -> u16 {
        let off = addr & 0x7FFE;
        if let Some(o) = Self::owns_ram(off) {
            return u16::from_le_bytes([self.ram[o], self.ram[o + 1]]);
        }
        if off >= 0x1000 {
            return 0;
        }
        match off {
            0x000 => 0x1440, // W_ID: DS
            0x044 => {
                // W_RANDOM: 11-bit value that changes on every read.
                self.random = ((self.random << 1) | (((self.random >> 10) ^ (self.random >> 8)) & 1)) & 0x7FF;
                self.random
            }
            0x15C => self.bb_read,  // W_BB_READ
            0x15E => 0,             // W_BB_BUSY: never busy
            0x180 => 0,             // W_RF_BUSY: never busy
            _ => self.regs[(off / 2) as usize],
        }
    }

    pub fn write16(&mut self, addr: u32, val: u16) {
        let off = addr & 0x7FFE;
        if let Some(o) = Self::owns_ram(off) {
            self.ram[o..o + 2].copy_from_slice(&val.to_le_bytes());
            return;
        }
        if off >= 0x1000 {
            return;
        }
        match off {
            0x000 | 0x044 | 0x15C | 0x15E | 0x180 | 0x214 => return, // read-only
            0x158 => {
                // W_BB_CNT: index in bits 0-7, 5 = write, 6 = read.
                let idx = (val & 0xFF) as usize;
                match val >> 12 {
                    5 => {
                        // Read-only / constant BB registers ignore writes.
                        if !matches!(idx, 0x00 | 0x0D..=0x12 | 0x16..=0x1A | 0x27 | 0x4D | 0x5D..=0x61 | 0x64 | 0x66 | 0x69..) {
                            self.bb[idx] = self.regs[0x15A / 2] as u8;
                        }
                    }
                    6 => self.bb_read = self.bb[idx] as u16,
                    _ => {}
                }
            }
            0x010 => {
                // W_IF: write 1 to acknowledge.
                self.regs[0x010 / 2] &= !val;
                return;
            }
            0x040 => {
                // W_POWERFORCE: bit 15 applies bit 0 to W_POWERSTATE bit 9
                // immediately. Powering down puts the RF state machine idle.
                if val & 0x8000 != 0 {
                    let ps = &mut self.regs[0x03C / 2];
                    if val & 1 != 0 {
                        *ps = (*ps & !0x0300) | 0x0200;
                        self.regs[0x214 / 2] = 9; // W_RF_STATUS: idle
                        self.regs[0x034 / 2] = 2; // W_INTERNAL
                    } else {
                        *ps &= !0x0300;
                    }
                }
            }
            0x03C => {
                // W_POWERSTATE: bits 8-9 are read-only; bit 1 requests
                // power-up, which (with no real radio) completes at once.
                let ps = &mut self.regs[0x03C / 2];
                *ps = (*ps & 0x0300) | (val & 0x0003);
                if val & 2 != 0 {
                    *ps &= !0x0302;
                    self.regs[0x214 / 2] = 1; // W_RF_STATUS: RX mode
                }
                return;
            }
            0x004 => {
                // W_MODE_RST bit 0 starts the MAC (RF leaves "initial" state).
                if val & 1 != 0 && self.regs[0x214 / 2] == 0 {
                    self.regs[0x214 / 2] = 9;
                }
            }
            _ => {}
        }
        self.regs[(off / 2) as usize] = val;
    }

    pub fn read32(&mut self, addr: u32) -> u32 {
        self.read16(addr) as u32 | ((self.read16(addr + 2) as u32) << 16)
    }

    pub fn write32(&mut self, addr: u32, val: u32) {
        self.write16(addr, val as u16);
        self.write16(addr + 2, (val >> 16) as u16);
    }

    pub fn read8(&mut self, addr: u32) -> u8 {
        (self.read16(addr & !1) >> ((addr & 1) * 8)) as u8
    }
}

#[cfg(test)]
mod tests {
    use super::Wifi;

    #[test]
    fn baseband_reads_chip_id_and_round_trips() {
        let mut w = Wifi::new();
        assert_eq!(w.read16(0x0480_8000), 0x1440);
        w.write16(0x0480_8158, 0x6000); // read BB[0]
        assert_eq!(w.read16(0x0480_815E), 0, "never busy");
        assert_eq!(w.read16(0x0480_815C), 0x6D);
        w.write16(0x0480_815A, 0x42);
        w.write16(0x0480_8158, 0x5013); // write BB[13h]
        w.write16(0x0480_8158, 0x6013);
        assert_eq!(w.read16(0x0480_815C), 0x42);
        // RAM, mirrored in the lower half too.
        w.write16(0x0480_4010, 0xBEEF);
        assert_eq!(w.read16(0x0480_C010), 0xBEEF);
        // Forced power-down: W_POWERSTATE bit 9 set, RF idle (9).
        w.write16(0x0480_8040, 0x8001);
        assert_eq!(w.read16(0x0480_803C) >> 8, 2);
        assert_eq!(w.read16(0x0480_8214), 9);
        // W_RANDOM moves.
        let a = w.read16(0x0480_8044);
        let b = w.read16(0x0480_8044);
        assert_ne!(a, b);
    }
}
