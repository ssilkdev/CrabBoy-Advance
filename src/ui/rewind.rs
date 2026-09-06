//! Real-Time Rewind / Time-Travel Engine for GBA Emulation

use crate::gba::Gba;
use std::collections::VecDeque;

pub struct RewindManager {
    states: VecDeque<Vec<u8>>,
    max_states: usize,
    frame_counter: usize,
    capture_interval: usize,
}

impl Default for RewindManager {
    fn default() -> Self {
        Self::new(120, 6) // 120 states captured every 6 frames = ~12 seconds of fluid rewind
    }
}

impl RewindManager {
    pub fn new(max_states: usize, capture_interval: usize) -> Self {
        Self {
            states: VecDeque::with_capacity(max_states),
            max_states,
            frame_counter: 0,
            capture_interval: capture_interval.max(1),
        }
    }

    /// Records a save state snapshot at regular frame intervals during normal emulation.
    pub fn record_frame(&mut self, gba: &Gba) {
        self.frame_counter += 1;
        if self.frame_counter % self.capture_interval == 0 {
            let state = gba.save_state();
            if self.states.len() >= self.max_states {
                self.states.pop_front();
            }
            self.states.push_back(state);
        }
    }

    /// Steps emulation one snapshot backwards in time.
    /// Returns true if a state was successfully restored, false if the buffer is empty.
    pub fn rewind_step(&mut self, gba: &mut Gba) -> bool {
        if let Some(state) = self.states.pop_back() {
            gba.load_state(&state)
        } else {
            false
        }
    }

    /// Returns the approximate number of seconds of rewind history currently available.
    pub fn available_seconds(&self) -> f32 {
        (self.states.len() * self.capture_interval) as f32 / 59.7275
    }

    /// Clears the rewind buffer (e.g. on ROM reload or hard reset).
    pub fn clear(&mut self) {
        self.states.clear();
        self.frame_counter = 0;
    }

    /// Returns the number of snapshots currently stored.
    pub fn state_count(&self) -> usize {
        self.states.len()
    }
}
