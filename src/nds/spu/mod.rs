//! Nintendo DS Sound Processing Unit (SPU)
//!
//! Emulates the 16 hardware sound channels of the Nintendo DS ARM7 subsystem:
//! - Channels 0..15: 8-bit PCM, 16-bit PCM, and 4-bit IMA-ADPCM
//! - Channels 8..13: Programmable PSG square waves with 8 duty cycle steps
//! - Channels 14..15: Pseudo-random white noise generator (15-bit LFSR)
//!
//! Mixed stereo audio is resampled and pushed to CrabBoy's 48 kHz / 44.1 kHz `AudioOutput` sink.

use crate::gba::apu::audio_output::AudioOutput;

pub const ARM7_CLOCK_HZ: f64 = 33_513_982.0;
pub const CORE_SAMPLE_RATE: f64 = 32_768.0;

const INDEX_TABLE: [i32; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];

const STEP_TABLE: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31,
    34, 37, 41, 45, 50, 55, 60, 66, 73, 80, 88, 97, 107, 118, 130, 143,
    157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658,
    724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272, 2499, 2749, 3024,
    3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493, 10442, 11487, 12635, 13899,
    15289, 16818, 18500, 20350, 22385, 24623, 26086, 28694, 31564,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaveFormat {
    Pcm8 = 0,
    Pcm16 = 1,
    Adpcm = 2,
    PsgOrNoise = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepeatMode {
    Manual = 0,
    Loop = 1,
    OneShot = 2,
    Prohibited = 3,
}

#[derive(Debug, Clone)]
pub struct NdsSoundChannel {
    pub id: usize,
    pub cnt: u32,
    pub sad: u32,
    pub tmr: u16,
    pub pnt: u16,
    pub len: u32,

    pub busy: bool,
    pub timer_counter: u32,
    pub current_byte_offset: u32,
    pub current_sample: i32,

    // ADPCM state
    pub adpcm_index: i32,
    pub adpcm_loop_sample: i32,
    pub adpcm_loop_index: i32,
    pub adpcm_high_nibble: bool,

    // PSG & Noise state
    pub psg_step: u8,
    pub lfsr: u16,
}

impl NdsSoundChannel {
    pub fn new(id: usize) -> Self {
        Self {
            id,
            cnt: 0,
            sad: 0,
            tmr: 0,
            pnt: 0,
            len: 0,
            busy: false,
            timer_counter: 0,
            current_byte_offset: 0,
            current_sample: 0,
            adpcm_index: 0,
            adpcm_loop_sample: 0,
            adpcm_loop_index: 0,
            adpcm_high_nibble: false,
            psg_step: 0,
            lfsr: 0x7FFF,
        }
    }

    #[inline]
    pub fn format(&self) -> WaveFormat {
        match (self.cnt >> 29) & 3 {
            0 => WaveFormat::Pcm8,
            1 => WaveFormat::Pcm16,
            2 => WaveFormat::Adpcm,
            3 => WaveFormat::PsgOrNoise,
            _ => WaveFormat::Pcm8,
        }
    }

    #[inline]
    pub fn repeat_mode(&self) -> RepeatMode {
        match (self.cnt >> 27) & 3 {
            0 => RepeatMode::Manual,
            1 => RepeatMode::Loop,
            2 => RepeatMode::OneShot,
            3 => RepeatMode::Prohibited,
            _ => RepeatMode::Manual,
        }
    }

    #[inline]
    pub fn total_length_bytes(&self) -> u32 {
        (self.pnt as u32 + (self.len & 0x003F_FFFF)) * 4
    }

    #[inline]
    pub fn loop_start_bytes(&self) -> u32 {
        (self.pnt as u32) * 4
    }

    pub fn start(&mut self, main_ram: &[u8], shared_wram: &[u8], arm7_wram: &[u8]) {
        self.busy = true;
        self.timer_counter = 0;
        self.psg_step = 0;
        self.lfsr = 0x7FFF;

        match self.format() {
            WaveFormat::Adpcm => {
                let s0 = read_ram(main_ram, shared_wram, arm7_wram, self.sad);
                let s1 = read_ram(main_ram, shared_wram, arm7_wram, self.sad.wrapping_add(1));
                let s2 = read_ram(main_ram, shared_wram, arm7_wram, self.sad.wrapping_add(2));

                self.current_sample = i16::from_le_bytes([s0, s1]) as i32;
                self.adpcm_index = ((s2 & 0x7F) as i32).min(88);
                self.adpcm_loop_sample = self.current_sample;
                self.adpcm_loop_index = self.adpcm_index;
                self.adpcm_high_nibble = false;
                self.current_byte_offset = 4;
            }
            _ => {
                self.current_byte_offset = 0;
                self.current_sample = 0;
            }
        }
    }

    pub fn step_timer(&mut self, cycles: u32, main_ram: &[u8], shared_wram: &[u8], arm7_wram: &[u8]) {
        if !self.busy {
            return;
        }

        let period = (0x10000 - (self.tmr as u32)).max(1);
        self.timer_counter += cycles;

        while self.timer_counter >= period {
            self.timer_counter -= period;
            self.advance_sample(main_ram, shared_wram, arm7_wram);
            if !self.busy {
                break;
            }
        }
    }

    fn advance_sample(&mut self, main_ram: &[u8], shared_wram: &[u8], arm7_wram: &[u8]) {
        let total_bytes = self.total_length_bytes().max(4);

        match self.format() {
            WaveFormat::Pcm8 => {
                let byte = read_ram(main_ram, shared_wram, arm7_wram, self.sad.wrapping_add(self.current_byte_offset));
                self.current_sample = (byte as i8 as i32) << 8;
                self.current_byte_offset += 1;

                if self.current_byte_offset >= total_bytes {
                    match self.repeat_mode() {
                        RepeatMode::OneShot => {
                            self.busy = false;
                            self.cnt &= !(1 << 31);
                            if (self.cnt & (1 << 15)) == 0 {
                                self.current_sample = 0;
                            }
                        }
                        _ => {
                            self.current_byte_offset = self.loop_start_bytes();
                        }
                    }
                }
            }
            WaveFormat::Pcm16 => {
                let b0 = read_ram(main_ram, shared_wram, arm7_wram, self.sad.wrapping_add(self.current_byte_offset));
                let b1 = read_ram(main_ram, shared_wram, arm7_wram, self.sad.wrapping_add(self.current_byte_offset + 1));
                self.current_sample = i16::from_le_bytes([b0, b1]) as i32;
                self.current_byte_offset += 2;

                if self.current_byte_offset >= total_bytes {
                    match self.repeat_mode() {
                        RepeatMode::OneShot => {
                            self.busy = false;
                            self.cnt &= !(1 << 31);
                            if (self.cnt & (1 << 15)) == 0 {
                                self.current_sample = 0;
                            }
                        }
                        _ => {
                            self.current_byte_offset = self.loop_start_bytes();
                        }
                    }
                }
            }
            WaveFormat::Adpcm => {
                // Check if loop position is reached
                if self.current_byte_offset == self.loop_start_bytes() && !self.adpcm_high_nibble {
                    self.adpcm_loop_sample = self.current_sample;
                    self.adpcm_loop_index = self.adpcm_index;
                }

                let byte = read_ram(main_ram, shared_wram, arm7_wram, self.sad.wrapping_add(self.current_byte_offset));
                let nibble = if self.adpcm_high_nibble {
                    self.adpcm_high_nibble = false;
                    self.current_byte_offset += 1;
                    (byte >> 4) & 0x0F
                } else {
                    self.adpcm_high_nibble = true;
                    byte & 0x0F
                };

                // Decode IMA-ADPCM nibble
                let step = STEP_TABLE[self.adpcm_index as usize];
                let mut diff = step >> 3;
                if (nibble & 1) != 0 { diff += step >> 2; }
                if (nibble & 2) != 0 { diff += step >> 1; }
                if (nibble & 4) != 0 { diff += step; }

                if (nibble & 8) != 0 {
                    self.current_sample = (self.current_sample - diff).clamp(-32768, 32767);
                } else {
                    self.current_sample = (self.current_sample + diff).clamp(-32768, 32767);
                }
                self.adpcm_index = (self.adpcm_index + INDEX_TABLE[(nibble & 7) as usize]).clamp(0, 88);

                if self.current_byte_offset >= total_bytes {
                    match self.repeat_mode() {
                        RepeatMode::OneShot => {
                            self.busy = false;
                            self.cnt &= !(1 << 31);
                            if (self.cnt & (1 << 15)) == 0 {
                                self.current_sample = 0;
                            }
                        }
                        _ => {
                            self.current_byte_offset = self.loop_start_bytes();
                            self.current_sample = self.adpcm_loop_sample;
                            self.adpcm_index = self.adpcm_loop_index;
                            self.adpcm_high_nibble = false;
                        }
                    }
                }
            }
            WaveFormat::PsgOrNoise => {
                if self.id < 14 {
                    // Channels 8..13: PSG Square wave
                    self.psg_step = (self.psg_step + 1) & 7;
                    let duty = (self.cnt >> 24) & 7;
                    let is_high = match duty {
                        0 => self.psg_step == 0,
                        1 => self.psg_step < 2,
                        2 => self.psg_step < 3,
                        3 => self.psg_step < 4,
                        4 => self.psg_step < 5,
                        5 => self.psg_step < 6,
                        6 => self.psg_step < 7,
                        _ => false,
                    };
                    self.current_sample = if is_high { 32767 } else { -32767 };
                } else {
                    // Channels 14..15: White Noise LFSR
                    let bit0 = self.lfsr & 1;
                    self.lfsr >>= 1;
                    if bit0 != 0 {
                        self.lfsr ^= 0x6000;
                    }
                    self.current_sample = if (self.lfsr & 1) != 0 { 32767 } else { -32767 };
                }
            }
        }
    }
}

pub struct NdsSpu {
    pub channels: [NdsSoundChannel; 16],
    pub soundcnt: u32,
    pub soundbias: u16,
    pub sndcap0cnt: u8,
    pub sndcap1cnt: u8,
    pub sndcap0dad: u32,
    pub sndcap1dad: u32,
    pub audio_output: AudioOutput,
    sample_timer: f64,
    cycles_per_sample: f64,
    sample_batch: Vec<f32>,
}

impl Default for NdsSpu {
    fn default() -> Self {
        Self::new()
    }
}

impl NdsSpu {
    pub fn new() -> Self {
        let channels = [
            NdsSoundChannel::new(0),
            NdsSoundChannel::new(1),
            NdsSoundChannel::new(2),
            NdsSoundChannel::new(3),
            NdsSoundChannel::new(4),
            NdsSoundChannel::new(5),
            NdsSoundChannel::new(6),
            NdsSoundChannel::new(7),
            NdsSoundChannel::new(8),
            NdsSoundChannel::new(9),
            NdsSoundChannel::new(10),
            NdsSoundChannel::new(11),
            NdsSoundChannel::new(12),
            NdsSoundChannel::new(13),
            NdsSoundChannel::new(14),
            NdsSoundChannel::new(15),
        ];

        let cycles_per_sample = ARM7_CLOCK_HZ / CORE_SAMPLE_RATE;

        Self {
            channels,
            soundcnt: 0x0000_0000,
            soundbias: 0x0200,
            sndcap0cnt: 0,
            sndcap1cnt: 0,
            sndcap0dad: 0,
            sndcap1dad: 0,
            audio_output: AudioOutput::new(),
            sample_timer: 0.0,
            cycles_per_sample,
            sample_batch: Vec::with_capacity(1024),
        }
    }

    pub fn step(&mut self, cycles: u32, main_ram: &[u8], shared_wram: &[u8], arm7_wram: &[u8]) {
        for ch in 0..16 {
            if self.channels[ch].busy {
                self.channels[ch].step_timer(cycles, main_ram, shared_wram, arm7_wram);
            }
        }

        self.sample_timer += cycles as f64;
        while self.sample_timer >= self.cycles_per_sample {
            self.sample_timer -= self.cycles_per_sample;
            self.mix_sample();
        }

        if self.sample_batch.len() >= 512 {
            self.audio_output.push_sample_batch(&self.sample_batch);
            self.sample_batch.clear();
        }
    }

    fn mix_sample(&mut self) {
        let master_enable = (self.soundcnt & (1 << 15)) != 0;
        if !master_enable {
            self.sample_batch.push(0.0);
            self.sample_batch.push(0.0);
            return;
        }

        let master_vol = ((self.soundcnt & 0x7F) as f32) / 127.0;
        let mut left = 0.0f32;
        let mut right = 0.0f32;

        for ch in 0..16 {
            let chan = &self.channels[ch];
            if !chan.busy {
                continue;
            }
            if ch < 8 && self.audio_output.is_channel_muted(ch) {
                continue;
            }

            let vol = ((chan.cnt & 0x7F) as f32) / 127.0;
            let shift_mult = match (chan.cnt >> 8) & 3 {
                0 => 1.0,
                1 => 0.5,
                2 => 0.25,
                3 => 0.0625,
                _ => 1.0,
            };
            let pan = (((chan.cnt >> 16) & 0x7F) as f32) / 127.0;

            let s = (chan.current_sample as f32) / 32768.0;
            let ch_vol = s * vol * shift_mult;

            left += ch_vol * (1.0 - pan);
            right += ch_vol * pan;
        }

        left = (left * master_vol).clamp(-1.0, 1.0);
        right = (right * master_vol).clamp(-1.0, 1.0);

        self.sample_batch.push(left);
        self.sample_batch.push(right);
    }

    pub fn flush_samples(&mut self) {
        if !self.sample_batch.is_empty() {
            self.audio_output.push_sample_batch(&self.sample_batch);
            self.sample_batch.clear();
        }
    }

    // --- Register I/O Handlers -------------------------------------------

    pub fn read_u8(&self, addr: u32) -> u8 {
        match addr {
            0x0400_0400..=0x0400_04FF => {
                let ch = ((addr - 0x0400_0400) / 0x10) as usize;
                let offset = (addr & 0x0F) as usize;
                let word = match offset / 4 {
                    0 => self.channels[ch].cnt,
                    1 => self.channels[ch].sad,
                    2 => (self.channels[ch].tmr as u32) | ((self.channels[ch].pnt as u32) << 16),
                    3 => self.channels[ch].len,
                    _ => 0,
                };
                ((word >> ((offset % 4) * 8)) & 0xFF) as u8
            }
            0x0400_0500 => (self.soundcnt & 0xFF) as u8,
            0x0400_0501 => ((self.soundcnt >> 8) & 0xFF) as u8,
            0x0400_0504 => (self.soundbias & 0xFF) as u8,
            0x0400_0505 => ((self.soundbias >> 8) & 0xFF) as u8,
            0x0400_0508 => self.sndcap0cnt,
            0x0400_0509 => self.sndcap1cnt,
            _ => 0,
        }
    }

    pub fn read_u16(&self, addr: u32) -> u16 {
        match addr {
            0x0400_0400..=0x0400_04FF => {
                let ch = ((addr - 0x0400_0400) / 0x10) as usize;
                let offset = (addr & 0x0E) as usize;
                match offset {
                    0x00 => (self.channels[ch].cnt & 0xFFFF) as u16,
                    0x02 => ((self.channels[ch].cnt >> 16) & 0xFFFF) as u16,
                    0x04 => (self.channels[ch].sad & 0xFFFF) as u16,
                    0x06 => ((self.channels[ch].sad >> 16) & 0xFFFF) as u16,
                    0x08 => self.channels[ch].tmr,
                    0x0A => self.channels[ch].pnt,
                    0x0C => (self.channels[ch].len & 0xFFFF) as u16,
                    0x0E => ((self.channels[ch].len >> 16) & 0xFFFF) as u16,
                    _ => 0,
                }
            }
            0x0400_0500 => self.soundcnt as u16,
            0x0400_0504 => self.soundbias,
            0x0400_0508 => (self.sndcap0cnt as u16) | ((self.sndcap1cnt as u16) << 8),
            _ => 0,
        }
    }

    pub fn read_u32(&self, addr: u32) -> u32 {
        match addr {
            0x0400_0400..=0x0400_04FF => {
                let ch = ((addr - 0x0400_0400) / 0x10) as usize;
                match addr & 0x0C {
                    0x00 => self.channels[ch].cnt,
                    0x04 => self.channels[ch].sad,
                    0x08 => (self.channels[ch].tmr as u32) | ((self.channels[ch].pnt as u32) << 16),
                    0x0C => self.channels[ch].len,
                    _ => 0,
                }
            }
            0x0400_0500 => self.soundcnt,
            0x0400_0504 => self.soundbias as u32,
            0x0400_0508 => (self.sndcap0cnt as u32) | ((self.sndcap1cnt as u32) << 8),
            0x0400_0510 => self.sndcap0dad,
            0x0400_0514 => self.sndcap1dad,
            _ => 0,
        }
    }

    pub fn write_u8(&mut self, addr: u32, val: u8, main_ram: &[u8], shared_wram: &[u8], arm7_wram: &[u8]) {
        match addr {
            0x0400_0400..=0x0400_04FF => {
                let ch = ((addr - 0x0400_0400) / 0x10) as usize;
                let offset = (addr & 0x0F) as usize;
                let shift = (offset % 4) * 8;
                match offset / 4 {
                    0 => {
                        let mut cnt = self.channels[ch].cnt;
                        cnt = (cnt & !(0xFF << shift)) | ((val as u32) << shift);
                        self.write_cnt(ch, cnt, main_ram, shared_wram, arm7_wram);
                    }
                    1 => {
                        self.channels[ch].sad = (self.channels[ch].sad & !(0xFF << shift)) | ((val as u32) << shift);
                    }
                    2 => {
                        if offset < 2 {
                            let s = offset * 8;
                            self.channels[ch].tmr = (self.channels[ch].tmr & !(0xFF << s)) | ((val as u16) << s);
                        } else {
                            let s = (offset - 2) * 8;
                            self.channels[ch].pnt = (self.channels[ch].pnt & !(0xFF << s)) | ((val as u16) << s);
                        }
                    }
                    3 => {
                        self.channels[ch].len = (self.channels[ch].len & !(0xFF << shift)) | ((val as u32) << shift);
                    }
                    _ => {}
                }
            }
            0x0400_0500 => self.soundcnt = (self.soundcnt & 0xFFFF_FF00) | (val as u32),
            0x0400_0501 => self.soundcnt = (self.soundcnt & 0xFFFF_00FF) | ((val as u32) << 8),
            0x0400_0504 => self.soundbias = (self.soundbias & 0xFF00) | (val as u16),
            0x0400_0505 => self.soundbias = (self.soundbias & 0x00FF) | ((val as u16) << 8),
            0x0400_0508 => self.sndcap0cnt = val,
            0x0400_0509 => self.sndcap1cnt = val,
            _ => {}
        }
    }

    pub fn write_u16(&mut self, addr: u32, val: u16, main_ram: &[u8], shared_wram: &[u8], arm7_wram: &[u8]) {
        match addr {
            0x0400_0400..=0x0400_04FF => {
                let ch = ((addr - 0x0400_0400) / 0x10) as usize;
                match addr & 0x0E {
                    0x00 => {
                        let cnt = (self.channels[ch].cnt & 0xFFFF_0000) | (val as u32);
                        self.write_cnt(ch, cnt, main_ram, shared_wram, arm7_wram);
                    }
                    0x02 => {
                        let cnt = (self.channels[ch].cnt & 0x0000_FFFF) | ((val as u32) << 16);
                        self.write_cnt(ch, cnt, main_ram, shared_wram, arm7_wram);
                    }
                    0x04 => self.channels[ch].sad = (self.channels[ch].sad & 0xFFFF_0000) | (val as u32),
                    0x06 => self.channels[ch].sad = (self.channels[ch].sad & 0x0000_FFFF) | ((val as u32) << 16),
                    0x08 => self.channels[ch].tmr = val,
                    0x0A => self.channels[ch].pnt = val,
                    0x0C => self.channels[ch].len = (self.channels[ch].len & 0xFFFF_0000) | (val as u32),
                    0x0E => self.channels[ch].len = (self.channels[ch].len & 0x0000_FFFF) | ((val as u32) << 16),
                    _ => {}
                }
            }
            0x0400_0500 => self.soundcnt = (self.soundcnt & 0xFFFF_0000) | (val as u32),
            0x0400_0504 => self.soundbias = val,
            0x0400_0508 => {
                self.sndcap0cnt = (val & 0xFF) as u8;
                self.sndcap1cnt = (val >> 8) as u8;
            }
            _ => {}
        }
    }

    pub fn write_u32(&mut self, addr: u32, val: u32, main_ram: &[u8], shared_wram: &[u8], arm7_wram: &[u8]) {
        match addr {
            0x0400_0400..=0x0400_04FF => {
                let ch = ((addr - 0x0400_0400) / 0x10) as usize;
                match addr & 0x0C {
                    0x00 => self.write_cnt(ch, val, main_ram, shared_wram, arm7_wram),
                    0x04 => self.channels[ch].sad = val & 0x07FF_FFFF,
                    0x08 => {
                        self.channels[ch].tmr = val as u16;
                        self.channels[ch].pnt = (val >> 16) as u16;
                    }
                    0x0C => self.channels[ch].len = val & 0x003F_FFFF,
                    _ => {}
                }
            }
            0x0400_0500 => self.soundcnt = val,
            0x0400_0504 => self.soundbias = val as u16,
            0x0400_0508 => {
                self.sndcap0cnt = (val & 0xFF) as u8;
                self.sndcap1cnt = ((val >> 8) & 0xFF) as u8;
            }
            0x0400_0510 => self.sndcap0dad = val,
            0x0400_0514 => self.sndcap1dad = val,
            _ => {}
        }
    }

    fn write_cnt(&mut self, ch: usize, val: u32, main_ram: &[u8], shared_wram: &[u8], arm7_wram: &[u8]) {
        let was_busy = self.channels[ch].busy;
        self.channels[ch].cnt = val;
        let now_busy = (val & (1 << 31)) != 0;

        if !was_busy && now_busy {
            self.channels[ch].start(main_ram, shared_wram, arm7_wram);
        } else if !now_busy {
            self.channels[ch].busy = false;
        }
    }
}

#[inline(always)]
fn read_ram(main_ram: &[u8], shared_wram: &[u8], arm7_wram: &[u8], addr: u32) -> u8 {
    match addr {
        0x0200_0000..=0x02FF_FFFF => main_ram[(addr & 0x3F_FFFF) as usize],
        0x0300_0000..=0x037F_FFFF => shared_wram[(addr & 0x7FFF) as usize],
        0x0380_0000..=0x0380_FFFF => arm7_wram[(addr & 0xFFFF) as usize],
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nds_spu_pcm8_playback() {
        let mut spu = NdsSpu::new();
        let mut main_ram = vec![0u8; 0x400000];
        let shared_wram = [0u8; 0x8000];
        let arm7_wram = [0u8; 0x10000];

        // Fill 4 bytes of signed PCM8: [0x40 (64), 0x7F (127), 0xC0 (-64), 0x80 (-128)]
        let sad = 0x0200_1000;
        main_ram[0x1000] = 0x40;
        main_ram[0x1001] = 0x7F;
        main_ram[0x1002] = 0xC0;
        main_ram[0x1003] = 0x80;

        // Configure Channel 0:
        spu.write_u32(0x0400_0404, sad, &main_ram, &shared_wram, &arm7_wram);
        spu.write_u32(0x0400_0408, 0x0000_FFFF, &main_ram, &shared_wram, &arm7_wram);
        spu.write_u32(0x0400_040C, 1, &main_ram, &shared_wram, &arm7_wram);
        spu.write_u16(0x0400_0500, 0x807F, &main_ram, &shared_wram, &arm7_wram);

        // CNT: Start (bit 31), Repeat One-shot (bit 28 = 1), Hold last sample (bit 15), Pan 64 (center), Volume 127
        let cnt = (1 << 31) | (2 << 27) | (1 << 15) | (64 << 16) | 127;
        spu.write_u32(0x0400_0400, cnt, &main_ram, &shared_wram, &arm7_wram);

        assert!(spu.channels[0].busy);

        // Step 1 cycle -> advances to first sample: 0x40 << 8 = 16384
        spu.step(1, &main_ram, &shared_wram, &arm7_wram);
        assert_eq!(spu.channels[0].current_sample, 64 << 8);

        // Step 1 cycle -> advances to second sample: 0x7F << 8 = 32512
        spu.step(1, &main_ram, &shared_wram, &arm7_wram);
        assert_eq!(spu.channels[0].current_sample, 127 << 8);

        // Step 1 cycle -> advances to third sample: -64 << 8 = -16384
        spu.step(1, &main_ram, &shared_wram, &arm7_wram);
        assert_eq!(spu.channels[0].current_sample, (-64i32) << 8);

        // Step 1 cycle -> advances to fourth sample: -128 << 8 = -32768, and one-shot finishes
        spu.step(1, &main_ram, &shared_wram, &arm7_wram);
        assert_eq!(spu.channels[0].current_sample, (-128i32) << 8);
        assert!(!spu.channels[0].busy);
    }

    #[test]
    fn test_nds_spu_pcm16_playback() {
        let mut spu = NdsSpu::new();
        let mut main_ram = vec![0u8; 0x400000];
        let shared_wram = [0u8; 0x8000];
        let arm7_wram = [0u8; 0x10000];

        let sad = 0x0200_2000;
        let s0 = 0x1234i16.to_le_bytes();
        let s1 = (-0x2345i16).to_le_bytes();
        main_ram[0x2000..0x2002].copy_from_slice(&s0);
        main_ram[0x2002..0x2004].copy_from_slice(&s1);

        spu.write_u32(0x0400_0414, sad, &main_ram, &shared_wram, &arm7_wram);
        spu.write_u32(0x0400_0418, 0x0000_FFFF, &main_ram, &shared_wram, &arm7_wram);
        spu.write_u32(0x0400_041C, 1, &main_ram, &shared_wram, &arm7_wram);

        let cnt = (1 << 31) | (1 << 29) | (1 << 27) | (64 << 16) | 127;
        spu.write_u32(0x0400_0410, cnt, &main_ram, &shared_wram, &arm7_wram);

        spu.step(1, &main_ram, &shared_wram, &arm7_wram);
        assert_eq!(spu.channels[1].current_sample, 0x1234);

        spu.step(1, &main_ram, &shared_wram, &arm7_wram);
        assert_eq!(spu.channels[1].current_sample, -0x2345);

        spu.step(1, &main_ram, &shared_wram, &arm7_wram);
        assert_eq!(spu.channels[1].current_sample, 0x1234);
    }

    #[test]
    fn test_nds_spu_psg_square_wave() {
        let mut spu = NdsSpu::new();
        let main_ram = vec![0u8; 0x400000];
        let shared_wram = [0u8; 0x8000];
        let arm7_wram = [0u8; 0x10000];

        spu.write_u32(0x0400_0488, 0x0000_FFFF, &main_ram, &shared_wram, &arm7_wram);
        let cnt = (1 << 31) | (3 << 29) | (3 << 24) | (64 << 16) | 127;
        spu.write_u32(0x0400_0480, cnt, &main_ram, &shared_wram, &arm7_wram);

        let mut samples = Vec::new();
        for _ in 0..8 {
            spu.step(1, &main_ram, &shared_wram, &arm7_wram);
            samples.push(spu.channels[8].current_sample);
        }

        let high_count = samples.iter().filter(|&&s| s == 32767).count();
        let low_count = samples.iter().filter(|&&s| s == -32767).count();
        assert_eq!(high_count, 4);
        assert_eq!(low_count, 4);
    }

    #[test]
    fn test_nds_spu_adpcm_decoding() {
        let mut spu = NdsSpu::new();
        let mut main_ram = vec![0u8; 0x400000];
        let shared_wram = [0u8; 0x8000];
        let arm7_wram = [0u8; 0x10000];

        // SAD header: initial PCM = 1000, initial index = 0
        let sad = 0x0200_3000;
        let init_pcm = 1000i16.to_le_bytes();
        main_ram[0x3000] = init_pcm[0];
        main_ram[0x3001] = init_pcm[1];
        main_ram[0x3002] = 0; // index 0
        main_ram[0x3003] = 0; // padding

        // First byte of nibbles: low nibble = 0x2 (step index 0, step=7: diff = 7 >> 3 + 7 >> 1 = 0 + 3 = 3)
        // high nibble = 0xA (sign bit set -> negative diff)
        main_ram[0x3004] = 0xA2;

        spu.write_u32(0x0400_0424, sad, &main_ram, &shared_wram, &arm7_wram); // Ch 2 SAD
        spu.write_u32(0x0400_0428, 0x0000_FFFF, &main_ram, &shared_wram, &arm7_wram); // TMR/PNT
        spu.write_u32(0x0400_042C, 2, &main_ram, &shared_wram, &arm7_wram); // LEN (2 words = 8 bytes)

        // CNT: Start, Format ADPCM (2 << 29), Volume 127
        let cnt = (1 << 31) | (2 << 29) | (64 << 16) | 127;
        spu.write_u32(0x0400_0420, cnt, &main_ram, &shared_wram, &arm7_wram);

        assert_eq!(spu.channels[2].current_sample, 1000);
        assert_eq!(spu.channels[2].adpcm_index, 0);

        // Step 1: low nibble 0x2 decoded (positive delta)
        spu.step(1, &main_ram, &shared_wram, &arm7_wram);
        assert!(spu.channels[2].current_sample > 1000);

        // Step 2: high nibble 0xA decoded (negative delta)
        let sample_after_low = spu.channels[2].current_sample;
        spu.step(1, &main_ram, &shared_wram, &arm7_wram);
        assert!(spu.channels[2].current_sample < sample_after_low);
    }

    #[test]
    fn test_nds_spu_white_noise() {
        let mut spu = NdsSpu::new();
        let main_ram = vec![0u8; 0x400000];
        let shared_wram = [0u8; 0x8000];
        let arm7_wram = [0u8; 0x10000];

        // Channel 14: White Noise (format 3 on channel 14)
        spu.write_u32(0x0400_04E8, 0x0000_FFFF, &main_ram, &shared_wram, &arm7_wram);
        let cnt = (1 << 31) | (3 << 29) | (64 << 16) | 127;
        spu.write_u32(0x0400_04E0, cnt, &main_ram, &shared_wram, &arm7_wram);

        let mut values = Vec::new();
        for _ in 0..16 {
            spu.step(1, &main_ram, &shared_wram, &arm7_wram);
            values.push(spu.channels[14].current_sample);
        }

        // Noise produces both positive (+32767) and negative (-32767) samples
        assert!(values.contains(&32767));
        assert!(values.contains(&-32767));
    }

    #[test]
    fn test_nds_spu_panning_and_master_volume() {
        let mut spu = NdsSpu::new();
        let _main_ram = vec![0u8; 0x400000];
        let _shared_wram = [0u8; 0x8000];
        let _arm7_wram = [0u8; 0x10000];

        // Master sound disabled initially: mixing produces silence
        spu.channels[8].busy = true;
        spu.channels[8].current_sample = 32767;
        spu.channels[8].cnt = (1 << 31) | (3 << 29) | (0 << 16) | 127; // Pan 0: full left

        spu.mix_sample();
        assert_eq!(spu.sample_batch[0], 0.0);
        assert_eq!(spu.sample_batch[1], 0.0);
        spu.sample_batch.clear();

        // Enable master sound (bit 15) with volume 127
        spu.soundcnt = (1 << 15) | 127;
        spu.mix_sample();

        // Full left: Left > 0.9, Right == 0.0
        assert!(spu.sample_batch[0] > 0.9);
        assert_eq!(spu.sample_batch[1], 0.0);
        spu.sample_batch.clear();

        // Pan 127 (full right)
        spu.channels[8].cnt = (1 << 31) | (3 << 29) | (127 << 16) | 127;
        spu.mix_sample();
        assert_eq!(spu.sample_batch[0], 0.0);
        assert!(spu.sample_batch[1] > 0.9);
    }
}

