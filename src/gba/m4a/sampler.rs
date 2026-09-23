//! High-Quality 48 kHz Stereo Sampler Engine
//!
//! Features:
//! - Full 32-bit floating point mixing bus with zero 8-bit quantization noise
//! - Linear, Cubic Hermite spline, and Band-limited Sinc interpolation
//! - High-precision 48 kHz ADSR envelope generator
//! - CGB PSG channel synthesis (square, wave, noise)
//! - Stereo panning laws (equal-power)
//! - High-fidelity stereo reverberation unit
//! - Smooth soft limiting

use std::sync::Arc;
use super::voice::{ToneData, WaveData};

/// Interpolation mode for PCM sample resampling
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum M4aInterpolation {
    /// Standard 2-point linear interpolation
    Linear,
    /// 4-point Catmull-Rom cubic Hermite spline (smooth C1 continuity)
    #[default]
    CubicHermite,
    /// 8-point band-limited windowed Sinc interpolation (highest fidelity)
    Sinc,
}

impl M4aInterpolation {
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Linear => "Linear",
            Self::CubicHermite => "Cubic Hermite Spline (Recommended)",
            Self::Sinc => "Band-Limited Sinc (8-Point)",
        }
    }
}

/// ADSR envelope generator state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdsrState {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

/// Active voice playing on the 48 kHz sampler
#[derive(Debug, Clone)]
pub struct Voice {
    pub is_active: bool,
    pub channel_id: usize,
    pub instrument_id: u8,
    pub note_key: u8,
    pub wave: Option<Arc<WaveData>>,
    pub pos: f64,
    pub step: f64,
    pub env_state: AdsrState,
    pub env_level: f32,
    pub attack_step: f32,
    pub decay_rate: f32,
    pub sustain_level: f32,
    pub release_rate: f32,
    pub vol_left: f32,
    pub vol_right: f32,
    pub cgb_type: Option<u8>,
    pub cgb_phase: f32,
    pub cgb_phase_inc: f32,
    pub age_samples: u64,
}

impl Voice {
    pub fn new() -> Self {
        Self {
            is_active: false,
            channel_id: 0,
            instrument_id: 0,
            note_key: 0,
            wave: None,
            pos: 0.0,
            step: 1.0,
            env_state: AdsrState::Idle,
            env_level: 0.0,
            attack_step: 0.01,
            decay_rate: 0.999,
            sustain_level: 0.7,
            release_rate: 0.998,
            vol_left: 0.5,
            vol_right: 0.5,
            cgb_type: None,
            cgb_phase: 0.0,
            cgb_phase_inc: 0.0,
            age_samples: 0,
        }
    }

    /// Advance ADSR state machine by 1 sample (at 48 kHz)
    pub fn step_adsr(&mut self) {
        match self.env_state {
            AdsrState::Idle => {
                self.env_level = 0.0;
                self.is_active = false;
            }
            AdsrState::Attack => {
                self.env_level += self.attack_step;
                if self.env_level >= 1.0 {
                    self.env_level = 1.0;
                    self.env_state = AdsrState::Decay;
                }
            }
            AdsrState::Decay => {
                self.env_level = (self.env_level - self.sustain_level) * self.decay_rate + self.sustain_level;
                if (self.env_level - self.sustain_level).abs() < 0.001 {
                    self.env_level = self.sustain_level;
                    self.env_state = AdsrState::Sustain;
                }
            }
            AdsrState::Sustain => {
                self.env_level = self.sustain_level;
            }
            AdsrState::Release => {
                self.env_level *= self.release_rate;
                if self.env_level < 0.0005 {
                    self.env_level = 0.0;
                    self.env_state = AdsrState::Idle;
                    self.is_active = false;
                }
            }
        }
    }

    /// Render 1 sample for this voice
    pub fn render_sample(&mut self, interp: M4aInterpolation) -> (f32, f32) {
        if !self.is_active || self.env_state == AdsrState::Idle {
            return (0.0, 0.0);
        }

        self.step_adsr();
        if self.env_level <= 0.0 {
            return (0.0, 0.0);
        }

        self.age_samples += 1;
        let raw_sample = if let Some(ref wave) = self.wave {
            // PCM playback
            let s = sample_pcm(wave, self.pos, interp);
            self.pos += self.step;

            if wave.is_looped() {
                let loop_start = wave.loop_start as f64;
                let size = wave.size as f64;
                if self.pos >= size {
                    let loop_len = (size - loop_start).max(1.0);
                    while self.pos >= size {
                        self.pos -= loop_len;
                    }
                }
            } else if self.pos >= wave.size as f64 {
                self.is_active = false;
                return (0.0, 0.0);
            }
            s
        } else if let Some(cgb) = self.cgb_type {
            // CGB PSG synthesis
            let s = sample_cgb(cgb, self.cgb_phase);
            self.cgb_phase += self.cgb_phase_inc;
            if self.cgb_phase >= 1.0 {
                self.cgb_phase -= 1.0;
            }
            s
        } else {
            0.0
        };

        let amp = raw_sample * self.env_level;
        (amp * self.vol_left, amp * self.vol_right)
    }

    /// Trigger note off
    pub fn release(&mut self) {
        if self.env_state != AdsrState::Idle {
            self.env_state = AdsrState::Release;
        }
    }
}

/// High-Quality 48 kHz stereo sampler
#[derive(Debug, Clone)]
pub struct HdM4aSampler {
    pub voices: Vec<Voice>,
    pub sample_rate: u32,
    pub interpolation: M4aInterpolation,
    pub reverb_enabled: bool,
    pub reverb_level: f32,
    pub channel_mute_mask: u16,
    pub master_volume: f32,
    // Simple stereo comb/all-pass reverb state
    reverb_comb_l: [Vec<f32>; 4],
    reverb_comb_r: [Vec<f32>; 4],
    reverb_comb_idx_l: [usize; 4],
    reverb_comb_idx_r: [usize; 4],
}

impl Default for HdM4aSampler {
    fn default() -> Self {
        Self::new(32, 48_000)
    }
}

impl HdM4aSampler {
    pub fn new(max_polyphony: usize, sample_rate: u32) -> Self {
        let poly = max_polyphony.clamp(1, 128);
        let mut voices = Vec::with_capacity(poly);
        for _ in 0..poly {
            voices.push(Voice::new());
        }

        // Stereo reverb comb filter delay line buffers (~25ms to ~45ms)
        let comb_lens = [1116, 1188, 1277, 1356];
        let mut reverb_comb_l = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
        let mut reverb_comb_r = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
        for i in 0..4 {
            reverb_comb_l[i] = vec![0.0; comb_lens[i]];
            reverb_comb_r[i] = vec![0.0; comb_lens[i] + 23]; // stereo decorrelation
        }

        Self {
            voices,
            sample_rate,
            interpolation: M4aInterpolation::CubicHermite,
            reverb_enabled: true,
            reverb_level: 0.25,
            channel_mute_mask: 0,
            master_volume: 1.0,
            reverb_comb_l,
            reverb_comb_r,
            reverb_comb_idx_l: [0; 4],
            reverb_comb_idx_r: [0; 4],
        }
    }

    /// Trigger a note on a channel
    pub fn note_on(
        &mut self,
        channel: usize,
        note: u8,
        velocity: u8,
        tone: &ToneData,
        wave: Option<Arc<WaveData>>,
        pan: u8,
        volume: u8,
        pitch_bend: i8,
        tune: i8,
    ) {
        // If channel is muted via solo/mute mask, ignore
        if (self.channel_mute_mask & (1 << (channel % 16))) != 0 {
            return;
        }

        // Allocate a voice: find idle voice or steal oldest
        let voice_idx = self.allocate_voice();
        let v = &mut self.voices[voice_idx];

        v.is_active = true;
        v.channel_id = channel;
        v.instrument_id = tone.wav_ptr as u8;
        v.note_key = note;
        v.wave = wave.clone();
        v.pos = 0.0;
        v.age_samples = 0;

        // Calculate pitch step
        let vel_norm = (velocity as f32 / 127.0).clamp(0.0, 1.0);
        let vol_norm = (volume as f32 / 127.0).clamp(0.0, 1.0) * vel_norm;

        // Equal-power stereo panning (pan 0..127, center 64)
        let pan_norm = (pan as f32 / 127.0).clamp(0.0, 1.0);
        let angle = pan_norm * std::f32::consts::FRAC_PI_2;
        v.vol_left = vol_norm * angle.cos();
        v.vol_right = vol_norm * angle.sin();

        // Calculate ADSR parameters scaled to 48 kHz
        // GBA ADSR: 255 is fastest rate
        let att_rate = tone.attack.max(1) as f32 / 255.0;
        v.attack_step = (att_rate * att_rate * 0.05).max(0.0001);

        let dec_rate = tone.decay as f32 / 255.0;
        v.decay_rate = 1.0 - (1.0 - dec_rate) * 0.001;

        v.sustain_level = tone.sustain as f32 / 255.0;

        let rel_rate = tone.release.max(1) as f32 / 255.0;
        v.release_rate = 1.0 - (rel_rate * rel_rate * 0.003).clamp(0.0001, 0.05);

        v.env_state = AdsrState::Attack;
        v.env_level = 0.0;

        // Calculate playback frequency
        let total_semitones = (note as f64) - (tone.key as f64)
            + ((pitch_bend as f64) / 32.0)
            + ((tune as f64) / 64.0);

        if let Some(ref w) = wave {
            v.cgb_type = None;
            let base_rate = w.base_sample_rate();
            let target_freq = base_rate * 2.0_f64.powf(total_semitones / 12.0);
            v.step = (target_freq / (self.sample_rate as f64)).max(0.0001);
        } else if (1..=4).contains(&tone.tone_type) {
            // CGB PSG channel
            v.cgb_type = Some(tone.tone_type);
            let freq_hz = 440.0 * 2.0_f64.powf(((note as f64) - 69.0 + total_semitones) / 12.0);
            v.cgb_phase = 0.0;
            v.cgb_phase_inc = (freq_hz / (self.sample_rate as f64)) as f32;
        } else {
            v.is_active = false;
        }
    }

    /// Trigger note off for a channel
    pub fn note_off(&mut self, channel: usize, key: u8) {
        for v in &mut self.voices {
            if v.is_active && v.channel_id == channel && v.note_key == key {
                v.release();
            }
        }
    }

    /// Stop all active voices immediately
    pub fn stop_all(&mut self) {
        for v in &mut self.voices {
            v.is_active = false;
            v.env_state = AdsrState::Idle;
            v.env_level = 0.0;
        }
    }

    /// Set volume for a channel
    pub fn set_volume(&mut self, channel: usize, volume: u8) {
        let vol_norm = (volume as f32 / 127.0).clamp(0.0, 1.0);
        for v in &mut self.voices {
            if v.is_active && v.channel_id == channel {
                let pan = v.vol_right / (v.vol_left + v.vol_right + 0.0001);
                let angle = pan * std::f32::consts::FRAC_PI_2;
                v.vol_left = vol_norm * angle.cos();
                v.vol_right = vol_norm * angle.sin();
            }
        }
    }

    /// Set panning for a channel
    pub fn set_pan(&mut self, channel: usize, pan: u8) {
        let pan_norm = (pan as f32 / 127.0).clamp(0.0, 1.0);
        let angle = pan_norm * std::f32::consts::FRAC_PI_2;
        for v in &mut self.voices {
            if v.is_active && v.channel_id == channel {
                let total_vol = (v.vol_left * v.vol_left + v.vol_right * v.vol_right).sqrt();
                v.vol_left = total_vol * angle.cos();
                v.vol_right = total_vol * angle.sin();
            }
        }
    }

    /// Render 1 stereo sample pair at 48 kHz
    pub fn render_sample(&mut self) -> (f32, f32) {
        let mut mix_l = 0.0f32;
        let mut mix_r = 0.0f32;

        let interp = self.interpolation;
        for v in &mut self.voices {
            if v.is_active {
                let (sl, sr) = v.render_sample(interp);
                mix_l += sl;
                mix_r += sr;
            }
        }

        // Apply Stereo Reverb if enabled
        if self.reverb_enabled && self.reverb_level > 0.001 {
            let (rev_l, rev_r) = self.process_reverb(mix_l, mix_r);
            mix_l = mix_l * (1.0 - self.reverb_level * 0.5) + rev_l * self.reverb_level;
            mix_r = mix_r * (1.0 - self.reverb_level * 0.5) + rev_r * self.reverb_level;
        }

        // Apply Master Volume and Soft Limiter
        mix_l *= self.master_volume;
        mix_r *= self.master_volume;

        (soft_limit(mix_l), soft_limit(mix_r))
    }

    fn allocate_voice(&mut self) -> usize {
        // 1. Find idle voice
        for (i, v) in self.voices.iter().enumerate() {
            if !v.is_active {
                return i;
            }
        }
        // 2. Find released voice with lowest envelope
        let mut lowest_idx = 0;
        let mut lowest_level = 999.0f32;
        for (i, v) in self.voices.iter().enumerate() {
            if v.env_state == AdsrState::Release && v.env_level < lowest_level {
                lowest_level = v.env_level;
                lowest_idx = i;
            }
        }
        if lowest_level < 999.0 {
            return lowest_idx;
        }
        // 3. Fallback: steal oldest voice
        let mut oldest_idx = 0;
        let mut oldest_age = 0;
        for (i, v) in self.voices.iter().enumerate() {
            if v.age_samples > oldest_age {
                oldest_age = v.age_samples;
                oldest_idx = i;
            }
        }
        oldest_idx
    }

    fn process_reverb(&mut self, in_l: f32, in_r: f32) -> (f32, f32) {
        let mut out_l = 0.0f32;
        let mut out_r = 0.0f32;
        const FEEDBACK: f32 = 0.72;

        for i in 0..4 {
            let buf_l = &mut self.reverb_comb_l[i];
            let idx_l = self.reverb_comb_idx_l[i];
            let delayed_l = buf_l[idx_l];
            buf_l[idx_l] = in_l + delayed_l * FEEDBACK;
            out_l += delayed_l;
            self.reverb_comb_idx_l[i] = (idx_l + 1) % buf_l.len();

            let buf_r = &mut self.reverb_comb_r[i];
            let idx_r = self.reverb_comb_idx_r[i];
            let delayed_r = buf_r[idx_r];
            buf_r[idx_r] = in_r + delayed_r * FEEDBACK;
            out_r += delayed_r;
            self.reverb_comb_idx_r[i] = (idx_r + 1) % buf_r.len();
        }

        (out_l * 0.25, out_r * 0.25)
    }
}

/// Resample PCM audio using selected interpolation
fn sample_pcm(wave: &WaveData, pos: f64, interp: M4aInterpolation) -> f32 {
    let samples = &wave.samples;
    let len = samples.len();
    if len == 0 {
        return 0.0;
    }

    match interp {
        M4aInterpolation::Linear => {
            let idx0 = (pos as usize).min(len - 1);
            let idx1 = (idx0 + 1).min(len - 1);
            let frac = (pos - (idx0 as f64)) as f32;
            let s0 = (samples[idx0] as f32) / 128.0;
            let s1 = (samples[idx1] as f32) / 128.0;
            s0 + (s1 - s0) * frac
        }
        M4aInterpolation::CubicHermite => {
            // 4-point Catmull-Rom cubic Hermite spline
            let idx = pos as isize;
            let frac = (pos - (idx as f64)) as f32;

            let p0 = get_sample_clamped(samples, idx - 1);
            let p1 = get_sample_clamped(samples, idx);
            let p2 = get_sample_clamped(samples, idx + 1);
            let p3 = get_sample_clamped(samples, idx + 2);

            let a = -0.5 * p0 + 1.5 * p1 - 1.5 * p2 + 0.5 * p3;
            let b = p0 - 2.5 * p1 + 2.0 * p2 - 0.5 * p3;
            let c = -0.5 * p0 + 0.5 * p2;
            let d = p1;

            ((a * frac + b) * frac + c) * frac + d
        }
        M4aInterpolation::Sinc => {
            // 8-point windowed Sinc interpolation with Hann window
            let idx = pos as isize;
            let frac = pos - (idx as f64);
            let mut sum = 0.0f64;
            let mut weight_sum = 0.0f64;

            for i in -3..=4 {
                let sample_val = get_sample_clamped(samples, idx + i) as f64;
                let x = (i as f64) - frac;
                let sinc_val = if x.abs() < 1e-6 {
                    1.0
                } else {
                    let pix = std::f64::consts::PI * x;
                    pix.sin() / pix
                };
                // Hann window over [-4, 4]
                let window = 0.5 * (1.0 + (std::f64::consts::PI * x / 4.0).cos());
                let w = sinc_val * window;
                sum += sample_val * w;
                weight_sum += w;
            }

            if weight_sum.abs() > 1e-6 {
                (sum / weight_sum) as f32
            } else {
                get_sample_clamped(samples, idx)
            }
        }
    }
}

#[inline(always)]
fn get_sample_clamped(samples: &[i8], idx: isize) -> f32 {
    let clamped = idx.clamp(0, (samples.len() as isize) - 1) as usize;
    (samples[clamped] as f32) / 128.0
}

/// Synthesize CGB PSG audio waveform (channels 1-4)
fn sample_cgb(cgb_type: u8, phase: f32) -> f32 {
    match cgb_type {
        1 | 2 => {
            // Square wave (50% duty cycle default)
            if phase < 0.5 { 0.8 } else { -0.8 }
        }
        3 => {
            // Triangle / Pseudo-sine wave for Wave channel
            if phase < 0.5 {
                (phase * 4.0 - 1.0) * 0.7
            } else {
                ((1.0 - phase) * 4.0 - 1.0) * 0.7
            }
        }
        4 => {
            // White noise (pseudo-random linear feedback)
            let n = (phase * 1000.0).sin();
            if n > 0.0 { 0.6 } else { -0.6 }
        }
        _ => 0.0,
    }
}

/// Smooth soft limiter preventing digital rail buzz
#[inline(always)]
fn soft_limit(x: f32) -> f32 {
    if x > 1.0 {
        1.0 - 1.0 / (1.0 + (x - 1.0))
    } else if x < -1.0 {
        -1.0 + 1.0 / (1.0 + (-x - 1.0))
    } else {
        x - (x * x * x) / 6.0
    }
}
