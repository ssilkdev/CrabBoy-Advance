//! GBA Audio Processing Unit (APU)
//! Supports DirectSound Channels A & B (FIFO) and legacy DMG channels.

pub mod audio_output;

pub use audio_output::{AudioOutput, SurroundMode};
use std::collections::VecDeque;

pub struct DirectSoundChannel {
    pub fifo: VecDeque<i8>,
    pub current_sample: i8,
    pub volume: f32,
    pub left_enable: bool,
    pub right_enable: bool,
    pub timer_select: usize,
}

impl Default for DirectSoundChannel {
    fn default() -> Self {
        Self::new()
    }
}

impl DirectSoundChannel {
    pub fn new() -> Self {
        Self {
            fifo: VecDeque::with_capacity(32),
            current_sample: 0,
            volume: 1.0,
            left_enable: false,
            right_enable: false,
            timer_select: 0,
        }
    }

    pub fn reset(&mut self) {
        self.fifo.clear();
        self.current_sample = 0;
    }

    pub fn push_byte(&mut self, val: u8) {
        if self.fifo.len() < 32 {
            self.fifo.push_back(val as i8);
        }
    }

    pub fn pop_sample(&mut self) -> bool {
        if let Some(s) = self.fifo.pop_front() {
            self.current_sample = s;
        }
        self.fifo.len() <= 16
    }
}

pub struct Apu {
    pub sound_a: DirectSoundChannel,
    pub sound_b: DirectSoundChannel,

    pub soundcnt_l: u16,
    pub soundcnt_h: u16,
    pub soundcnt_x: u16,
    pub soundbias: u16,

    pub audio_output: AudioOutput,
    sample_timer: f64,
    base_cycles_per_sample: f64,
    cpu_cycles_per_sample: f64,
    sample_batch: Vec<f32>,
}

impl Default for Apu {
    fn default() -> Self {
        Self::new()
    }
}

impl Apu {
    pub fn new() -> Self {
        let audio_output = AudioOutput::new();
        let sample_rate = audio_output.sample_rate() as f64;
        let gba_cpu_freq = 16_777_216.0; // 16.78 MHz
        let cpu_cycles_per_sample = gba_cpu_freq / sample_rate;

        Self {
            sound_a: DirectSoundChannel::new(),
            sound_b: DirectSoundChannel::new(),
            soundcnt_l: 0,
            soundcnt_h: 0,
            soundcnt_x: 0,
            soundbias: 0x0200,
            audio_output,
            sample_timer: 0.0,
            base_cycles_per_sample: cpu_cycles_per_sample,
            cpu_cycles_per_sample,
            sample_batch: Vec::with_capacity(1024),
        }
    }

    pub fn write_soundcnt_h(&mut self, val: u16) {
        self.soundcnt_h = val;

        self.sound_a.volume = if (val & (1 << 2)) != 0 { 1.0 } else { 0.5 };
        self.sound_b.volume = if (val & (1 << 3)) != 0 { 1.0 } else { 0.5 };

        self.sound_a.right_enable = (val & (1 << 8)) != 0;
        self.sound_a.left_enable = (val & (1 << 9)) != 0;
        self.sound_a.timer_select = ((val >> 10) & 1) as usize;

        if (val & (1 << 11)) != 0 {
            self.sound_a.reset();
        }

        self.sound_b.right_enable = (val & (1 << 12)) != 0;
        self.sound_b.left_enable = (val & (1 << 13)) != 0;
        self.sound_b.timer_select = ((val >> 14) & 1) as usize;

        if (val & (1 << 15)) != 0 {
            self.sound_b.reset();
        }
    }

    /// Step APU by elapsed CPU cycles, timer overflows, and stream audio to host
    pub fn step(&mut self, cycles: u32, timer_overflows: [bool; 4]) -> (bool, bool) {
        let mut dma_req_a = false;
        let mut dma_req_b = false;

        // Check Timer overflows feeding DirectSound A & B
        if timer_overflows[self.sound_a.timer_select]
            && self.sound_a.pop_sample() {
                dma_req_a = true;
            }

        if timer_overflows[self.sound_b.timer_select]
            && self.sound_b.pop_sample() {
                dma_req_b = true;
            }

        // Host audio resampling
        self.sample_timer += cycles as f64;
        while self.sample_timer >= self.cpu_cycles_per_sample {
            self.sample_timer -= self.cpu_cycles_per_sample;
            self.mix_and_push_sample();
        }

        (dma_req_a, dma_req_b)
    }

    fn mix_and_push_sample(&mut self) {
        let enabled = (self.soundcnt_x & (1 << 7)) != 0;
        if !enabled {
            self.sample_batch.push(0.0);
            self.sample_batch.push(0.0);
            if self.sample_batch.len() >= 512 {
                self.flush_samples();
            }
            return;
        }

        // Fast-forward smart mute
        if self.audio_output.is_fast_forwarding() && self.audio_output.fast_forward_mode() == 1 {
            self.sample_batch.push(0.0);
            self.sample_batch.push(0.0);
            if self.sample_batch.len() >= 512 {
                self.flush_samples();
            }
            return;
        }

        // DirectSound samples (-128..127 normalized to -1.0..1.0)
        let ch_a_unmuted = !self.audio_output.is_channel_muted(0);
        let ch_b_unmuted = !self.audio_output.is_channel_muted(1);

        let sa_norm = if ch_a_unmuted {
            (self.sound_a.current_sample as f32 / 128.0) * self.sound_a.volume
        } else {
            0.0
        };
        let sb_norm = if ch_b_unmuted {
            (self.sound_b.current_sample as f32 / 128.0) * self.sound_b.volume
        } else {
            0.0
        };

        let mut left = 0.0f32;
        let mut right = 0.0f32;

        if self.sound_a.left_enable {
            left += sa_norm;
        }
        if self.sound_a.right_enable {
            right += sa_norm;
        }

        if self.sound_b.left_enable {
            left += sb_norm;
        }
        if self.sound_b.right_enable {
            right += sb_norm;
        }

        self.sample_batch.push(left * 0.5);
        self.sample_batch.push(right * 0.5);

        if self.sample_batch.len() >= 512 {
            self.flush_samples();
        }
    }

    /// Flushes accumulated audio samples to host audio stream and applies Dynamic Rate Control (DRC)
    pub fn flush_samples(&mut self) {
        if self.sample_batch.is_empty() {
            return;
        }

        self.audio_output.push_sample_batch(&self.sample_batch);
        self.sample_batch.clear();

        // Dynamic Rate Control (DRC) smoothly regulates buffer fill level without clicks
        let q_len = self.audio_output.buffer_len();
        if q_len > 3000 {
            // Buffer is getting full: produce samples slightly slower (+0.5% cycles per sample)
            self.cpu_cycles_per_sample = self.base_cycles_per_sample * 1.005;
        } else if q_len < 1000 {
            // Buffer is getting low: produce samples slightly faster (-0.5% cycles per sample)
            self.cpu_cycles_per_sample = self.base_cycles_per_sample * 0.995;
        } else {
            self.cpu_cycles_per_sample = self.base_cycles_per_sample;
        }
    }
}
