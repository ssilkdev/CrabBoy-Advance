//! GBA Cartridge Real-Time Clock (RTC) Emulation (Seiko S-3511A)
//! Used by compatible cartridges for in-game clock, tides, day/night cycles.

use std::time::{SystemTime, UNIX_EPOCH};

pub struct Rtc {
    pub enabled: bool,
    pub time_offset_secs: i64,
    data_reg: u8,
    dir_reg: u8,
    gpio_control: u8,
    rtc_control: u8,
    state: RtcState,
    command: u8,
    cmd_bits_received: u8,
    bytes_to_transfer: usize,
    buffer: [u8; 7],
    buf_bit_idx: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RtcState {
    Idle,
    Command,
    TransferData,
}

impl Default for Rtc {
    fn default() -> Self {
        Self::new()
    }
}

impl Rtc {
    pub fn new() -> Self {
        Self {
            enabled: false,
            time_offset_secs: 0,
            data_reg: 0,
            dir_reg: 0,
            gpio_control: 0,
            rtc_control: 0x40, // Bit 6 = 24-hour mode enabled, Power-off flag cleared
            state: RtcState::Idle,
            command: 0,
            cmd_bits_received: 0,
            bytes_to_transfer: 0,
            buffer: [0; 7],
            buf_bit_idx: 0,
        }
    }

    pub fn read8(&self, addr: u32) -> u8 {
        match addr & 0xFFFF {
            0x00C4 => self.data_reg,
            0x00C6 => self.dir_reg,
            0x00C8 => self.gpio_control,
            _ => 0,
        }
    }

    pub fn write8(&mut self, addr: u32, val: u8) {
        match addr & 0xFFFF {
            0x00C4 => {
                let old_data = self.data_reg;
                self.data_reg = (self.data_reg & !self.dir_reg) | (val & self.dir_reg);
                self.step_gpio(old_data, self.data_reg);
            }
            0x00C6 => {
                self.dir_reg = val & 0x0F;
            }
            0x00C8 => {
                self.gpio_control = val & 1;
                self.enabled = (val & 1) != 0;
            }
            _ => {}
        }
    }

    fn step_gpio(&mut self, old_val: u8, new_val: u8) {
        let cs = (new_val & 4) != 0;
        let old_sck = (old_val & 1) != 0;
        let sck = (new_val & 1) != 0;
        let sio_in = (new_val & 2) != 0;

        if !cs {
            self.state = RtcState::Idle;
            self.cmd_bits_received = 0;
            self.buf_bit_idx = 0;
            return;
        }

        // On SCK rising edge: read SIO (LSB first)
        if !old_sck && sck {
            match self.state {
                RtcState::Idle => {
                    // Unlike ordinary data bytes (LSB-first), the RTC command byte
                    // is clocked in MSB-first: `0110 bbb r` (magic nibble, 3-bit
                    // register, read/write flag).
                    self.state = RtcState::Command;
                    self.command = if sio_in { 1 } else { 0 };
                    self.cmd_bits_received = 1;
                }
                RtcState::Command => {
                    self.command = (self.command << 1) | (if sio_in { 1 } else { 0 });
                    self.cmd_bits_received += 1;
                    if self.cmd_bits_received == 8 {
                        self.process_command();
                    }
                }
                RtcState::TransferData => {
                    // Write data bit from game pak to RTC (LSB first)
                    if (self.dir_reg & 2) != 0 {
                        let byte_idx = self.buf_bit_idx / 8;
                        let bit_idx = self.buf_bit_idx % 8;
                        if byte_idx < self.buffer.len() {
                            if sio_in {
                                self.buffer[byte_idx] |= 1 << bit_idx;
                            } else {
                                self.buffer[byte_idx] &= !(1 << bit_idx);
                            }
                        }
                        self.buf_bit_idx += 1;
                        if self.buf_bit_idx >= self.bytes_to_transfer * 8 {
                            self.finish_write();
                            self.state = RtcState::Idle;
                        }
                    }
                }
            }
        }

        // On SCK falling edge: write SIO bit from RTC to game pak (LSB first)
        if old_sck && !sck
            && self.state == RtcState::TransferData && (self.dir_reg & 2) == 0 {
                let byte_idx = self.buf_bit_idx / 8;
                let bit_idx = self.buf_bit_idx % 8;
                if byte_idx < self.bytes_to_transfer {
                    let bit = (self.buffer[byte_idx] >> bit_idx) & 1;
                    self.data_reg = (self.data_reg & !2) | (bit << 1);
                } else {
                    self.data_reg &= !2;
                }
                self.buf_bit_idx += 1;
                if self.buf_bit_idx >= self.bytes_to_transfer * 8 {
                    self.state = RtcState::Idle;
                }
            }
    }

    fn process_command(&mut self) {
        // Command byte is clocked in MSB-first as `0110 bbb r`:
        // Magic (4, high nibble) | Register (3) | Read flag (1, low bit)
        let magic = (self.command >> 4) & 0x0F;
        if magic != 0x06 {
            self.state = RtcState::Idle;
            return;
        }

        let is_read = (self.command & 0x01) != 0;
        let cmd = (self.command >> 1) & 0x07;

        self.buf_bit_idx = 0;
        self.state = RtcState::TransferData;

        match cmd {
            0 => {
                // RTC_RESET
                self.rtc_control = 0;
                self.bytes_to_transfer = 0;
                self.state = RtcState::Idle;
            }
            2 => {
                // RTC_DATETIME (7 bytes)
                self.bytes_to_transfer = 7;
                if is_read {
                    self.load_current_time();
                } else {
                    self.buffer = [0; 7];
                }
            }
            3 => {
                // RTC_FORCE_IRQ
                self.bytes_to_transfer = 0;
                self.state = RtcState::Idle;
            }
            4 => {
                // RTC_CONTROL (1 byte)
                self.bytes_to_transfer = 1;
                if is_read {
                    self.buffer[0] = self.rtc_control;
                } else {
                    self.buffer[0] = 0;
                }
            }
            6 => {
                // RTC_TIME (3 bytes: hours, minutes, seconds)
                self.bytes_to_transfer = 3;
                if is_read {
                    self.load_current_time();
                    self.buffer[0] = self.buffer[4];
                    self.buffer[1] = self.buffer[5];
                    self.buffer[2] = self.buffer[6];
                } else {
                    self.buffer = [0; 7];
                }
            }
            _ => {
                self.bytes_to_transfer = 0;
                self.state = RtcState::Idle;
            }
        }
    }

    fn finish_write(&mut self) {
        let cmd = (self.command >> 1) & 0x07;
        if cmd == 4 {
            self.rtc_control = self.buffer[0];
        }
    }

    pub fn add_offset_secs(&mut self, secs: i64) {
        self.time_offset_secs += secs;
    }

    pub fn reset_offset(&mut self) {
        self.time_offset_secs = 0;
    }

    pub fn get_datetime_components(&self) -> (i32, u8, u8, u8, u8, u8, &'static str) {
        let now_unix = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
        let now = (now_unix + self.time_offset_secs).max(0) as u64;

        let secs = (now % 60) as u8;
        let mins = ((now / 60) % 60) as u8;
        let hrs = ((now / 3600) % 24) as u8;
        let mut total_days = (now / 86400) as i64;

        let dow_idx = ((total_days + 4) % 7) as usize;
        let dow_names = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
        let dow = dow_names[dow_idx];

        let mut year = 1970i32;
        loop {
            let days_in_year: i64 = if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 { 366 } else { 365 };
            if total_days < days_in_year {
                break;
            }
            total_days -= days_in_year;
            year += 1;
        }
        let is_leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
        let mdays: [i64; 12] = [31, if is_leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        let mut month = 0u8;
        for (m, &days) in mdays.iter().enumerate() {
            if total_days < days {
                month = m as u8 + 1;
                break;
            }
            total_days -= days;
        }
        if month == 0 { month = 12; }
        let day = total_days as u8 + 1;

        (year, month, day, hrs, mins, secs, dow)
    }

    fn load_current_time(&mut self) {
        fn to_bcd(val: u8) -> u8 {
            ((val / 10) << 4) | (val % 10)
        }

        let (year, month, day, hrs, mins, secs, _) = self.get_datetime_components();
        let dow = {
            let now_unix = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
            let now = (now_unix + self.time_offset_secs).max(0) as u64;
            let total_days = (now / 86400) as i64;
            ((total_days + 4) % 7) as u8
        };

        // GBA RTC year is 2-digit offset from 2000
        let rtc_year = (year - 2000).clamp(0, 99) as u8;

        let bcd_hrs = if (self.rtc_control & 0x40) != 0 {
            to_bcd(hrs)
        } else {
            let ampm = if hrs >= 12 { 0x80 } else { 0 };
            to_bcd(hrs % 12) | ampm
        };

        self.buffer[0] = to_bcd(rtc_year);
        self.buffer[1] = to_bcd(month);
        self.buffer[2] = to_bcd(day);
        self.buffer[3] = dow;
        self.buffer[4] = bcd_hrs;
        self.buffer[5] = to_bcd(mins);
        self.buffer[6] = to_bcd(secs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds the MSB-first command byte `0110 bbb r` for a given register/read-flag,
    /// as it would appear serialized bit-by-bit over the wire.
    fn command_byte(register: u8, is_read: bool) -> u8 {
        (0x06 << 4) | ((register & 0x07) << 1) | (is_read as u8)
    }

    #[test]
    fn command_register_decode_is_not_bit_reversed() {
        // Every documented register number (0,2,3,4,6) must decode to itself,
        // not a bit-reversed value. Registers 0 and 2 are palindromic in the
        // 3-bit field and would pass even with the old buggy LSB-first assembly;
        // 3, 4, and 6 are not, so they're the ones that catch the regression.
        for &register in &[0u8, 2, 3, 4, 6] {
            for &is_read in &[false, true] {
                let mut rtc = Rtc::new();
                rtc.command = command_byte(register, is_read);
                rtc.process_command();
                let decoded_cmd = (rtc.command >> 1) & 0x07;
                assert_eq!(
                    decoded_cmd, register,
                    "register {} (is_read={}) decoded as {}",
                    register, is_read, decoded_cmd
                );
            }
        }
    }

    #[test]
    fn rtc_control_write_then_read_roundtrips() {
        let mut rtc = Rtc::new();

        // WRITE to RTC_CONTROL (register 4): process_command() puts us in
        // TransferData with a zeroed buffer ready to receive the incoming byte.
        rtc.command = command_byte(4, false);
        rtc.process_command();
        assert_eq!(rtc.bytes_to_transfer, 1);
        rtc.buffer[0] = 0x40; // 24-hour mode
        rtc.finish_write();
        assert_eq!(rtc.rtc_control, 0x40);

        // READ from RTC_CONTROL (register 4) should reflect the value just written.
        rtc.command = command_byte(4, true);
        rtc.process_command();
        assert_eq!(rtc.buffer[0], 0x40);
    }

    #[test]
    fn magic_nibble_still_gates_unknown_commands() {
        let mut rtc = Rtc::new();
        rtc.command = 0x00; // magic nibble wrong (not 0110)
        rtc.process_command();
        assert_eq!(rtc.state, RtcState::Idle);
    }
}
