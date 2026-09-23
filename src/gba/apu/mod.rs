//! GBA Audio Processing Unit (APU)
//! Supports DirectSound Channels A & B (FIFO) and legacy DMG channels (PSG).

pub mod audio_output;
pub mod dmg;
pub mod ff_stretch;
pub mod resample;
pub mod ring;
pub mod spatial;

pub use audio_output::{AudioOutput, SurroundMode};
use spatial::soft_limit;
pub use dmg::DmgAudio;
use std::collections::VecDeque;

pub struct DirectSoundChannel {
    pub fifo: VecDeque<i8>,
    pub current_sample: i8,
    pub volume: f32,
    pub left_enable: bool,
    pub right_enable: bool,
    pub timer_select: usize,
    pub trigger_count: u64,
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
            trigger_count: 0,
        }
    }

    pub fn reset(&mut self) {
        self.fifo.clear();
        self.current_sample = 0;
        self.trigger_count += 1;
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

/// GBA system clock in Hz.
const GBA_CLOCK_HZ: u64 = 16_777_216;

pub struct Apu {
    pub sound_a: DirectSoundChannel,
    pub sound_b: DirectSoundChannel,
    pub dmg: DmgAudio,

    pub soundcnt_l: u16,
    pub soundcnt_h: u16,
    pub soundcnt_x: u16,
    pub soundbias: u16,

    pub audio_output: AudioOutput,
    /// Core sample clock: accumulates `cycles * CORE_SAMPLE_RATE`; one
    /// sample is due per `GBA_CLOCK_HZ` (exact integer arithmetic, so the
    /// sample stream is identical on every host; ROADMAP M2).
    sample_timer: u64,
    sample_batch: Vec<f32>,

    // DSP Filter States
    dc_x_l: f32,
    dc_y_l: f32,
    dc_x_r: f32,
    dc_y_r: f32,
    lp_l: f32,
    lp_r: f32,

    // Real-time Oscilloscope Waveform Ring Buffer
    pub scope_buffer: [f32; 512],
    pub scope_idx: usize,

    // Audio samples awaiting diagnostic analysis
    pub pending_diagnostic_samples: Vec<f32>,
    /// When `Some`, every core sample (interleaved stereo, CORE_SAMPLE_RATE)
    /// is also appended here. For replay hashing, recording and analysis
    /// tools; `None` (the default) costs nothing.
    pub capture: Option<Vec<f32>>,
    /// Suppress sample emission to output, capture, and scope buffers
    /// (used during run-ahead speculative frames).
    pub speculative: bool,
    /// Audio engine mode (HardwareOnly vs HdReSynthesis; ROADMAP M9)
    pub hd_audio_mode: crate::gba::m4a::AudioEngineMode,
    /// 48 kHz sample stream for HD audio re-synthesis
    pub hd_sample_stream: VecDeque<(f32, f32)>,
}

impl Default for Apu {
    fn default() -> Self {
        Self::new()
    }
}

impl Apu {
    pub fn new() -> Self {
        let audio_output = AudioOutput::new();

        Self {
            sound_a: DirectSoundChannel::new(),
            sound_b: DirectSoundChannel::new(),
            dmg: DmgAudio::new(),
            soundcnt_l: 0,
            soundcnt_h: 0,
            soundcnt_x: 0,
            soundbias: 0x0200,
            audio_output,
            sample_timer: 0,
            sample_batch: Vec::with_capacity(1024),
            dc_x_l: 0.0,
            dc_y_l: 0.0,
            dc_x_r: 0.0,
            dc_y_r: 0.0,
            lp_l: 0.0,
            lp_r: 0.0,
            scope_buffer: [0.0; 512],
            scope_idx: 0,
            pending_diagnostic_samples: Vec::with_capacity(2048),
            capture: None,
            speculative: false,
            hd_audio_mode: crate::gba::m4a::AudioEngineMode::HdReSynthesis,
            hd_sample_stream: VecDeque::with_capacity(2048),
        }
    }

    pub fn read_reg16(&self, addr: u32) -> u16 {
        match addr & 0x3FE {
            0x060 => self.dmg.ch1.cnt_l & 0x007F,
            0x062 => self.dmg.ch1.cnt_h & 0xFFC0,
            0x064 => if self.dmg.ch1.length_enabled { 0x4000 } else { 0 },
            0x068 => self.dmg.ch2.cnt_l & 0xFFC0,
            0x06C => if self.dmg.ch2.length_enabled { 0x4000 } else { 0 },
            0x070 => self.dmg.ch3.cnt_l & 0x00E0,
            0x072 => self.dmg.ch3.cnt_h & 0xE000,
            0x074 => if self.dmg.ch3.length_enabled { 0x4000 } else { 0 },
            0x078 => self.dmg.ch4.cnt_l & 0xFF00,
            0x07C => (self.dmg.ch4.cnt_h & 0x00FF) | if self.dmg.ch4.length_enabled { 0x4000 } else { 0 },
            0x080 => self.soundcnt_l & 0xFF77,
            0x082 => self.soundcnt_h & 0x770F,
            0x084 => (self.soundcnt_x & 0x0080) | self.dmg.soundcnt_x_bits(),
            0x088 => self.soundbias & 0xC3FE,
            0x090..=0x09E => {
                let off = (addr & 0x0E) as usize;
                let lo = self.dmg.ch3.read_wave_ram(off) as u16;
                let hi = self.dmg.ch3.read_wave_ram(off + 1) as u16;
                lo | (hi << 8)
            }
            _ => 0,
        }
    }

    pub fn read_reg8(&self, addr: u32) -> u8 {
        let off = addr & 0x3FF;
        match off {
            0x090..=0x09F => self.dmg.ch3.read_wave_ram((off & 0x0F) as usize),
            _ => {
                let val16 = self.read_reg16(off & !1);
                if (off & 1) != 0 {
                    (val16 >> 8) as u8
                } else {
                    (val16 & 0xFF) as u8
                }
            }
        }
    }

    pub fn write_reg8(&mut self, addr: u32, val: u8) {
        let off = addr & 0x3FF;
        match off {
            0x060 => self.dmg.ch1.write_cnt_l_byte(0, val),
            0x061 => self.dmg.ch1.write_cnt_l_byte(1, val),
            0x062 => self.dmg.ch1.write_cnt_h_byte(0, val),
            0x063 => self.dmg.ch1.write_cnt_h_byte(1, val),
            0x064 => self.dmg.ch1.write_cnt_x_byte(0, val),
            0x065 => self.dmg.ch1.write_cnt_x_byte(1, val),
            0x068 => self.dmg.ch2.write_cnt_l_byte(0, val),
            0x069 => self.dmg.ch2.write_cnt_l_byte(1, val),
            0x06C => self.dmg.ch2.write_cnt_h_byte(0, val),
            0x06D => self.dmg.ch2.write_cnt_h_byte(1, val),
            0x070 => self.dmg.ch3.write_cnt_l_byte(0, val),
            0x071 => self.dmg.ch3.write_cnt_l_byte(1, val),
            0x072 => self.dmg.ch3.write_cnt_h_byte(0, val),
            0x073 => self.dmg.ch3.write_cnt_h_byte(1, val),
            0x074 => self.dmg.ch3.write_cnt_x_byte(0, val),
            0x075 => self.dmg.ch3.write_cnt_x_byte(1, val),
            0x078 => self.dmg.ch4.write_cnt_l_byte(0, val),
            0x079 => self.dmg.ch4.write_cnt_l_byte(1, val),
            0x07C => self.dmg.ch4.write_cnt_h_byte(0, val),
            0x07D => self.dmg.ch4.write_cnt_h_byte(1, val),
            0x080 => self.soundcnt_l = (self.soundcnt_l & 0xFF00) | (val as u16),
            0x081 => self.soundcnt_l = (self.soundcnt_l & 0x00FF) | ((val as u16) << 8),
            0x082 => {
                let new_val = (self.soundcnt_h & 0xFF00) | (val as u16);
                self.write_soundcnt_h(new_val);
            }
            0x083 => {
                let new_val = (self.soundcnt_h & 0x00FF) | ((val as u16) << 8);
                self.write_soundcnt_h(new_val);
            }
            0x084 => {
                self.write_reg16(0x084, val as u16);
            }
            0x088 => self.soundbias = (self.soundbias & 0xFF00) | (val as u16),
            0x089 => self.soundbias = (self.soundbias & 0x00FF) | ((val as u16) << 8),
            0x090..=0x09F => {
                self.dmg.ch3.write_wave_ram((off & 0x0F) as usize, val);
            }
            0x0A0..=0x0A3 => {
                self.sound_a.push_byte(val);
            }
            0x0A4..=0x0A7 => {
                self.sound_b.push_byte(val);
            }
            _ => {}
        }
    }

    pub fn write_reg16(&mut self, addr: u32, val: u16) {
        match addr & 0x3FE {
            0x060 => self.dmg.ch1.write_cnt_l(val),
            0x062 => self.dmg.ch1.write_cnt_h(val),
            0x064 => self.dmg.ch1.write_cnt_x(val),
            0x068 => self.dmg.ch2.write_cnt_l(val),
            0x06C => self.dmg.ch2.write_cnt_h(val),
            0x070 => self.dmg.ch3.write_cnt_l(val),
            0x072 => self.dmg.ch3.write_cnt_h(val),
            0x074 => self.dmg.ch3.write_cnt_x(val),
            0x078 => self.dmg.ch4.write_cnt_l(val),
            0x07C => self.dmg.ch4.write_cnt_h(val),
            0x080 => self.soundcnt_l = val,
            0x082 => self.write_soundcnt_h(val),
            0x084 => {
                let master_enable = (val & 0x0080) != 0;
                self.soundcnt_x = (self.soundcnt_x & 0x007F) | (val & 0x0080);
                if !master_enable {
                    // Real hardware zeroes NR10-NR51 (duty/envelope/sweep/
                    // frequency) on power-off, not just the active flags, so
                    // a later power-on doesn't resume with stale settings.
                    // Wave RAM (not an NRx register) and the diagnostic
                    // trigger counters are left untouched.
                    self.dmg.ch1.power_off_reset();
                    self.dmg.ch2.power_off_reset();
                    self.dmg.ch3.power_off_reset();
                    self.dmg.ch4.power_off_reset();
                }
            }
            0x088 => self.soundbias = val,
            0x090..=0x09E => {
                let off = (addr & 0x0E) as usize;
                self.dmg.ch3.write_wave_ram(off, (val & 0xFF) as u8);
                self.dmg.ch3.write_wave_ram(off + 1, (val >> 8) as u8);
            }
            0x0A0 | 0x0A2 => {
                self.sound_a.push_byte((val & 0xFF) as u8);
                self.sound_a.push_byte((val >> 8) as u8);
            }
            0x0A4 | 0x0A6 => {
                self.sound_b.push_byte((val & 0xFF) as u8);
                self.sound_b.push_byte((val >> 8) as u8);
            }
            _ => {}
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

        // Step DMG channels and frame sequencer
        self.dmg.step(cycles);

        // Check Timer overflows feeding DirectSound A & B
        if timer_overflows[self.sound_a.timer_select]
            && self.sound_a.pop_sample() {
                dma_req_a = true;
            }

        if timer_overflows[self.sound_b.timer_select]
            && self.sound_b.pop_sample() {
                dma_req_b = true;
            }

        // Fixed-rate core sample clock (CORE_SAMPLE_RATE); the output layer
        // resamples to the device.
        self.sample_timer += cycles as u64 * resample::CORE_SAMPLE_RATE as u64;
        while self.sample_timer >= GBA_CLOCK_HZ {
            self.sample_timer -= GBA_CLOCK_HZ;
            self.mix_and_push_sample();
        }

        (dma_req_a, dma_req_b)
    }

    fn mix_and_push_sample(&mut self) {
        let enabled = (self.soundcnt_x & (1 << 7)) != 0;
        if !enabled {
            if !self.speculative {
                self.sample_batch.push(0.0);
                self.sample_batch.push(0.0);
                if self.sample_batch.len() >= 512 {
                    self.flush_samples();
                }
            }
            return;
        }

        // (Fast-forward muting happens in AudioOutput, so the core's own
        // sample stream never depends on host state.)

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

        let mut ds_left = 0.0f32;
        let mut ds_right = 0.0f32;

        if self.sound_a.left_enable {
            ds_left += sa_norm;
        }
        if self.sound_a.right_enable {
            ds_right += sa_norm;
        }

        if self.sound_b.left_enable {
            ds_left += sb_norm;
        }
        if self.sound_b.right_enable {
            ds_right += sb_norm;
        }

        // DMG PSG channels (1-4)
        let (s1, s2, s3, s4) = self.dmg.get_samples();

        let vol_right = ((self.soundcnt_l & 7) as f32 + 1.0) / 8.0;
        let vol_left = (((self.soundcnt_l >> 4) & 7) as f32 + 1.0) / 8.0;

        let dmg_ratio = match self.soundcnt_h & 3 {
            0 => 0.25,
            1 => 0.5,
            2 => 1.0,
            _ => 1.0,
        };

        let mut dmg_left = 0.0f32;
        let mut dmg_right = 0.0f32;

        // Ch 1 (idx 2): Square + Sweep
        if !self.audio_output.is_channel_muted(2) {
            if (self.soundcnt_l & (1 << 12)) != 0 { dmg_left += s1; }
            if (self.soundcnt_l & (1 << 8)) != 0 { dmg_right += s1; }
        }
        // Ch 2 (idx 3): Square
        if !self.audio_output.is_channel_muted(3) {
            if (self.soundcnt_l & (1 << 13)) != 0 { dmg_left += s2; }
            if (self.soundcnt_l & (1 << 9)) != 0 { dmg_right += s2; }
        }
        // Ch 3 (idx 4): Wave
        if !self.audio_output.is_channel_muted(4) {
            if (self.soundcnt_l & (1 << 14)) != 0 { dmg_left += s3; }
            if (self.soundcnt_l & (1 << 10)) != 0 { dmg_right += s3; }
        }
        // Ch 4 (idx 5): Noise
        if !self.audio_output.is_channel_muted(5) {
            if (self.soundcnt_l & (1 << 15)) != 0 { dmg_left += s4; }
            if (self.soundcnt_l & (1 << 11)) != 0 { dmg_right += s4; }
        }

        // Calibrated DMG gain: balanced headroom that blends naturally with DirectSound without overpowering BGM
        dmg_left *= vol_left * dmg_ratio * 0.10;
        dmg_right *= vol_right * dmg_ratio * 0.10;
        let mut raw_left = ds_left * 0.50 + dmg_left;
        let mut raw_right = ds_right * 0.50 + dmg_right;

        // If HD audio re-synthesis is enabled and has stream samples available,
        // substitute DirectSound PCM with high-resolution re-synthesized audio,
        // blending DMG PSG channels for sound effects (ROADMAP M9).
        if self.hd_audio_mode == crate::gba::m4a::AudioEngineMode::HdReSynthesis {
            if let Some((hd_l, hd_r)) = self.hd_sample_stream.pop_front() {
                raw_left = hd_l + dmg_left;
                raw_right = hd_r + dmg_right;
            }
        }

        // 1. DC Blocking Filter (AC coupling capacitor modeling, ~35Hz cutoff at 44.1kHz)
        // y[n] = x[n] - x[n-1] + R * y[n-1], with R = 0.995
        let dc_blocked_l = raw_left - self.dc_x_l + 0.995 * self.dc_y_l;
        self.dc_x_l = raw_left;
        self.dc_y_l = dc_blocked_l;

        let dc_blocked_r = raw_right - self.dc_x_r + 0.995 * self.dc_y_r;
        self.dc_x_r = raw_right;
        self.dc_y_r = dc_blocked_r;

        // 2. Analog Band-Limiting Low-Pass Filter (emulates GBA hardware analog RC filtering ~12kHz)
        // Smooths harsh high-frequency square wave edges without losing crispness
        const LP_ALPHA: f32 = 0.80;
        self.lp_l += LP_ALPHA * (dc_blocked_l - self.lp_l);
        self.lp_r += LP_ALPHA * (dc_blocked_r - self.lp_r);

        // 3. Smooth Soft Limiter (prevents digital clipping rail buzz while preserving musical dynamics)
        let final_left = soft_limit(self.lp_l);
        let final_right = soft_limit(self.lp_r);

        if !self.speculative {
            self.sample_batch.push(final_left);
            self.sample_batch.push(final_right);

            // Update real-time oscilloscope buffer
            self.scope_buffer[self.scope_idx] = (final_left + final_right) * 0.5;
            self.scope_idx = (self.scope_idx + 1) % 512;

            if self.sample_batch.len() >= 512 {
                self.flush_samples();
            }
        }
    }

    /// Drop samples produced by a frame that is being thrown away (run-ahead
    /// speculation): they reach neither the device nor any capture.
    pub fn discard_samples(&mut self) {
        self.sample_batch.clear();
    }

    /// Flushes accumulated audio samples to host audio stream and applies Dynamic Rate Control (DRC)
    pub fn flush_samples(&mut self) {
        if self.sample_batch.is_empty() {
            return;
        }

        if self.speculative {
            self.sample_batch.clear();
            return;
        }

        self.pending_diagnostic_samples.extend_from_slice(&self.sample_batch);
        if let Some(cap) = self.capture.as_mut() {
            cap.extend_from_slice(&self.sample_batch);
        }
        self.audio_output.push_sample_batch(&self.sample_batch);
        self.sample_batch.clear();

        // Rate control against the host device lives in AudioOutput's
        // resampler (ROADMAP M2).
    }

    pub fn set_hd_audio_mode(&mut self, mode: crate::gba::m4a::AudioEngineMode) {
        self.hd_audio_mode = mode;
        if mode == crate::gba::m4a::AudioEngineMode::HardwareOnly {
            self.hd_sample_stream.clear();
        }
    }

    pub fn hd_audio_mode(&self) -> crate::gba::m4a::AudioEngineMode {
        self.hd_audio_mode
    }
}

// ---- Save states (ROADMAP M2) --------------------------------------------
use crate::gba::state::{Snapshot, StateReader, StateWriter};

impl Snapshot for DirectSoundChannel {
    fn save(&self, w: &mut StateWriter) {
        let fifo: Vec<u8> = self.fifo.iter().map(|&s| s as u8).collect();
        w.bytes(&fifo);
        w.u8(self.current_sample as u8); w.f32(self.volume);
        w.bool(self.left_enable); w.bool(self.right_enable); w.u8(self.timer_select as u8);
    }
    fn load(&mut self, r: &mut StateReader) -> Option<()> {
        let fifo = r.bytes()?;
        if fifo.len() > 64 { return None; }
        self.fifo.clear();
        self.fifo.extend(fifo.iter().map(|&b| b as i8));
        self.current_sample = r.u8()? as i8; self.volume = r.f32()?;
        self.left_enable = r.bool()?; self.right_enable = r.bool()?;
        self.timer_select = (r.u8()? & 1) as usize;
        Some(())
    }
}

impl Snapshot for Apu {
    fn save(&self, w: &mut StateWriter) {
        self.sound_a.save(w); self.sound_b.save(w); self.dmg.save(w);
        for v in [self.soundcnt_l, self.soundcnt_h, self.soundcnt_x, self.soundbias] { w.u16(v); }
        w.u64(self.sample_timer);
        for v in [self.dc_x_l, self.dc_y_l, self.dc_x_r, self.dc_y_r, self.lp_l, self.lp_r] { w.f32(v); }
    }
    fn load(&mut self, r: &mut StateReader) -> Option<()> {
        self.sound_a.load(r)?; self.sound_b.load(r)?; self.dmg.load(r)?;
        self.soundcnt_l = r.u16()?; self.soundcnt_h = r.u16()?; self.soundcnt_x = r.u16()?; self.soundbias = r.u16()?;
        self.sample_timer = r.u64()?;
        self.dc_x_l = r.f32()?; self.dc_y_l = r.f32()?; self.dc_x_r = r.f32()?; self.dc_y_r = r.f32()?;
        self.lp_l = r.f32()?; self.lp_r = r.f32()?;
        // Samples produced before the load belong to the old timeline.
        self.sample_batch.clear();
        self.hd_sample_stream.clear();
        Some(())
    }
}
