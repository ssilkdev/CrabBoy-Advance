//! Output-side spatial audio DSP: stereo, 5.1/7.1 and the "3D headphone"
//! mode (ROADMAP M1, AI_AGENT_FIX_DESIGN Phase 3 APU items).
//!
//! Moved out of the CPAL callback so it can be unit-tested. Changes from
//! the original callback code:
//! - Headphone 3D now applies a real interaural time difference: the
//!   crossfed (opposite-ear) signal is low-passed *and* delayed by ~0.3 ms,
//!   the delay a sound from about 45° off-centre takes to reach the far
//!   ear. Previously only the low-pass crossfeed existed.
//! - The 5.1 -> stereo downmix uses the ITU-R BS.775 coefficients (centre
//!   and surrounds at -3 dB = 1/sqrt 2), then normalises so a full-scale
//!   input can't clip.
//! - Every channel goes through the same soft (Padé tanh) limiter as the
//!   core's mixer instead of some channels hard-clamping.

use super::audio_output::SurroundMode;

/// Interaural delay for the 3D headphone crossfeed, in seconds.
const ITD_SECONDS: f32 = 0.0003;

/// Padé approximation of tanh: transparent near zero, saturates smoothly.
#[inline]
pub fn soft_limit(x: f32) -> f32 {
    if x <= -3.0 {
        -1.0
    } else if x >= 3.0 {
        1.0
    } else {
        x * (27.0 + x * x) / (27.0 + 9.0 * x * x)
    }
}

pub struct SpatialDsp {
    lfe_state: f32,
    lfe_alpha: f32,
    /// Haas delay lines for the surround ambience (~12 ms).
    haas_l: Vec<f32>,
    haas_r: Vec<f32>,
    haas_idx: usize,
    /// Crossfeed low-pass state and its interaural delay lines.
    cross_l: f32,
    cross_r: f32,
    cross_alpha: f32,
    itd_l: Vec<f32>,
    itd_r: Vec<f32>,
    itd_idx: usize,
}

impl SpatialDsp {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate.max(8000) as f32;
        let haas = ((sr * 0.012) as usize).clamp(64, 1024);
        let itd = ((sr * ITD_SECONDS) as usize).max(1);
        Self {
            lfe_state: 0.0,
            lfe_alpha: (std::f32::consts::TAU * 120.0 / sr).clamp(0.001, 0.1),
            haas_l: vec![0.0; haas],
            haas_r: vec![0.0; haas],
            haas_idx: 0,
            cross_l: 0.0,
            cross_r: 0.0,
            cross_alpha: (std::f32::consts::TAU * 700.0 / sr).clamp(0.01, 0.3),
            itd_l: vec![0.0; itd],
            itd_r: vec![0.0; itd],
            itd_idx: 0,
        }
    }

    /// Interaural delay length in samples.
    pub fn itd_samples(&self) -> usize {
        self.itd_l.len()
    }

    /// Process one stereo frame into `frame` (1, 2, 6 or 8 channels).
    pub fn process(&mut self, left: f32, right: f32, mode: SurroundMode, bass: f32, width: f32, frame: &mut [f32]) {
        // Subwoofer / LFE low-pass.
        let mono = (left + right) * 0.5;
        self.lfe_state += self.lfe_alpha * (mono - self.lfe_state);
        let sub_lfe = self.lfe_state * (1.0 + bass * 2.5);

        // Surround ambience: side difference plus a Haas-delayed copy.
        let diff_l = left - 0.5 * right;
        let diff_r = right - 0.5 * left;
        let delayed_l = self.haas_l[self.haas_idx];
        let delayed_r = self.haas_r[self.haas_idx];
        self.haas_l[self.haas_idx] = diff_l;
        self.haas_r[self.haas_idx] = diff_r;
        self.haas_idx = (self.haas_idx + 1) % self.haas_l.len();
        let surround_l = (diff_l * 0.5 + delayed_l * 0.5) * width;
        let surround_r = (diff_r * 0.5 + delayed_r * 0.5) * width;
        let center = (left + right) * std::f32::consts::FRAC_1_SQRT_2;

        // 3D headphones: low-passed crossfeed, delayed by the ITD.
        self.cross_l += self.cross_alpha * (right - self.cross_l);
        self.cross_r += self.cross_alpha * (left - self.cross_r);
        let far_l = self.itd_l[self.itd_idx];
        let far_r = self.itd_r[self.itd_idx];
        self.itd_l[self.itd_idx] = self.cross_l;
        self.itd_r[self.itd_idx] = self.cross_r;
        self.itd_idx = (self.itd_idx + 1) % self.itd_l.len();
        let hp_l = left * 0.70 + far_l * 0.18 + sub_lfe * 0.15;
        let hp_r = right * 0.70 + far_r * 0.18 + sub_lfe * 0.15;

        let channels = frame.len();
        if channels >= 6 {
            match mode {
                SurroundMode::Stereo => {
                    frame[0] = soft_limit(left);
                    frame[1] = soft_limit(right);
                    frame[2..].fill(0.0);
                }
                SurroundMode::Surround51 | SurroundMode::Headphone3D => {
                    frame[0] = soft_limit(left);
                    frame[1] = soft_limit(right);
                    frame[2] = soft_limit(center);
                    frame[3] = soft_limit(sub_lfe);
                    frame[4] = soft_limit(surround_l);
                    frame[5] = soft_limit(surround_r);
                    if channels >= 8 {
                        frame[6] = soft_limit(surround_l * 0.85);
                        frame[7] = soft_limit(surround_r * 0.85);
                    }
                }
            }
        } else if channels == 2 {
            let (l, r) = match mode {
                SurroundMode::Stereo => (left, right),
                SurroundMode::Surround51 => {
                    // ITU-R BS.775: Lo = L + 0.707 C + 0.707 Ls (LFE is
                    // usually dropped; kept quietly here for the bass
                    // boost). Normalised by the sum of the gains.
                    const K: f32 = std::f32::consts::FRAC_1_SQRT_2;
                    const NORM: f32 = 1.0 / (1.0 + K + K);
                    (
                        (left + center * K + surround_l * K + sub_lfe * 0.25) * NORM,
                        (right + center * K + surround_r * K + sub_lfe * 0.25) * NORM,
                    )
                }
                SurroundMode::Headphone3D => (hp_l, hp_r),
            };
            frame[0] = soft_limit(l);
            frame[1] = soft_limit(r);
        } else if channels == 1 {
            frame[0] = soft_limit(mono);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soft_limit_is_transparent_small_and_bounded_large() {
        assert!((soft_limit(0.1) - 0.1).abs() < 0.001);
        assert!(soft_limit(10.0) <= 1.0 && soft_limit(-10.0) >= -1.0);
    }

    #[test]
    fn headphone_crossfeed_arrives_after_itd() {
        let mut dsp = SpatialDsp::new(44_100);
        let itd = dsp.itd_samples();
        assert_eq!(itd, 13); // 0.3 ms at 44.1 kHz
        // A click in the left channel only: the right ear hears the direct
        // path of nothing, and the crossfed copy only after the ITD.
        let mut right_ear = Vec::new();
        for i in 0..40 {
            let l = if i == 0 { 1.0 } else { 0.0 };
            let mut f = [0.0f32; 2];
            dsp.process(l, 0.0, SurroundMode::Headphone3D, 0.0, 0.0, &mut f);
            right_ear.push(f[1]);
        }
        // Before the ITD only the (tiny) LFE path contributes.
        let early = right_ear[..itd].iter().fold(0.0f32, |m, x| m.max(x.abs()));
        let late = right_ear[itd..itd + 5].iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(late > early * 2.0, "early {early} late {late}");
    }

    #[test]
    fn downmix_does_not_clip_full_scale() {
        let mut dsp = SpatialDsp::new(44_100);
        let mut peak = 0.0f32;
        for _ in 0..2000 {
            let mut f = [0.0f32; 2];
            dsp.process(1.0, 1.0, SurroundMode::Surround51, 1.0, 1.0, &mut f);
            peak = peak.max(f[0].abs()).max(f[1].abs());
        }
        assert!(peak < 1.0, "peak {peak}");
    }
}
