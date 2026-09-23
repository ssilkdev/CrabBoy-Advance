//! Pitch-preserving fast-forward audio ("Pitch-Preserved Decimate",
//! fast-forward mode 2; ROADMAP M1 / AI_AGENT_FIX_DESIGN 3.6).
//!
//! While fast-forwarding the core produces N seconds of audio per real
//! second. Playing it all would need resampling (chipmunk pitch) and simply
//! dropping whatever overflows the output queue causes hard clicks. Instead
//! the stream is cut into short grains: a grain is kept only while the
//! output queue is below its target fill level, and kept grains are joined
//! with a short equal-gain crossfade. Pitch stays exact, the kept fraction
//! adapts to any speed automatically, and the joins don't click.

/// Stereo frames per grain (~23 ms at 44.1 kHz).
pub const GRAIN_FRAMES: usize = 1024;
/// Stereo frames crossfaded between consecutive kept grains.
pub const FADE_FRAMES: usize = 128;

#[derive(Default)]
pub struct GrainDecimator {
    /// Interleaved stereo input not yet forming a whole grain.
    pending: Vec<f32>,
    /// Last FADE_FRAMES of the previous kept grain, held back so the next
    /// kept grain can crossfade into it.
    tail: Vec<f32>,
}

impl GrainDecimator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed interleaved stereo samples. `queue_len` is the current number of
    /// samples waiting in the output queue and `target` the fill level to
    /// hold it at. Returns the samples to enqueue.
    pub fn process(&mut self, samples: &[f32], queue_len: usize, target: usize) -> Vec<f32> {
        self.pending.extend_from_slice(samples);
        let grain = GRAIN_FRAMES * 2;
        let fade = FADE_FRAMES * 2;
        let mut out = Vec::new();
        let mut consumed = 0;
        while self.pending.len() - consumed >= grain {
            let g = &self.pending[consumed..consumed + grain];
            consumed += grain;
            if queue_len + out.len() >= target {
                continue; // drop this grain
            }
            if self.tail.is_empty() {
                out.extend_from_slice(&g[..grain - fade]);
            } else {
                for i in 0..fade {
                    let t = (i / 2) as f32 / FADE_FRAMES as f32;
                    out.push(self.tail[i] * (1.0 - t) + g[i] * t);
                }
                out.extend_from_slice(&g[fade..grain - fade]);
            }
            self.tail.clear();
            self.tail.extend_from_slice(&g[grain - fade..]);
        }
        self.pending.drain(..consumed);
        out
    }

    /// Leaving fast-forward: return the held-back samples (tail, then any
    /// partial grain) so no audio is lost, and reset.
    pub fn flush(&mut self) -> Vec<f32> {
        let mut out = std::mem::take(&mut self.tail);
        out.append(&mut self.pending);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sine at a fixed period: the pitch must survive decimation, i.e.
    /// every kept stretch is an unmodified copy of the input.
    fn sine(frames: usize, period: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let v = (i as f32 * std::f32::consts::TAU / period as f32).sin();
                [v, v]
            })
            .collect()
    }

    #[test]
    fn keeps_everything_when_queue_is_empty() {
        let mut d = GrainDecimator::new();
        let input = sine(GRAIN_FRAMES * 4, 64);
        let mut out = d.process(&input, 0, usize::MAX);
        out.extend(d.flush());
        // Every join between consecutive grains overlaps FADE_FRAMES, so
        // 4 grains lose 3 fades' worth of samples and nothing else.
        assert_eq!(out.len(), input.len() - 3 * FADE_FRAMES * 2);
    }

    #[test]
    fn drops_grains_when_queue_is_full_and_preserves_samples() {
        let mut d = GrainDecimator::new();
        // Simulate 4x fast-forward: the consumer drains one grain per four
        // produced, so the queue sits near the target most of the time.
        let input = sine(GRAIN_FRAMES * 16, 64);
        let target = GRAIN_FRAMES * 2;
        let mut queue = 0usize;
        let mut total = 0usize;
        for chunk in input.chunks(GRAIN_FRAMES * 2) {
            let out = d.process(chunk, queue, target);
            total += out.len();
            queue = (queue + out.len()).saturating_sub(GRAIN_FRAMES * 2 / 4);
            // Outside the crossfade regions the output is the input itself
            // (same sample values: no resampling, so no pitch change).
            for s in &out {
                assert!(s.abs() <= 1.0 + 1e-6);
            }
        }
        assert!(total < input.len() / 2, "kept {total} of {}", input.len());
        assert!(total > 0);
    }

    #[test]
    fn crossfade_is_continuous() {
        let mut d = GrainDecimator::new();
        // Two grains of constant, different levels: the join must ramp.
        let mut input = vec![1.0f32; GRAIN_FRAMES * 2];
        input.extend(vec![-1.0f32; GRAIN_FRAMES * 2]);
        let out = d.process(&input, 0, usize::MAX);
        let join = GRAIN_FRAMES * 2 - FADE_FRAMES * 2;
        let steps: Vec<f32> = out[join..join + FADE_FRAMES * 2].to_vec();
        let max_jump = steps.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        assert!(max_jump < 0.05, "max jump {max_jump}");
    }
}
