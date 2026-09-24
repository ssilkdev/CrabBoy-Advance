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

/// When set, `AudioOutput::new` does not open a host audio device
/// (ROADMAP M2). Headless runs (replays, tests, tools, run-ahead's shadow
/// core) don't need one, and on Android a process without an Activity
/// can't open one at all. See `Gba::new_headless`.
static HEADLESS: AtomicBool = AtomicBool::new(false);

/// Make AudioOutputs created from now on headless (or not); returns the
/// previous setting so callers can scope it (see `Gba::new_headless`).
pub fn set_headless(on: bool) -> bool {
    HEADLESS.swap(on, Ordering::Relaxed)
}

/// Queue fill band the resampler's rate control aims for (interleaved
/// samples): below LOW it speeds up output by 0.5%, above HIGH it slows it.
const RATE_LOW: usize = 1000;
const RATE_HIGH: usize = 3000;
/// Interleaved samples the device plays between two emulated frames at
/// slow-motion `speed`.
fn slowmo_gap(speed: f32) -> usize {
    (super::resample::CORE_SAMPLE_RATE as f32 * 2.0 / 60.0 / speed.max(0.05)) as usize
}

/// Queue band for slow motion (ROADMAP M10). The core delivers a frame's
/// audio in one burst per emulated frame, and at speed `s` those bursts are
/// 1/(60 s) seconds apart, so the queue must hold at least one gap's worth
/// (plus a stretcher hop) or the device runs dry between frames. At 10%
/// speed that's ~200 ms of latency, which is fine while slowed down.
fn slowmo_band(speed: f32) -> (usize, usize) {
    let per_gap = slowmo_gap(speed);
    let low = (per_gap + 2 * super::slowmo_stretch::HOP * 2 + 600).min(QUEUE_CAPACITY / 2);
    (low, (low + 2000).min(QUEUE_CAPACITY - 4096))
}
/// Output queue capacity in interleaved samples (~680 ms at 48 kHz). Only
/// slow motion fills it this far (see `slowmo_band`); at normal speed the
/// rate control keeps it within RATE_LOW..RATE_HIGH, so latency is the same
/// as with a small queue.
const QUEUE_CAPACITY: usize = 65536;
/// Length of the fade applied when a batch has to be cut short.
const FADE_SAMPLES: usize = 256;

/// Output-queue fill level (interleaved samples, ~45 ms of stereo at
/// 44.1 kHz) the fast-forward decimator holds the queue at.
const FF_TARGET_FILL: usize = 4000;

pub struct AudioOutput {
    _stream: Option<cpal::Stream>,
    /// Whether `_stream` is playing (see `set_stream_active`).
    stream_active: bool,
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
    /// Core rate -> device rate, with rate control (see `resample`).
    resampler: Mutex<super::resample::Resampler>,
    /// Slow-motion speed as f32 bits (1.0 = normal; ROADMAP M10).
    slow_motion_speed: AtomicU32,
    /// Slow-motion audio: 0 pitch-preserved, 1 tape, 2 mute.
    slow_motion_audio: AtomicU8,
    /// Pitch-preserving stretcher for slow motion.
    stretcher: Mutex<super::slowmo_stretch::TimeStretcher>,
    /// Slow-motion speed seen by the previous batch (f32 bits).
    last_slow_speed: AtomicU32,
    /// Dropping grains to shed leftover slow-motion buffering.
    catching_up: AtomicBool,
}

impl Default for AudioOutput {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioOutput {
    /// Stop or restart the device stream. A paused stream lets the audio
    /// hardware and its callback thread sleep (Android: the AAudio stream
    /// stops waking the CPU every few ms). Callers pause it only when no
    /// samples are being produced (muted, in a menu, in the background).
    pub fn set_stream_active(&mut self, active: bool) {
        let Some(stream) = self._stream.as_ref() else { return };
        if active == self.stream_active {
            return;
        }
        let res = if active { stream.play().map_err(|e| e.to_string()) } else { stream.pause().map_err(|e| e.to_string()) };
        match res {
            Ok(()) => self.stream_active = active,
            Err(e) => log::warn!("Could not {} audio stream: {e}", if active { "resume" } else { "pause" }),
        }
    }

    pub fn is_stream_active(&self) -> bool {
        self.stream_active
    }

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

        let (stream, sample_rate, channels) = if HEADLESS.load(Ordering::Relaxed) {
            (None, super::resample::CORE_SAMPLE_RATE, 2)
        } else {
            Self::init_cpal_stream(
                buffer_clone,
                Arc::clone(&surround_mode),
                Arc::clone(&bass_boost),
                Arc::clone(&surround_width),
            )
            .unwrap_or((None, 44100, 2))
        };

        Self {
            _stream: stream,
            stream_active: true,
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
            resampler: Mutex::new(super::resample::Resampler::new(sample_rate)),
            slow_motion_speed: AtomicU32::new(1.0f32.to_bits()),
            slow_motion_audio: AtomicU8::new(0),
            stretcher: Mutex::new(super::slowmo_stretch::TimeStretcher::new()),
            last_slow_speed: AtomicU32::new(1.0f32.to_bits()),
            catching_up: AtomicBool::new(false),
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

    /// Tell the output how fast emulation runs relative to real time
    /// (ROADMAP M10 slow motion). Values >= 1.0 mean normal speed; fast
    /// forward is handled separately by `set_fast_forwarding`.
    pub fn set_slow_motion(&self, speed: f32, audio: crate::gba::accessibility::SlowMotionAudio) {
        use crate::gba::accessibility::SlowMotionAudio;
        let speed = if speed.is_finite() { speed.clamp(0.05, 1.0) } else { 1.0 };
        self.slow_motion_speed.store(speed.to_bits(), Ordering::Relaxed);
        let mode = match audio {
            SlowMotionAudio::PitchPreserved => 0,
            SlowMotionAudio::Tape => 1,
            SlowMotionAudio::Mute => 2,
        };
        self.slow_motion_audio.store(mode, Ordering::Relaxed);
    }

    pub fn slow_motion_speed(&self) -> f32 {
        f32::from_bits(self.slow_motion_speed.load(Ordering::Relaxed))
    }

    /// Take up to `n` queued samples, as the device callback would. For
    /// headless use (tests, tools) where no device drains the queue.
    pub fn drain_queued(&self, n: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(n);
        while out.len() < n {
            match self.buffer.pop() {
                Some(v) => out.push(v),
                None => break,
            }
        }
        out
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

    /// Queue a batch of core-rate (`CORE_SAMPLE_RATE`) interleaved stereo
    /// samples for playback. Everything host-dependent happens here, after
    /// the core: fast-forward muting/decimation, resampling to the device
    /// rate and rate control against the queue fill.
    pub fn push_sample_batch(&self, samples: &[f32]) {
        if self.muted || samples.is_empty() {
            return;
        }
        // Slow motion (ROADMAP M10): the core makes `speed` seconds of
        // audio per real second, so stretch it back to real time.
        let slow = if self.is_fast_forwarding() { 1.0 } else { self.slow_motion_speed() };
        let slow_mode = self.slow_motion_audio.load(Ordering::Relaxed);
        let slowed = slow < 0.999;
        // Entering slow motion (or going slower): the queue must jump to the
        // deeper band at once, so top it up with silence (a few tens of ms,
        // once) instead of letting the device run dry while rate control
        // creeps up to it.
        let prev_slow = f32::from_bits(self.last_slow_speed.swap(slow.to_bits(), Ordering::Relaxed));
        let fresh_stretch = slow_mode == 0 && self.stretcher.lock().map(|st| !st.is_active()).unwrap_or(false);
        if slowed && (slow < prev_slow - 0.001 || fresh_stretch) {
            let (lo, hi) = slowmo_band(slow);
            // A fresh time stretcher holds back its first grain, which at
            // low speeds is more than one frame's input: cover one more gap.
            let want = (lo + hi) / 2 + if fresh_stretch { slowmo_gap(slow) } else { 0 };
            let have = self.buffer_len();
            if have < want {
                self.buffer.push_slice(&vec![0.0; (want - have) & !1]);
            }
        }
        let stretched;
        let silent;
        let samples = if (self.is_fast_forwarding() && self.fast_forward_mode() == 1) || (slowed && slow_mode == 2) {
            // Smart mute: keep the stream flowing, silently. In slow motion
            // the silence must also last 1/speed times longer.
            let len = if slowed { ((samples.len() as f32 / slow) as usize) & !1 } else { samples.len() };
            silent = vec![0.0; len];
            &silent[..]
        } else if slowed && slow_mode == 0 {
            stretched = match self.stretcher.lock() {
                Ok(mut st) => st.process(samples, slow),
                Err(_) => samples.to_vec(),
            };
            &stretched[..]
        } else {
            // Leaving pitch-preserved slow motion: play out its tail.
            if let Ok(mut st) = self.stretcher.lock() {
                if st.is_active() {
                    stretched = { let mut t = st.flush(); t.extend_from_slice(samples); t };
                    self.enqueue_resampled(&stretched, 1.0, (RATE_LOW, RATE_HIGH));
                    return;
                }
            }
            samples
        };
        let tape_speed = if slowed && slow_mode == 1 { slow } else { 1.0 };
        let band = if slowed { slowmo_band(slow) } else { (RATE_LOW, RATE_HIGH) };
        self.enqueue_resampled(samples, tape_speed, band);
    }

    /// Resample to the device rate, apply fast-forward decimation, queue.
    fn enqueue_resampled(&self, samples: &[f32], tape_speed: f32, (low, high): (usize, usize)) {
        let resampled = match self.resampler.lock() {
            Ok(mut r) => r.process_at_speed(samples, self.buffer_len(), low, high, tape_speed),
            Err(_) => samples.to_vec(),
        };
        let samples = &resampled[..];
        if samples.is_empty() {
            return;
        }

        // Fast-forward mode 2: keep whole grains only while the queue has
        // room (pitch unchanged, crossfaded joins). Outside fast-forward,
        // hand back anything the decimator still holds first.
        // Also used to catch up after slow motion: the queue is then far
        // deeper than the normal band, and rate control's 0.5% nudge would
        // take a minute to remove that latency. Dropping whole grains (same
        // pitch, crossfaded) gets back to normal in about half a second.
        let q_len = self.buffer_len();
        // Before a push the queue is at most RATE_HIGH at normal speed, plus
        // under one frame's burst of jitter, so above that it's leftover.
        // Once started, keep going until the queue is back in the band.
        let normal = tape_speed >= 1.0 && high == RATE_HIGH;
        let was = self.catching_up.load(Ordering::Relaxed);
        let catching_up = normal && if was { q_len > RATE_HIGH } else { q_len > RATE_HIGH + 1600 };
        self.catching_up.store(catching_up, Ordering::Relaxed);
        let decimate = (self.is_fast_forwarding() && self.fast_forward_mode() == 2) || catching_up;
        let target = if catching_up { (RATE_LOW + RATE_HIGH) / 2 } else { FF_TARGET_FILL };
        let staged: Option<Vec<f32>> = match self.ff_decimator.lock() {
            Ok(mut d) if decimate => Some(d.process(samples, q_len, target)),
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


