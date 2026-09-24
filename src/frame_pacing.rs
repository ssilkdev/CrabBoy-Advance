//! Frame pacing against the display's refresh rate.
//!
//! The GBA runs at 59.7275 Hz; nearly every monitor refreshes at 60 Hz (or a
//! multiple). Pacing emulation at the GBA's true rate on such a screen means
//! that about every 3.7 s one refresh gets no new frame, so the previous one
//! shows twice: a visible hitch while the picture scrolls. When repaints land
//! 1/60 s apart we emulate exactly one frame per repaint instead (0.46 %
//! faster, inaudible because audio is resampled against its queue level).
//!
//! egui doesn't report the monitor's refresh rate, so it's measured from the
//! gaps between repaints, which vsync snaps to whole refreshes. The median
//! over the last second ignores one-off stalls (menus, window drags).

/// The GBA's real refresh rate (16.78 MHz / 280,896 cycles per frame).
pub const GBA_REFRESH_HZ: f64 = 59.7275;

/// Repaint rates within this range lock emulation to 60 fps.
const LOCK_RANGE_HZ: std::ops::RangeInclusive<f64> = 59.0..=61.0;
const WINDOW: usize = 60;
const MIN_SAMPLES: usize = 30;

#[derive(Debug, Clone, Default)]
pub struct FramePacer {
    /// Recent repaint gaps in seconds (ring buffer).
    gaps: Vec<f64>,
    next: usize,
}

impl FramePacer {
    /// Record the time since the last repaint and return the emulation rate
    /// (frames per second at 1x speed) to use.
    pub fn base_rate(&mut self, since_last_repaint: std::time::Duration) -> f64 {
        let dt = since_last_repaint.as_secs_f64();
        if dt > 0.0 && dt < 0.1 {
            if self.gaps.len() < WINDOW {
                self.gaps.push(dt);
            } else {
                self.gaps[self.next] = dt;
            }
            self.next = (self.next + 1) % WINDOW;
        }
        self.base_rate_hint()
    }

    /// The rate `base_rate` would return now, without recording a gap.
    pub fn base_rate_hint(&self) -> f64 {
        match self.repaint_hz() {
            Some(hz) if LOCK_RANGE_HZ.contains(&hz) => 60.0,
            _ => GBA_REFRESH_HZ,
        }
    }

    /// Median repaint rate over the last second, once measured.
    pub fn repaint_hz(&self) -> Option<f64> {
        if self.gaps.len() < MIN_SAMPLES {
            return None;
        }
        let mut g = self.gaps.clone();
        g.sort_by(|a, b| a.total_cmp(b));
        Some(1.0 / g[g.len() / 2])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn feed(p: &mut FramePacer, gaps_ms: &[f64], n: usize) -> f64 {
        let mut r = 0.0;
        for i in 0..n {
            r = p.base_rate(Duration::from_secs_f64(gaps_ms[i % gaps_ms.len()] / 1000.0));
        }
        r
    }

    #[test]
    fn locks_to_a_60hz_display_despite_jitter_and_missed_refreshes() {
        let mut p = FramePacer::default();
        // ±1 ms jitter and one missed vsync in 10.
        let gaps = [16.2, 17.1, 16.6, 15.9, 17.3, 16.7, 16.4, 16.9, 16.6, 33.3];
        assert_eq!(feed(&mut p, &gaps, 120), 60.0);
    }

    #[test]
    fn a_120hz_display_showing_every_other_refresh_locks_too() {
        // Timer-driven repaints at ~60 fps snap to 2 refreshes, sometimes 3.
        let mut p = FramePacer::default();
        assert_eq!(feed(&mut p, &[16.67, 16.67, 16.67, 25.0], 120), 60.0);
    }

    #[test]
    fn uses_true_gba_rate_on_other_displays() {
        for gaps in [&[13.9, 13.9, 20.8][..], &[13.33][..], &[20.0][..], &[6.94, 6.94, 6.94][..]] {
            let mut p = FramePacer::default();
            assert_eq!(feed(&mut p, gaps, 200), GBA_REFRESH_HZ, "{gaps:?}");
        }
    }

    #[test]
    fn true_rate_until_measured() {
        let mut p = FramePacer::default();
        assert_eq!(feed(&mut p, &[16.67], MIN_SAMPLES - 1), GBA_REFRESH_HZ);
        assert_eq!(feed(&mut p, &[16.67], 1), 60.0);
    }
}
