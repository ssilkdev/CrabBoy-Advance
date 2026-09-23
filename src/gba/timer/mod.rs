//! GBA Hardware Timers (TM0 to TM3).
//!
//! ROADMAP M1: timers are **timestamp-based** rather than stepped. Each
//! running timer remembers the (prescaler-aligned) cycle at which its
//! counter last had a known value (`last_event`); the counter at any later
//! cycle is computed from the elapsed ticks. This gives cycle-exact reads
//! in the middle of an instruction (the old stepped model only updated
//! counters after each whole instruction) and exact overflow times, which
//! the IRQ logic needs to model the hardware's IRQ latency.
//!
//! The emulator calls [`TimerController::sync`] after every instruction,
//! and the register-write paths sync before changing anything. Overflows
//! found while syncing are queued (IRQ bits, the cycle of the earliest
//! IRQ-raising overflow, and per-timer overflow flags for DirectSound) and
//! collected with [`TimerController::take_events`].

pub struct Timer {
    pub reload: u16,
    /// Counter value at `last_event`. (For a count-up timer this is simply
    /// its current value: it only changes when the previous timer
    /// overflows.)
    pub counter: u16,
    pub cnt_h: u16,
    pub enabled: bool,
    /// Cycles per tick: 1, 64, 256 or 1024.
    pub prescaler_cycles: u32,
    /// Unused since the switch to timestamps; kept (always 0) so the
    /// save-state layout doesn't change.
    pub cycles_accum: u32,
    pub cascade: bool,
    pub irq_enable: bool,
    /// Prescaler-aligned cycle at which `counter` was valid. Not saved in
    /// save states; `rebase_all` re-anchors it on load.
    pub last_event: u64,
}

impl Default for Timer {
    fn default() -> Self {
        Self::new()
    }
}

impl Timer {
    pub fn new() -> Self {
        Self {
            reload: 0,
            counter: 0,
            cnt_h: 0,
            enabled: false,
            prescaler_cycles: 1,
            cycles_accum: 0,
            cascade: false,
            irq_enable: false,
            last_event: 0,
        }
    }

    #[inline]
    fn shift(&self) -> u32 {
        self.prescaler_cycles.trailing_zeros()
    }

    /// Whether the counter runs off the system clock (enabled and not in
    /// count-up mode; TM0 can't count up).
    #[inline]
    fn free_running(&self, idx: usize) -> bool {
        self.enabled && !(idx > 0 && self.cascade)
    }

    /// Counter value at cycle `now`, without changing state (register reads
    /// happen through `&self`).
    pub fn counter_at(&self, idx: usize, now: u64) -> u16 {
        if !self.free_running(idx) || now <= self.last_event {
            return self.counter;
        }
        let ticks = (now - self.last_event) >> self.shift();
        let pos = self.counter as u64 + ticks;
        if pos < 0x1_0000 {
            return pos as u16;
        }
        let period = 0x1_0000 - self.reload as u64;
        (self.reload as u64 + (pos - 0x1_0000) % period) as u16
    }

    /// Re-anchor the counter at `now`, aligned down to a prescaler tick
    /// (the prescalers divide one free-running system counter).
    fn rebase(&mut self, now: u64) {
        self.last_event = now & !((self.prescaler_cycles as u64) - 1);
    }

    /// TMxCNT_H write. The controller has already synced to `now`.
    pub fn write_cnt_h(&mut self, val: u16, idx: usize, now: u64) {
        let was_enabled = self.enabled;
        let old_prescaler = self.prescaler_cycles;
        let old_cascade = self.cascade;
        self.cnt_h = val & 0x00C7;
        self.enabled = (val & (1 << 7)) != 0;
        self.cascade = idx > 0 && (val & (1 << 2)) != 0;
        self.irq_enable = (val & (1 << 6)) != 0;
        self.prescaler_cycles = [1, 64, 256, 1024][(val & 3) as usize];

        if !was_enabled && self.enabled {
            self.counter = self.reload;
            self.rebase(now);
        } else if old_prescaler != self.prescaler_cycles || old_cascade != self.cascade {
            self.rebase(now);
        }
    }
}

/// Events produced by timer overflows since the last `take_events`.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct TimerEvents {
    /// IRQ bits (bit n = timer n) to raise.
    pub irq_mask: u8,
    /// Cycle of the earliest IRQ-raising overflow.
    pub irq_time: u64,
    /// Which timers overflowed at least once (DirectSound FIFO clocking).
    pub overflows: [bool; 4],
}

pub struct TimerController {
    pub timers: [Timer; 4],
    pending: TimerEvents,
}

impl Default for TimerController {
    fn default() -> Self {
        Self::new()
    }
}

impl TimerController {
    pub fn new() -> Self {
        Self {
            timers: [Timer::new(), Timer::new(), Timer::new(), Timer::new()],
            pending: TimerEvents::default(),
        }
    }

    /// Bring every counter up to cycle `now`, queueing overflow events.
    pub fn sync(&mut self, now: u64) {
        // (overflow count, time of first overflow) of the previous timer,
        // which clocks a count-up timer.
        let mut prev: (u64, u64) = (0, 0);
        for i in 0..4 {
            let t = &mut self.timers[i];
            let (count, first) = if t.free_running(i) {
                if now <= t.last_event {
                    (0, 0)
                } else {
                    let shift = t.shift();
                    let ticks = (now - t.last_event) >> shift;
                    let to_first = 0x1_0000 - t.counter as u64;
                    if ticks < to_first {
                        t.counter += ticks as u16;
                        t.last_event += ticks << shift;
                        (0, 0)
                    } else {
                        let period = 0x1_0000 - t.reload as u64;
                        let rest = ticks - to_first;
                        let first = t.last_event + (to_first << shift);
                        t.counter = (t.reload as u64 + rest % period) as u16;
                        t.last_event += ticks << shift;
                        (1 + rest / period, first)
                    }
                }
            } else if t.enabled && i > 0 && t.cascade && prev.0 > 0 {
                let to_first = 0x1_0000 - t.counter as u64;
                if prev.0 < to_first {
                    t.counter += prev.0 as u16;
                    (0, 0)
                } else {
                    let period = 0x1_0000 - t.reload as u64;
                    let rest = prev.0 - to_first;
                    t.counter = (t.reload as u64 + rest % period) as u16;
                    (1 + rest / period, prev.1)
                }
            } else {
                (0, 0)
            };
            if count > 0 {
                self.pending.overflows[i] = true;
                if t.irq_enable {
                    if self.pending.irq_mask == 0 || first < self.pending.irq_time {
                        self.pending.irq_time = first;
                    }
                    self.pending.irq_mask |= 1 << i;
                }
            }
            prev = (count, first);
        }
    }

    /// Take (and clear) the queued overflow events.
    pub fn take_events(&mut self) -> TimerEvents {
        std::mem::take(&mut self.pending)
    }

    /// TMxCNT_L write (reload value) at cycle `now`.
    pub fn write_reload(&mut self, idx: usize, val: u16, now: u64) {
        self.sync(now);
        self.timers[idx].reload = val;
    }

    /// TMxCNT_H write at cycle `now`.
    pub fn write_control(&mut self, idx: usize, val: u16, now: u64) {
        self.sync(now);
        self.timers[idx].write_cnt_h(val, idx, now);
    }

    /// TMxCNT_L read at cycle `now`.
    pub fn read_counter(&self, idx: usize, now: u64) -> u16 {
        self.timers[idx].counter_at(idx, now)
    }

    /// After loading a save state: anchor every timer at `now` (the anchor
    /// isn't serialized; the saved counter is exact at save time).
    pub fn rebase_all(&mut self, now: u64) {
        for t in &mut self.timers {
            t.cycles_accum = 0;
            t.rebase(now);
        }
        self.pending = TimerEvents::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_is_exact_between_syncs() {
        let mut c = TimerController::new();
        c.write_reload(0, 0xFFF0, 100);
        c.write_control(0, 0x80, 100);
        assert_eq!(c.read_counter(0, 100), 0xFFF0);
        assert_eq!(c.read_counter(0, 105), 0xFFF5);
        // Wraps to reload after 16 ticks.
        assert_eq!(c.read_counter(0, 116), 0xFFF0);
        assert_eq!(c.read_counter(0, 117), 0xFFF1);
    }

    #[test]
    fn overflow_time_and_irq_are_reported() {
        let mut c = TimerController::new();
        c.write_reload(0, 0xFFFE, 1000);
        c.write_control(0, 0xC0, 1000); // enable + IRQ, prescaler 1
        c.sync(1010);
        let ev = c.take_events();
        assert_eq!(ev.irq_mask, 1);
        assert_eq!(ev.irq_time, 1002);
        assert!(ev.overflows[0]);
        assert_eq!(c.take_events(), TimerEvents::default());
    }

    #[test]
    fn prescaler_aligns_to_tick_boundary() {
        let mut c = TimerController::new();
        c.write_control(0, 0x81, 100); // /64, anchored at 64
        assert_eq!(c.read_counter(0, 127), 0);
        assert_eq!(c.read_counter(0, 128), 1);
    }

    #[test]
    fn cascade_counts_every_overflow() {
        let mut c = TimerController::new();
        c.write_reload(0, 0xFFFF, 0);
        c.write_control(1, 0x84, 0); // TM1 count-up
        c.write_control(0, 0x80, 0); // TM0 overflows every cycle
        c.sync(10);
        assert_eq!(c.timers[1].counter, 10);
        assert!(!c.take_events().overflows[1]);
    }
}
