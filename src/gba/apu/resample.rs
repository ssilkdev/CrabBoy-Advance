//! Core-rate -> device-rate resampler with dynamic rate control
//! (ROADMAP M2).
//!
//! The emulator cores produce audio at a fixed [`CORE_SAMPLE_RATE`] so the
//! sample stream is identical on every machine (a requirement for
//! deterministic replays and frame/audio hashes). Matching the host device
//! (44.1 kHz, 48 kHz, ...) and absorbing clock drift between the emulated
//! and real time bases happens here, on the output side only: linear
//! interpolation at a ratio nudged by +-0.5% depending on how full the
//! output queue is. This used to be done by changing how many core cycles
//! make one sample, which made the core's audio depend on the host.

/// Sample rate every emulator core produces, in Hz.
pub const CORE_SAMPLE_RATE: u32 = 48_000;

pub struct Resampler {
    /// Output samples per input sample (device rate / core rate).
    base_step: f64,
    /// Fractional read position between `prev` and the next input frame.
    pos: f64,
    prev: [f32; 2],
    primed: bool,
}

impl Resampler {
    pub fn new(device_rate: u32) -> Self {
        Self {
            base_step: CORE_SAMPLE_RATE as f64 / device_rate.max(1) as f64,
            pos: 0.0,
            prev: [0.0; 2],
            primed: false,
        }
    }

    /// Resample interleaved stereo `input`. `fill` is the output queue's
    /// current level and `low`/`high` the band to hold it in: below `low`
    /// produce 0.5% more output, above `high` 0.5% less.
    pub fn process(&mut self, input: &[f32], fill: usize, low: usize, high: usize) -> Vec<f32> {
        self.process_at_speed(input, fill, low, high, 1.0)
    }

    /// Like `process`, but plays the input at `speed` (0 < speed <= 1):
    /// fewer input frames per output frame, so the audio lasts `1 / speed`
    /// times longer at proportionally lower pitch ("tape" slow motion,
    /// ROADMAP M10).
    pub fn process_at_speed(&mut self, input: &[f32], fill: usize, low: usize, high: usize, speed: f32) -> Vec<f32> {
        let speed = (speed as f64).clamp(0.05, 1.0);
        let adjust = if fill > high + high / 2 {
            1.015
        } else if fill > high {
            1.005
        } else if fill < low / 2 {
            0.985
        } else if fill < low {
            0.995
        } else {
            1.0
        };
        // Input frames consumed per output frame.
        let step = self.base_step * adjust * speed;
        let mut out = Vec::with_capacity((input.len() as f64 / step) as usize + 4);
        for frame in input.chunks_exact(2) {
            let cur = [frame[0], frame[1]];
            if !self.primed {
                self.prev = cur;
                self.primed = true;
                continue;
            }
            // Emit every output frame whose position lies in [prev, cur).
            while self.pos < 1.0 {
                let t = self.pos as f32;
                out.push(self.prev[0] + (cur[0] - self.prev[0]) * t);
                out.push(self.prev[1] + (cur[1] - self.prev[1]) * t);
                self.pos += step;
            }
            self.pos -= 1.0;
            self.prev = cur;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(frames: usize) -> Vec<f32> {
        (0..frames).flat_map(|i| {
            let v = (i as f32 * 0.05).sin();
            [v, -v]
        }).collect()
    }

    #[test]
    fn converts_rate_within_band() {
        let mut r = Resampler::new(44_100);
        let out = r.process(&tone(48_000), 2000, 1000, 3000);
        let frames = out.len() / 2;
        assert!((44_090..=44_110).contains(&frames), "{frames}");
    }

    #[test]
    fn rate_control_nudges_output_length() {
        let mut lo = Resampler::new(48_000);
        let mut hi = Resampler::new(48_000);
        let n_lo = lo.process(&tone(48_000), 0, 1000, 3000).len() / 2;
        let n_hi = hi.process(&tone(48_000), 9999, 1000, 3000).len() / 2;
        assert!(n_lo > 48_000 && n_hi < 48_000, "{n_lo} {n_hi}");
    }

    #[test]
    fn tape_speed_lengthens_output() {
        let mut r = Resampler::new(CORE_SAMPLE_RATE);
        let out = r.process_at_speed(&tone(4800), 2000, 1000, 3000, 0.5);
        let frames = out.len() / 2;
        assert!((9590..=9610).contains(&frames), "{frames}");
    }

    #[test]
    fn identity_rate_is_transparent() {
        let mut r = Resampler::new(CORE_SAMPLE_RATE);
        let input = tone(100);
        let out = r.process(&input, 2000, 1000, 3000);
        // One frame of latency, otherwise sample-exact.
        assert_eq!(&out[..], &input[..out.len()]);
    }
}
