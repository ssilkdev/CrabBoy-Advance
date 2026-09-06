//! Host Audio Output Streaming via CPAL with 5.1 Surround & 3D Headphone Spatialization

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurroundMode {
    Stereo = 0,
    Surround51 = 1,
    Headphone3D = 2,
}

impl SurroundMode {
    pub fn from_u8(val: u8) -> Self {
        match val {
            0 => Self::Stereo,
            1 => Self::Surround51,
            _ => Self::Headphone3D,
        }
    }
}

pub struct AudioOutput {
    _stream: Option<cpal::Stream>,
    buffer: Arc<Mutex<VecDeque<f32>>>,
    sample_rate: u32,
    channels: usize,
    pub volume: f32,
    pub muted: bool,
    surround_mode: Arc<AtomicU8>,
    bass_boost: Arc<AtomicU32>,
    surround_width: Arc<AtomicU32>,
    pub channel_mute_mask: Arc<AtomicU8>,
    pub fast_forward_mode: Arc<AtomicU8>, // 0: Normal, 1: Smart Mute, 2: Pitch-Preserved Decimate
    pub is_fast_forwarding: Arc<AtomicBool>,
}

impl AudioOutput {
    pub fn new() -> Self {
        let buffer = Arc::new(Mutex::new(VecDeque::with_capacity(8192)));
        let buffer_clone = Arc::clone(&buffer);

        let surround_mode = Arc::new(AtomicU8::new(SurroundMode::Headphone3D as u8));
        let bass_boost = Arc::new(AtomicU32::new(0.65_f32.to_bits()));
        let surround_width = Arc::new(AtomicU32::new(0.85_f32.to_bits()));
        let channel_mute_mask = Arc::new(AtomicU8::new(0)); // All unmuted
        let fast_forward_mode = Arc::new(AtomicU8::new(1)); // Default Smart Mute during fast-forward
        let is_fast_forwarding = Arc::new(AtomicBool::new(false));

        let (stream, sample_rate, channels) = Self::init_cpal_stream(
            buffer_clone,
            Arc::clone(&surround_mode),
            Arc::clone(&bass_boost),
            Arc::clone(&surround_width),
        )
        .unwrap_or((None, 44100, 2));

        Self {
            _stream: stream,
            buffer,
            sample_rate,
            channels,
            volume: 0.8,
            muted: false,
            surround_mode,
            bass_boost,
            surround_width,
            channel_mute_mask,
            fast_forward_mode,
            is_fast_forwarding,
        }
    }

    fn init_cpal_stream(
        buffer: Arc<Mutex<VecDeque<f32>>>,
        surround_mode: Arc<AtomicU8>,
        bass_boost: Arc<AtomicU32>,
        surround_width: Arc<AtomicU32>,
    ) -> Result<(Option<cpal::Stream>, u32, usize), cpal::DefaultStreamConfigError> {
        let host = cpal::default_host();
        let device = match host.default_output_device() {
            Some(d) => d,
            None => {
                log::warn!("No default audio output device found");
                return Ok((None, 44100, 2));
            }
        };

        let config = match device.default_output_config() {
            Ok(c) => c,
            Err(e) => {
                log::warn!("Failed to get default output audio config: {}", e);
                return Ok((None, 44100, 2));
            }
        };

        let sample_rate = config.sample_rate().0;
        let channels = config.channels() as usize;

        let err_fn = |err| log::error!("Audio stream error: {}", err);

        // DSP state variables local to the audio callback
        let mut lfe_filter_state = 0.0_f32;
        let lfe_alpha = (2.0 * std::f32::consts::PI * 120.0 / (sample_rate as f32)).clamp(0.001, 0.1);

        // Acoustic delay buffer (~12ms delay: ~530 samples at 44.1kHz)
        let delay_len = ((sample_rate as f32 * 0.012) as usize).clamp(64, 1024);
        let mut delay_buf_l = vec![0.0_f32; delay_len];
        let mut delay_buf_r = vec![0.0_f32; delay_len];
        let mut delay_idx = 0usize;

        // Headphone crossfeed filter state
        let mut cross_l = 0.0_f32;
        let mut cross_r = 0.0_f32;
        let cross_alpha = (2.0 * std::f32::consts::PI * 700.0 / (sample_rate as f32)).clamp(0.01, 0.3);

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_output_stream(
                &config.into(),
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    let mode = SurroundMode::from_u8(surround_mode.load(Ordering::Relaxed));
                    let bass = f32::from_bits(bass_boost.load(Ordering::Relaxed)).clamp(0.0, 1.0);
                    let width = f32::from_bits(surround_width.load(Ordering::Relaxed)).clamp(0.0, 1.0);

                    if let Ok(mut q) = buffer.lock() {
                        for frame in data.chunks_mut(channels) {
                            let (left, right) = if q.len() >= 2 {
                                (q.pop_front().unwrap_or(0.0), q.pop_front().unwrap_or(0.0))
                            } else {
                                (0.0, 0.0)
                            };

                            // 1. Subwoofer / LFE Low-Pass Filter
                            let mono = (left + right) * 0.5;
                            lfe_filter_state += lfe_alpha * (mono - lfe_filter_state);
                            let sub_lfe = (lfe_filter_state * (1.0 + bass * 2.5)).clamp(-1.0, 1.0);

                            // 2. Spatial Ambience Difference with Haas Acoustic Delay
                            let diff_l = left - 0.5 * right;
                            let diff_r = right - 0.5 * left;
                            let delayed_l = delay_buf_l[delay_idx];
                            let delayed_r = delay_buf_r[delay_idx];
                            delay_buf_l[delay_idx] = diff_l;
                            delay_buf_r[delay_idx] = diff_r;
                            delay_idx = (delay_idx + 1) % delay_len;

                            let surround_l = ((diff_l * 0.5 + delayed_l * 0.5) * width).clamp(-1.0, 1.0);
                            let surround_r = ((diff_r * 0.5 + delayed_r * 0.5) * width).clamp(-1.0, 1.0);
                            let center = ((left + right) * 0.7071).clamp(-1.0, 1.0);

                            // 3. Headphone 3D Spatializer (Bauer Crossfeed & Sub-Bass Reinforcement)
                            cross_l += cross_alpha * (right - cross_l);
                            cross_r += cross_alpha * (left - cross_r);
                            let hp_3d_l = (left * 0.80 + cross_l * 0.22 + sub_lfe * 0.25).clamp(-1.0, 1.0);
                            let hp_3d_r = (right * 0.80 + cross_r * 0.22 + sub_lfe * 0.25).clamp(-1.0, 1.0);

                            // 4. Channel Output Distribution
                            if channels >= 6 {
                                // Multi-channel 5.1 / 7.1 Surround
                                match mode {
                                    SurroundMode::Stereo => {
                                        frame[0] = left;
                                        frame[1] = right;
                                        for s in &mut frame[2..] {
                                            *s = 0.0;
                                        }
                                    }
                                    SurroundMode::Surround51 | SurroundMode::Headphone3D => {
                                        frame[0] = left;         // Front Left
                                        frame[1] = right;        // Front Right
                                        frame[2] = center;       // Center
                                        frame[3] = sub_lfe;      // Subwoofer (LFE)
                                        frame[4] = surround_l;   // Surround Left
                                        frame[5] = surround_r;   // Surround Right
                                        if channels >= 8 {
                                            frame[6] = surround_l * 0.85; // Side Left (7.1)
                                            frame[7] = surround_r * 0.85; // Side Right (7.1)
                                        }
                                    }
                                }
                            } else if channels == 2 {
                                // 2-Channel Output (Headphones / Stereo Speakers)
                                match mode {
                                    SurroundMode::Stereo => {
                                        frame[0] = left;
                                        frame[1] = right;
                                    }
                                    SurroundMode::Surround51 => {
                                        // 5.1 Matrix Virtual Downmix to Stereo Headphones
                                        let downmix_l = (left + center * 0.7 + surround_l * 0.7 + sub_lfe * 0.35) * 0.65;
                                        let downmix_r = (right + center * 0.7 + surround_r * 0.7 + sub_lfe * 0.35) * 0.65;
                                        frame[0] = downmix_l.clamp(-1.0, 1.0);
                                        frame[1] = downmix_r.clamp(-1.0, 1.0);
                                    }
                                    SurroundMode::Headphone3D => {
                                        frame[0] = hp_3d_l;
                                        frame[1] = hp_3d_r;
                                    }
                                }
                            } else if channels == 1 {
                                frame[0] = mono.clamp(-1.0, 1.0);
                            }
                        }
                    }
                },
                err_fn,
                None,
            ),
            _ => {
                log::warn!("Unsupported audio sample format");
                return Ok((None, sample_rate, channels));
            }
        };

        match stream {
            Ok(s) => {
                if let Err(e) = s.play() {
                    log::warn!("Failed to start audio stream: {}", e);
                    Ok((None, sample_rate, channels))
                } else {
                    log::info!(
                        "Spatial Audio initialized: {}Hz ({} physical hardware channels)",
                        sample_rate,
                        channels
                    );
                    Ok((Some(s), sample_rate, channels))
                }
            }
            Err(e) => {
                log::warn!("Failed to build audio stream: {}", e);
                Ok((None, sample_rate, channels))
            }
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn hardware_channels(&self) -> usize {
        self.channels
    }

    pub fn surround_mode(&self) -> SurroundMode {
        SurroundMode::from_u8(self.surround_mode.load(Ordering::Relaxed))
    }

    pub fn set_surround_mode(&self, mode: SurroundMode) {
        self.surround_mode.store(mode as u8, Ordering::Relaxed);
    }

    pub fn bass_boost(&self) -> f32 {
        f32::from_bits(self.bass_boost.load(Ordering::Relaxed))
    }

    pub fn set_bass_boost(&self, val: f32) {
        self.bass_boost.store(val.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn surround_width(&self) -> f32 {
        f32::from_bits(self.surround_width.load(Ordering::Relaxed))
    }

    pub fn set_surround_width(&self, val: f32) {
        self.surround_width.store(val.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn channel_mute_mask(&self) -> u8 {
        self.channel_mute_mask.load(Ordering::Relaxed)
    }

    pub fn set_channel_mute_mask(&self, mask: u8) {
        self.channel_mute_mask.store(mask, Ordering::Relaxed);
    }

    pub fn is_channel_muted(&self, ch: usize) -> bool {
        (self.channel_mute_mask.load(Ordering::Relaxed) & (1 << ch)) != 0
    }

    pub fn set_channel_muted(&self, ch: usize, muted: bool) {
        let cur = self.channel_mute_mask.load(Ordering::Relaxed);
        let new_mask = if muted {
            cur | (1 << ch)
        } else {
            cur & !(1 << ch)
        };
        self.channel_mute_mask.store(new_mask, Ordering::Relaxed);
    }

    pub fn fast_forward_mode(&self) -> u8 {
        self.fast_forward_mode.load(Ordering::Relaxed)
    }

    pub fn set_fast_forward_mode(&self, mode: u8) {
        self.fast_forward_mode.store(mode, Ordering::Relaxed);
    }

    pub fn is_fast_forwarding(&self) -> bool {
        self.is_fast_forwarding.load(Ordering::Relaxed)
    }

    pub fn set_fast_forwarding(&self, active: bool) {
        self.is_fast_forwarding.store(active, Ordering::Relaxed);
    }

    pub fn buffer_len(&self) -> usize {
        self.buffer.lock().map(|q| q.len()).unwrap_or(0)
    }

    pub fn push_sample_batch(&self, samples: &[f32]) {
        if self.muted || samples.is_empty() {
            return;
        }

        if let Ok(mut q) = self.buffer.lock() {
            if q.len() > 6000 {
                let excess = q.len() - 3000;
                q.drain(0..excess);
            }

            for &s in samples {
                let sample = (s * self.volume).clamp(-1.0, 1.0);
                q.push_back(sample);
            }
        }
    }

    pub fn push_samples(&self, left: f32, right: f32) {
        if self.muted {
            return;
        }

        if let Ok(mut q) = self.buffer.lock() {
            if q.len() > 6000 {
                let excess = q.len() - 3000;
                q.drain(0..excess);
            }

            let l = (left * self.volume).clamp(-1.0, 1.0);
            let r = (right * self.volume).clamp(-1.0, 1.0);
            q.push_back(l);
            q.push_back(r);
        }
    }
}


