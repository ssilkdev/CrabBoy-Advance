//! Slow-motion audio (ROADMAP M10).
//!
//! In slow motion the core produces less audio per real second than the
//! device plays, so without help the output queue underruns and crackles.
//! Two fixes, picked by `SlowMotionAudio`:
//!
//! - **Tape**: the resampler reads the input slower (see
//!   `Resampler::process_at_speed`). Pitch drops with speed.
//! - **Pitch-preserved**: this module. WSOLA (waveform-similarity
//!   overlap-add): Hann-windowed grains of `GRAIN` frames are laid down every
//!   `HOP` output frames, while the read position advances only
//!   `HOP * speed` input frames. Each grain's start is nudged by up to
//!   `TOLERANCE` frames to the spot that best continues the previous grain's
//!   waveform, so the overlaps line up in phase instead of beating.
//!
//! Everything is interleaved stereo `f32` at the core rate.

/// Frames per grain (~21 ms at 48 kHz).
pub const GRAIN: usize = 1024;
/// Output hop: 50% overlap, where periodic Hann windows sum to exactly 1.
pub const HOP: usize = GRAIN / 2;
/// How far a grain may move to find the best waveform match.
pub const TOLERANCE: usize = 256;

pub struct TimeStretcher {
    window: Vec<f32>,
    /// Buffered input, interleaved stereo.
    input: Vec<f32>,
    /// Next nominal read position, in frames into `input`.
    read_pos: f64,
    /// Where the previous grain's waveform naturally continues (its start
    /// plus HOP), in frames into `input`.
    cont: Option<usize>,
    /// Overlap-add accumulator, GRAIN frames interleaved.
    ola: Vec<f32>,
}

impl Default for TimeStretcher {
    fn default() -> Self {
        Self::new()
    }
}

impl TimeStretcher {
    pub fn new() -> Self {
        let window = (0..GRAIN)
            .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / GRAIN as f32).cos())
            .collect();
        Self {
            window,
            input: Vec::new(),
            read_pos: 0.0,
            cont: None,
            ola: vec![0.0; GRAIN * 2],
        }
    }

    pub fn reset(&mut self) {
        self.input.clear();
        self.read_pos = 0.0;
        self.cont = None;
        self.ola.iter_mut().for_each(|s| *s = 0.0);
    }

    /// Whether anything is buffered (so leaving slow motion should flush).
    pub fn is_active(&self) -> bool {
        !self.input.is_empty() || self.cont.is_some()
    }

    /// End of slow motion: return the pending overlap tail and reset.
    pub fn flush(&mut self) -> Vec<f32> {
        let out = if self.cont.is_some() { self.ola[..HOP * 2].to_vec() } else { Vec::new() };
        self.reset();
        out
    }

    fn frames(&self) -> usize {
        self.input.len() / 2
    }

    fn mono(&self, frame: usize) -> f32 {
        self.input[frame * 2] + self.input[frame * 2 + 1]
    }

    /// Normalized cross-correlation of `len` frames at `a` and `b`, sampled
    /// every `step` frames.
    fn similarity(&self, a: usize, b: usize, len: usize, step: usize) -> f32 {
        let (mut xy, mut yy) = (0.0f32, 0.0f32);
        let mut i = 0;
        while i < len {
            let x = self.mono(a + i);
            let y = self.mono(b + i);
            xy += x * y;
            yy += y * y;
            i += step;
        }
        xy / (yy.sqrt() + 1e-6)
    }

    /// Feed core samples and get the stretched output. `speed` is the
    /// emulation speed (0 < speed < 1 slows down; output length is about
    /// `input.len() / speed`).
    pub fn process(&mut self, samples: &[f32], speed: f32) -> Vec<f32> {
        let speed = speed.clamp(0.05, 1.0) as f64;
        self.input.extend_from_slice(samples);
        let mut out = Vec::new();

        loop {
            let nominal = self.read_pos.round() as usize;
            // The first grain isn't searched, so it needs no lookahead.
            let lookahead = if self.cont.is_some() { TOLERANCE } else { 0 };
            if nominal + lookahead + GRAIN > self.frames() {
                break;
            }
            let lo = nominal.saturating_sub(TOLERANCE);
            let hi = nominal + TOLERANCE;
            let start = match self.cont {
                None => nominal,
                Some(target) => {
                    // The previous grain's natural continuation is what the
                    // next grain's first half should look like.
                    let score = |s: &Self, c: usize| s.similarity(target, c, HOP, 2);
                    let mut best = nominal;
                    let mut best_score = f32::MIN;
                    let mut c = lo;
                    while c <= hi {
                        let v = score(self, c);
                        if v > best_score {
                            best_score = v;
                            best = c;
                        }
                        c += 8;
                    }
                    let (a, b) = (best.saturating_sub(7).max(lo), (best + 7).min(hi));
                    for c in a..=b {
                        let v = score(self, c);
                        if v > best_score {
                            best_score = v;
                            best = c;
                        }
                    }
                    best
                }
            };

            for i in 0..GRAIN {
                let w = self.window[i];
                self.ola[i * 2] += self.input[(start + i) * 2] * w;
                self.ola[i * 2 + 1] += self.input[(start + i) * 2 + 1] * w;
            }
            out.extend_from_slice(&self.ola[..HOP * 2]);
            self.ola.copy_within(HOP * 2.., 0);
            let len = self.ola.len();
            self.ola[len - HOP * 2..].iter_mut().for_each(|s| *s = 0.0);

            let cont = start + HOP;
            self.read_pos += HOP as f64 * speed;

            // Drop input nothing can reach any more.
            let keep_from = (self.read_pos as usize).saturating_sub(TOLERANCE).min(cont);
            self.input.drain(..keep_from * 2);
            self.read_pos -= keep_from as f64;
            self.cont = Some(cont - keep_from);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(frames: usize, period: f32) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let v = (i as f32 * std::f32::consts::TAU / period).sin() * 0.5;
                [v, v]
            })
            .collect()
    }

    /// Zero crossings per frame: proportional to pitch.
    fn crossings_per_frame(s: &[f32]) -> f32 {
        let left: Vec<f32> = s.iter().step_by(2).copied().collect();
        let n = left.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
        n as f32 / left.len() as f32
    }

    #[test]
    fn output_length_scales_with_speed() {
        for speed in [0.75f32, 0.5, 0.25, 0.1] {
            let mut t = TimeStretcher::new();
            let input = sine(96_000, 96.0);
            // Steady-state rate: output produced for the second half of the
            // input (the first half absorbs the start-up lookahead of
            // GRAIN + 2 * TOLERANCE input frames).
            let (first, second) = input.split_at(input.len() / 2);
            for chunk in first.chunks(1600) {
                t.process(chunk, speed);
            }
            let got: usize = second.chunks(1600).map(|c| t.process(c, speed).len() / 2).sum();
            let expect = 48_000.0 / speed;
            let rel = (got as f32 - expect).abs() / expect;
            assert!(rel < 0.01, "speed {speed}: got {got}, expected ~{expect}");
        }
    }

    #[test]
    fn pitch_is_preserved() {
        let input = sine(48_000, 96.0); // 500 Hz
        let mut t = TimeStretcher::new();
        let out: Vec<f32> = input.chunks(800).flat_map(|c| t.process(c, 0.5)).collect();
        // Skip the fade-in grain.
        let steady = &out[GRAIN * 4..];
        let (a, b) = (crossings_per_frame(&input), crossings_per_frame(steady));
        assert!((a - b).abs() / a < 0.02, "input {a} output {b}");
    }

    #[test]
    fn steady_tone_has_no_dropouts_or_clicks() {
        let input = sine(48_000, 100.0);
        let mut t = TimeStretcher::new();
        let out: Vec<f32> = input.chunks(512).flat_map(|c| t.process(c, 0.3)).collect();
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        let steady = &left[GRAIN * 2..];
        // Amplitude stays near 0.5 (phase-aligned overlaps don't cancel)...
        for win in steady.chunks(400) {
            let peak = win.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(peak > 0.42 && peak < 0.58, "peak {peak}");
        }
        // ...and no sample-to-sample jump exceeds what a 0.5-amplitude sine
        // of this period can do (2*pi*0.5/100 ~= 0.031).
        let max_jump = steady.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        assert!(max_jump < 0.05, "max jump {max_jump}");
    }
}
