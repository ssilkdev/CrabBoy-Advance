//! GBA Cartridge Real-Time Clock (RTC) Emulation (Seiko S-3511A)
//! Used by compatible cartridges for in-game clock, tides, day/night cycles.

use std::time::{SystemTime, UNIX_EPOCH};

pub struct Rtc {
    pub enabled: bool,
    pub time_offset_secs: i64,
    data_reg: u8,
    dir_reg: u8,
    control_reg: u8,
    state: RtcState,
    command: u8,
    cmd_bits_received: u8,
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
            control_reg: 0,
            state: RtcState::Idle,
            command: 0,
            cmd_bits_received: 0,
            buffer: [0; 7],
            buf_bit_idx: 0,
        }
    }

    pub fn read8(&self, addr: u32) -> u8 {
        match addr & 0xFFFF {
            0x00C4 => self.data_reg,
            0x00C6 => self.dir_reg,
            0x00C8 => self.control_reg,
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
                self.control_reg = val & 1;
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
            return;
        }

        // On SCK rising edge: read SIO
        if !old_sck && sck {
            match self.state {
                RtcState::Idle => {
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
                    // Write data bit from game pak to RTC
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
                    }
                }
            }
        }

        // On SCK falling edge: write SIO bit from RTC to game pak
        if old_sck && !sck {
            if self.state == RtcState::TransferData && (self.dir_reg & 2) == 0 {
                let byte_idx = self.buf_bit_idx / 8;
                let bit_idx = self.buf_bit_idx % 8;
                if byte_idx < self.buffer.len() {
                    let bit = (self.buffer[byte_idx] >> bit_idx) & 1;
                    self.data_reg = (self.data_reg & !2) | (bit << 1);
                } else {
                    self.data_reg &= !2;
                }
                self.buf_bit_idx += 1;
            }
        }
    }

    fn process_command(&mut self) {
        // Bits: 0 1 1 0 [Cmd 3:0]
        let is_read = (self.command & 1) != 0;
        let reg_type = (self.command >> 1) & 0x07;

        if is_read {
            // Populate buffer with current time
            self.load_current_time();
            self.state = RtcState::TransferData;
            self.buf_bit_idx = 0;
        } else {
            self.state = RtcState::TransferData;
            self.buf_bit_idx = 0;
        }
        let _ = reg_type;
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
        for m in 0..12 {
            if total_days < mdays[m] {
                month = m as u8 + 1;
                break;
            }
            total_days -= mdays[m];
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
        let rtc_year = ((year - 2000).max(0).min(99)) as u8;

        self.buffer[0] = to_bcd(rtc_year);
        self.buffer[1] = to_bcd(month);
        self.buffer[2] = to_bcd(day);
        self.buffer[3] = dow;
        self.buffer[4] = to_bcd(hrs);
        self.buffer[5] = to_bcd(mins);
        self.buffer[6] = to_bcd(secs);
    }
}
