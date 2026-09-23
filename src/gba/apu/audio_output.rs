//! Host Audio Output Streaming via CPAL with 5.1 Surround & 3D Headphone Spatialization

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use super::ring::SampleRing;
use super::spatial::SpatialDsp;
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

/// Output queue capacity in interleaved samples.
const QUEUE_CAPACITY: usize = 8192;
/// Length of the fade applied when a batch has to be cut short.
const FADE_SAMPLES: usize = 256;

/// Output-queue fill level (interleaved samples, ~45 ms of stereo at
/// 44.1 kHz) the fast-forward decimator holds the queue at.
const FF_TARGET_FILL: usize = 4000;

pub struct AudioOutput {
    _stream: Option<cpal::Stream>,
    buffer: Arc<SampleRing>,
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
    /// Grain decimator for fast-forward mode 2 (see `ff_stretch`).
    ff_decimator: Mutex<super::ff_stretch::GrainDecimator>,
}

impl Default for AudioOutput {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioOutput {
    pub fn new() -> Self {
        // ~90 ms of stereo at 44.1 kHz; the rate control in Apu keeps the
        // fill around 1000-3000 samples.
        let buffer = Arc::new(SampleRing::new(QUEUE_CAPACITY));
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
            ff_decimator: Mutex::new(super::ff_stretch::GrainDecimator::new()),
        }
    }

    fn init_cpal_stream(
        buffer: Arc<SampleRing>,
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

        // All DSP state lives in the callback (see `spatial`).
        let mut dsp = SpatialDsp::new(sample_rate);

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_output_stream(
                &config.into(),
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    let mode = SurroundMode::from_u8(surround_mode.load(Ordering::Relaxed));
                    let bass = f32::from_bits(bass_boost.load(Ordering::Relaxed)).clamp(0.0, 1.0);
                    let width = f32::from_bits(surround_width.load(Ordering::Relaxed)).clamp(0.0, 1.0);
                    // Lock-free: never blocks, and an underrun plays
                    // silence (every frame is always written).
                    for frame in data.chunks_mut(channels) {
                        let (left, right) = if buffer.len() >= 2 {
                            (buffer.pop().unwrap_or(0.0), buffer.pop().unwrap_or(0.0))
                        } else {
                            (0.0, 0.0)
                        };
                        dsp.process(left, right, mode, bass, width, frame);
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
        self.buffer.len()
    }

    /// Push samples, scaling by volume. If the queue is nearly full (the
    /// emulator is running ahead of the audio device), the batch is
    /// thinned gradually: a short fade-out of what fits instead of the old
    /// "drop 3000 queued samples at once", which clicked.
    fn enqueue(&self, samples: &[f32]) {
        let room = self.buffer.capacity().saturating_sub(self.buffer.len());
        let mut scaled: Vec<f32> = samples.iter().map(|&s| (s * self.volume).clamp(-1.0, 1.0)).collect();
        if scaled.len() > room {
            let keep = room & !1; // whole stereo frames
            let fade = keep.min(FADE_SAMPLES);
            for i in 0..fade {
                let t = 1.0 - (i / 2) as f32 / (fade / 2).max(1) as f32;
                let idx = keep - fade + i;
                scaled[idx] *= t;
            }
            scaled.truncate(keep);
        }
        self.buffer.push_slice(&scaled);
    }

    pub fn push_sample_batch(&self, samples: &[f32]) {
        if self.muted || samples.is_empty() {
            return;
        }

        // Fast-forward mode 2: keep whole grains only while the queue has
        // room (pitch unchanged, crossfaded joins). Outside fast-forward,
        // hand back anything the decimator still holds first.
        let decimate = self.is_fast_forwarding() && self.fast_forward_mode() == 2;
        let staged: Option<Vec<f32>> = match self.ff_decimator.lock() {
            Ok(mut d) if decimate => {
                let q_len = self.buffer_len();
                Some(d.process(samples, q_len, FF_TARGET_FILL))
            }
            Ok(mut d) => {
                let mut held = d.flush();
                if held.is_empty() {
                    None
                } else {
                    held.extend_from_slice(samples);
                    Some(held)
                }
            }
            Err(_) => None,
        };
        let samples: &[f32] = staged.as_deref().unwrap_or(samples);
        if samples.is_empty() {
            return;
        }

        self.enqueue(samples);
    }

    pub fn push_samples(&self, left: f32, right: f32) {
        if self.muted {
            return;
        }

        self.enqueue(&[left, right]);
    }
}


